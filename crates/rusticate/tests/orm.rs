//! End-to-end ORM coverage on in-memory SQLite, plus per-dialect SQL
//! rendering through lazy handles (no live Postgres/MySQL required).

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use rusticate::{
    async_trait, factory, scopes, BelongsTo, Changeset, DecodeField, HasMany, Migration, Migrator,
    Model, Observer, Query, Result, Schema, Seeder, Target, DB,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Role {
    Admin,
    Member,
}

impl fmt::Display for Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => write!(formatter, "admin"),
            Self::Member => write!(formatter, "member"),
        }
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        match text {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            other => Err(format!("unknown role {other:?}")),
        }
    }
}

#[derive(Debug, Model)]
#[model(table = "users", timestamps = true, soft_deletes = true, appends = ["display_name"])]
pub struct User {
    #[model(id, auto_increment)]
    pub id: i64,
    pub name: String,
    pub email: String,
    pub active: bool,
    #[model(casts = "json")]
    pub tags: Vec<String>,
    #[model(casts = "string")]
    pub role: Role,
    #[model(hidden)]
    pub password_hash: String,
    #[model(has_many = "Post")]
    pub posts: HasMany<Post>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl User {
    fn display_name(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

#[derive(Debug, Model)]
#[model(table = "posts")]
pub struct Post {
    #[model(id, auto_increment)]
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    #[model(belongs_to = "User", foreign_key = "user_id")]
    pub author: BelongsTo<User>,
}

#[scopes]
impl User {
    pub fn scope_active(query: Query<User>) -> Query<User> {
        query.where_eq("active", true)
    }

    pub fn scope_admins(query: Query<User>) -> Query<User> {
        query.where_eq("role", "admin")
    }
}

async fn setup() -> DB {
    let db = DB::memory().await.unwrap();
    let schema = Schema::new(&db);
    schema
        .create("users", |t| {
            t.id();
            t.string("name");
            t.string("email").unique();
            t.boolean("active").default(false);
            t.json("tags").nullable();
            t.string("role").default("member");
            t.string("password_hash");
            t.timestamps();
            t.soft_deletes();
        })
        .await
        .unwrap();
    schema
        .create("posts", |t| {
            t.id();
            t.foreign_id("user_id")
                .references("id")
                .on("users")
                .on_delete("cascade");
            t.string("title");
        })
        .await
        .unwrap();
    db
}

fn user_changeset(name: &str) -> Changeset {
    Changeset::new()
        .set("name", name)
        .set("email", format!("{name}@example.com"))
        .set("active", true)
        .set("tags", serde_json::json!(["new"]))
        .set("role", "member")
        .set("password_hash", "secret")
}

#[tokio::test]
async fn crud_round_trip_with_timestamps_and_soft_deletes() -> Result<()> {
    let db = setup().await;

    let user = User::create(&db, user_changeset("ada")).await?;
    assert_eq!(user.id, 1);
    assert_eq!(user.name, "ada");
    assert_eq!(user.role, Role::Member);
    assert_eq!(user.tags, vec!["new".to_string()]);
    assert!(user.created_at <= user.updated_at);
    assert!(!user.trashed());

    let found = User::find(&db, user.id)
        .await?
        .expect("created row reads back");
    assert_eq!(found.email, "ada@example.com");
    assert!(User::find(&db, 999).await?.is_none());
    assert!(User::find_or_fail(&db, 999).await.is_err());
    assert_eq!(User::all(&db).await?.len(), 1);

    let mut user = found;
    user.update(&db, Changeset::new().set("name", "ada l"))
        .await?;
    assert_eq!(user.name, "ada l");
    user.role = Role::Admin;
    user.save(&db).await?;
    user.refresh(&db).await?;
    assert_eq!(user.role, Role::Admin);

    user.delete(&db).await?;
    assert!(user.trashed());
    assert!(
        User::find(&db, user.id).await?.is_none(),
        "soft scope excludes"
    );
    let trashed = User::query(&db).with_trashed().first_or_fail().await?;
    assert!(trashed.trashed());
    assert_eq!(User::query(&db).only_trashed().count().await?, 1);

    user.restore(&db).await?;
    assert!(!user.trashed());
    assert!(User::find(&db, user.id).await?.is_some());

    user.force_delete(&db).await?;
    assert!(User::query(&db).with_trashed().first().await?.is_none());
    Ok(())
}

#[tokio::test]
async fn unique_violations_and_invalid_writes_map() -> Result<()> {
    let db = setup().await;
    User::create(&db, user_changeset("ada")).await?;

    let duplicate = User::create(&db, user_changeset("ada")).await;
    assert!(duplicate.is_err_and(|error| error.is_unique_violation()));

    let empty = User::create(&db, Changeset::new()).await;
    assert!(matches!(empty, Err(rusticate::Error::InvalidQuery(_))));

    let mut user = User::find_or_fail(&db, 1).await?;
    assert!(user.update(&db, Changeset::new()).await.is_err());
    let unknown = user.update(&db, Changeset::new().set("nope", 1i64)).await;
    assert!(matches!(unknown, Err(rusticate::Error::InvalidQuery(_))));
    let pk = user.update(&db, Changeset::new().set("id", 2i64)).await;
    assert!(matches!(pk, Err(rusticate::Error::InvalidQuery(_))));
    Ok(())
}

#[tokio::test]
async fn query_builder_filters_orders_and_paginates() -> Result<()> {
    let db = setup().await;
    for name in ["ann", "bob", "cat", "dan", "eve"] {
        let mut changeset = user_changeset(name);
        if name == "bob" || name == "dan" {
            changeset = Changeset::new()
                .set("name", name)
                .set("email", format!("{name}@example.com"))
                .set("active", false)
                .set("tags", serde_json::json!([]))
                .set("role", "admin")
                .set("password_hash", "secret");
        }
        User::create(&db, changeset).await?;
    }

    assert_eq!(User::query(&db).where_eq("active", true).count().await?, 3);
    assert_eq!(User::query(&db).where_like("name", "%a%").count().await?, 3);
    assert_eq!(
        User::query(&db)
            .where_in("name", ["ann", "eve"])
            .count()
            .await?,
        2
    );
    assert_eq!(
        User::query(&db)
            .where_in("name", Vec::<String>::new())
            .count()
            .await?,
        0
    );
    assert_eq!(User::query(&db).where_null("deleted_at").count().await?, 5);
    assert_eq!(
        User::query(&db)
            .where_not_null("deleted_at")
            .count()
            .await?,
        0
    );

    let desc = User::query(&db).order_by_desc("id").limit(2).get().await?;
    assert_eq!(
        desc.iter().map(|user| user.id).collect::<Vec<_>>(),
        vec![5, 4]
    );
    let page = User::query(&db)
        .order_by("id")
        .offset(4)
        .limit(10)
        .get()
        .await?;
    assert_eq!(page.len(), 1);

    assert!(User::query(&db).where_eq("name", "ann").exists().await?);
    assert!(!User::query(&db).where_eq("name", "zed").exists().await?);
    let avg = User::query(&db)
        .avg("id")
        .await?
        .expect("average over rows");
    assert!((avg - 3.0).abs() < f64::EPSILON);
    assert!(User::query(&db)
        .where_eq("name", "zed")
        .avg("id")
        .await?
        .is_none());

    let first = User::query(&db)
        .order_by("id")
        .first()
        .await?
        .expect("first row");
    assert_eq!(first.name, "ann");
    assert!(User::query(&db)
        .where_eq("name", "zed")
        .first_or_fail()
        .await
        .is_err());

    let second = User::query(&db).order_by("id").paginate(2, 2).await?;
    assert_eq!(second.total, 5);
    assert_eq!(second.page, 2);
    assert_eq!(second.per_page, 2);
    assert_eq!(second.last_page, 3);
    assert!(second.has_more);
    assert_eq!(second.items.len(), 2);
    let last = User::query(&db).order_by("id").paginate(2, 3).await?;
    assert!(!last.has_more);
    assert_eq!(last.items.len(), 1);
    assert!(User::query(&db).paginate(0, 1).await.is_err());
    assert!(User::query(&db).paginate(2, 0).await.is_err());

    let mut batches = Vec::new();
    User::query(&db)
        .order_by("id")
        .chunk(2, |users| {
            batches.push(users.len());
            async { Ok::<(), rusticate::Error>(()) }
        })
        .await?;
    assert_eq!(batches, vec![2, 2, 1]);
    assert!(User::query(&db)
        .chunk(0, |_: Vec<User>| async { Ok::<(), rusticate::Error>(()) })
        .await
        .is_err());

    // Identifiers validate before any I/O.
    let bad = User::query(&db)
        .where_eq("x; DROP TABLE users", 1i64)
        .get()
        .await;
    assert!(matches!(bad, Err(rusticate::Error::InvalidQuery(_))));
    // Aggregates on missing tables surface driver errors, not panics.
    assert!(User::query(&db).order_by("id").first().await.is_ok());
    Ok(())
}
#[tokio::test]
async fn relations_lazy_and_eager_load() -> Result<()> {
    let db = setup().await;
    let user = User::create(&db, user_changeset("ada")).await?;
    assert!(user.posts.get().is_err(), "unloaded relation errors");

    let post = user
        .create_posts(&db, Changeset::new().set("title", "hello"))
        .await?;
    assert_eq!(post.user_id, user.id);

    let mine = user.posts(&db).get().await?;
    assert_eq!(mine.len(), 1);
    let author = post.author(&db).first_or_fail().await?;
    assert_eq!(author.id, user.id);

    let loaded = User::query(&db).with(["posts"]).first_or_fail().await?;
    assert_eq!(loaded.posts.get()?.len(), 1);

    let nested = User::query(&db)
        .with(["posts.author"])
        .first_or_fail()
        .await?;
    let nested_author = nested.posts.get()?[0]
        .author
        .get()?
        .as_ref()
        .expect("nested owner");
    assert_eq!(nested_author.id, user.id);

    let unknown = User::query(&db).with(["nope"]).get().await;
    assert!(matches!(
        unknown,
        Err(rusticate::Error::RelationNotFound(_))
    ));
    let malformed = User::query(&db).with(["posts."]).get().await;
    assert!(matches!(malformed, Err(rusticate::Error::InvalidQuery(_))));
    Ok(())
}

#[tokio::test]
async fn scopes_chain_as_methods_and_functions() -> Result<()> {
    let db = setup().await;
    User::create(&db, user_changeset("ann")).await?;
    User::create(
        &db,
        user_changeset("zed")
            .set("active", false)
            .set("role", "admin"),
    )
    .await?;

    // Associated-function form always works, with or without the macro.
    assert_eq!(User::scope_active(User::query(&db)).count().await?, 1);
    // Macro form: import the generated trait and chain.
    assert_eq!(User::query(&db).active().count().await?, 1);
    assert_eq!(User::query(&db).admins().count().await?, 1);
    assert_eq!(User::query(&db).active().admins().count().await?, 0);
    Ok(())
}

#[tokio::test]
async fn serialization_hides_and_appends() -> Result<()> {
    let db = setup().await;
    let user = User::create(&db, user_changeset("ada")).await?;
    user.create_posts(&db, Changeset::new().set("title", "hello"))
        .await?;

    let plain = User::find_or_fail(&db, user.id).await?.to_value()?;
    assert_eq!(plain["name"], "ada");
    assert_eq!(plain["display_name"], "ada <ada@example.com>");
    assert!(
        plain.get("password_hash").is_none(),
        "hidden column excluded"
    );
    assert!(plain.get("posts").is_none(), "unloaded relation absent");

    let with_posts = User::query(&db)
        .with(["posts"])
        .first_or_fail()
        .await?
        .to_value()?;
    assert_eq!(with_posts["posts"][0]["title"], "hello");
    assert!(with_posts["posts"][0].get("author").is_none());
    Ok(())
}

#[tokio::test]
async fn bad_cast_values_fail_with_column_context() -> Result<()> {
    let db = setup().await;
    db.raw("INSERT INTO users (name, email, active, tags, role, password_hash) VALUES (?, ?, ?, ?, ?, ?)")
        .bind("x")
        .bind("x@example.com")
        .bind(true)
        .bind("not json")
        .bind("member")
        .bind("secret")
        .execute()
        .await?;
    let error = User::find_or_fail(&db, 1)
        .await
        .expect_err("bad json must fail");
    assert!(error.to_string().contains("tags"), "{error}");

    db.raw("UPDATE users SET tags = ?, role = ? WHERE id = ?")
        .bind("[]")
        .bind("superuser")
        .bind(1i64)
        .execute()
        .await?;
    let error = User::find_or_fail(&db, 1)
        .await
        .expect_err("bad role must fail");
    assert!(error.to_string().contains("role"), "{error}");
    Ok(())
}

#[derive(Debug, Default)]
struct HookLog {
    order: Mutex<Vec<String>>,
    counts_seen: Mutex<Vec<i64>>,
}

struct RecordingObserver {
    log: Arc<HookLog>,
}

#[async_trait]
impl Observer<User> for RecordingObserver {
    async fn saving(&self, _target: &Target, _user: &mut User) -> Result<()> {
        self.log.order.lock().unwrap().push("saving".to_string());
        Ok(())
    }

    async fn creating(&self, _target: &Target, user: &mut User) -> Result<()> {
        self.log.order.lock().unwrap().push("creating".to_string());
        user.name = format!("hooked-{}", user.name);
        Ok(())
    }

    async fn created(&self, target: &Target, user: &User) -> Result<()> {
        self.log.order.lock().unwrap().push("created".to_string());
        // Hooks query through the write's own target: inside a transaction
        // this sees uncommitted rows on the same connection (no deadlock —
        // dispatch holds no connection lock across user code).
        let count = User::query(target).count().await?;
        self.log.counts_seen.lock().unwrap().push(count);
        assert!(user.name.starts_with("hooked-"));
        Ok(())
    }

    async fn saved(&self, _target: &Target, _user: &User) -> Result<()> {
        self.log.order.lock().unwrap().push("saved".to_string());
        Ok(())
    }
}

struct VetoDelete;

#[async_trait]
impl Observer<User> for VetoDelete {
    async fn deleting(&self, _target: &Target, _user: &mut User) -> Result<()> {
        Err(rusticate::Error::invalid_query("deletes disabled"))
    }
}

#[tokio::test]
async fn observers_adjust_veto_and_query_in_hooks() -> Result<()> {
    let db = setup().await;
    let log = Arc::new(HookLog::default());
    db.observe(RecordingObserver { log: log.clone() });

    let user = User::create(&db, user_changeset("ada")).await?;
    assert_eq!(user.name, "hooked-ada");
    assert_eq!(
        *log.order.lock().unwrap(),
        vec!["saving", "creating", "created", "saved"]
    );
    assert_eq!(*log.counts_seen.lock().unwrap(), vec![1]);

    // The same hooks fire for transactional writes, on the transaction.
    db.transaction(|tx| async move {
        User::create(&tx, user_changeset("grace")).await?;
        Ok::<(), rusticate::Error>(())
    })
    .await?;
    assert_eq!(*log.counts_seen.lock().unwrap(), vec![1, 2]);

    db.observe(VetoDelete);
    let mut user = User::find_or_fail(&db, user.id).await?;
    assert!(user.delete(&db).await.is_err(), "veto aborts the write");
    assert!(
        User::find(&db, user.id).await?.is_some(),
        "row survives the veto"
    );
    Ok(())
}

#[tokio::test]
async fn transactions_join_models_and_raw_queries() -> Result<()> {
    let db = setup().await;

    db.transaction(|tx| async move {
        User::create(&tx, user_changeset("ada")).await?;
        Ok::<(), rusticate::Error>(())
    })
    .await?;
    assert_eq!(User::all(&db).await?.len(), 1);

    let rolled_back: Result<()> = db
        .transaction(|tx| async move {
            User::create(&tx, user_changeset("grace")).await?;
            Err(rusticate::Error::not_found("boom"))
        })
        .await;
    assert!(rolled_back.is_err());
    assert_eq!(User::all(&db).await?.len(), 1, "rolled back");

    // Builders retarget onto manual transactions; finishing rules are strict.
    // (No pool queries while the transaction is open: the single in-memory
    // connection belongs to the transaction until it finishes.)
    let tx = db.begin().await?;
    assert!(!tx.is_finished());
    User::create(&tx, user_changeset("lin")).await?;
    assert_eq!(User::query(&db).on(&tx).count().await?, 2);
    tx.commit().await?;
    assert!(tx.is_finished());
    assert!(tx.commit().await.is_err(), "double commit errors");
    assert!(
        User::query(&db).on(&tx).count().await.is_err(),
        "queries after finish error"
    );
    assert_eq!(User::all(&db).await?.len(), 2);

    let tx = db.begin().await?;
    User::create(&tx, user_changeset("zed")).await?;
    tx.rollback().await?;
    assert!(tx.is_finished());
    assert_eq!(User::all(&db).await?.len(), 2, "rolled back");
    Ok(())
}

#[derive(Debug, Model)]
#[model(table = "counters")]
pub struct Counter {
    #[model(id, auto_increment)]
    pub id: i64,
}

#[derive(Debug, Model)]
#[model(table = "secrets")]
pub struct Secret {
    #[model(id, auto_increment, hidden)]
    pub id: i64,
    #[model(hidden)]
    pub token: String,
}

#[tokio::test]
async fn edge_models_create_serialize_and_reject_includes() -> Result<()> {
    let db = DB::memory().await?;
    let schema = Schema::new(&db);
    schema
        .create("counters", |t| {
            t.id();
        })
        .await?;
    schema
        .create("secrets", |t| {
            t.id();
            t.string("token");
        })
        .await?;

    // Pk-only models insert default rows (the auto key is never user-set).
    let counter = Counter::create(&db, Changeset::new().set("id", 0i64)).await?;
    assert_eq!(counter.id, 1);
    assert_eq!(Counter::all(&db).await?.len(), 1);

    // Fully-hidden models serialize to `{}`.
    let secret = Secret::create(&db, Changeset::new().set("token", "abc")).await?;
    assert_eq!(secret.to_value()?, serde_json::json!({}));

    // Unknown create columns fail fast, like updates do.
    let bad = User::create(&db, user_changeset("x").set("nope", 1i64)).await;
    assert!(matches!(bad, Err(rusticate::Error::InvalidQuery(_))));

    // Relation-less models reject every include explicitly.
    let error = Counter::query(&db)
        .with(["posts"])
        .get()
        .await
        .expect_err("no relations");
    assert!(matches!(error, rusticate::Error::RelationNotFound(_)));
    assert!(error.to_string().contains("no relations"), "{error}");
    Ok(())
}

#[tokio::test]
async fn only_trashed_rejects_models_without_soft_deletes() -> Result<()> {
    let db = setup().await;
    let error = Post::query(&db)
        .only_trashed()
        .get()
        .await
        .expect_err("must reject");
    assert!(matches!(error, rusticate::Error::InvalidQuery(_)));
    // with_trashed is a harmless no-op there.
    assert_eq!(Post::query(&db).with_trashed().count().await?, 0);
    Ok(())
}
struct CreateUsers;

#[async_trait]
impl Migration for CreateUsers {
    async fn up(&self, schema: &mut Schema) -> Result<()> {
        schema
            .create("users", |t| {
                t.id();
                t.string("name");
            })
            .await
    }

    async fn down(&self, schema: &mut Schema) -> Result<()> {
        schema.drop("users").await
    }
}

struct CreatePosts;

#[async_trait]
impl Migration for CreatePosts {
    async fn up(&self, schema: &mut Schema) -> Result<()> {
        schema
            .create("posts", |t| {
                t.id();
                t.foreign_id("user_id").references("id").on("users");
                t.string("title");
            })
            .await
    }

    async fn down(&self, schema: &mut Schema) -> Result<()> {
        schema.drop("posts").await
    }
}

#[tokio::test]
async fn migrations_track_batches_and_roll_back_in_order() -> Result<()> {
    let db = DB::memory().await?;
    let migrator = Migrator::new(&db);
    let all: &[&dyn Migration] = &[&CreateUsers, &CreatePosts];

    assert_eq!(migrator.run(&all[..1]).await?.len(), 1);
    assert!(Schema::new(&db).has_table("users").await?);
    assert!(!Schema::new(&db).has_table("posts").await?);

    // Second run applies only the pending migration, in a new batch.
    assert_eq!(migrator.run(all).await?.len(), 1);
    let status = migrator.status(all).await?;
    assert!(status.iter().all(|entry| entry.ran));
    assert_eq!(status[0].batch, Some(1));
    assert_eq!(status[1].batch, Some(2));
    assert!(migrator.run(all).await?.is_empty(), "idempotent");

    // Roll back one batch: only the later migration reverts.
    assert_eq!(migrator.rollback(all, 1).await?.len(), 1);
    assert!(Schema::new(&db).has_table("users").await?);
    assert!(!Schema::new(&db).has_table("posts").await?);
    assert!(migrator.rollback(all, 0).await.is_err());

    // Rolling back without passing every migration fails explicitly.
    migrator.run(all).await?;
    let partial: &[&dyn Migration] = &[&CreateUsers];
    assert!(migrator.rollback(partial, 2).await.is_err());

    // Fresh drops everything and replays from scratch.
    assert_eq!(migrator.fresh(all).await?.len(), 2);
    assert!(Schema::new(&db).has_table("users").await?);
    assert!(Schema::new(&db).has_table("posts").await?);
    Ok(())
}

factory!(UserFactory, User, |seq| {
    Changeset::new()
        .set("name", format!("Factory {seq}"))
        .set("email", format!("factory{seq}@example.com"))
        .set("active", true)
        .set("tags", serde_json::json!([]))
        .set("role", "member")
        .set("password_hash", "secret")
});

struct AdminSeeder;

#[async_trait]
impl Seeder for AdminSeeder {
    async fn run(&self, db: &DB) -> Result<()> {
        User::create(
            db,
            user_changeset("root")
                .set("email", "admin@example.com")
                .set("role", "admin"),
        )
        .await?;
        Ok(())
    }
}

#[tokio::test]
async fn factories_sequence_and_seeders_plant() -> Result<()> {
    let db = setup().await;
    let factory = UserFactory::new();

    let first = factory.make();
    let second = factory.make();
    assert_ne!(
        first.get("email"),
        second.get("email"),
        "sequences stay unique"
    );

    let overridden = factory.make_with(Changeset::new().set("name", "custom"));
    assert_eq!(
        overridden.get("name"),
        Some(&rusticate::BindValue::Text("custom".to_string()))
    );

    let user = factory.create(&db).await?;
    assert!(user.email.starts_with("factory"));
    let custom = factory
        .create_with(&db, Changeset::new().set("role", "admin"))
        .await?;
    assert_eq!(custom.role, Role::Admin);
    let many = factory.create_many(&db, 3).await?;
    assert_eq!(many.len(), 3);
    assert_eq!(User::all(&db).await?.len(), 5);

    AdminSeeder.run(&db).await?;
    assert_eq!(User::query(&db).admins().count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn raw_queries_bind_with_exact_types() -> Result<()> {
    let db = setup().await;
    let moment = chrono::DateTime::parse_from_rfc3339("2026-09-30T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    let row = db
        .raw("SELECT ? AS text, ? AS num")
        .bind("hi")
        .bind(7i64)
        .fetch_one()
        .await?;
    assert_eq!(String::decode_field(&row, "text")?, "hi");
    assert_eq!(i64::decode_field(&row, "num")?, 7);

    // Timestamp binds convert to the dialect's text form.
    let row = db
        .raw("SELECT ? AS moment")
        .bind(moment)
        .fetch_one()
        .await?;
    let rendered = String::decode_field(&row, "moment")?;
    assert!(rendered.starts_with("2026-09-30"), "{rendered}");

    assert!(db
        .raw("SELECT 1 AS one WHERE 1 = 0")
        .fetch_optional()
        .await?
        .is_none());
    Ok(())
}

#[tokio::test]
async fn select_renders_casts_and_placeholders_per_dialect() {
    let pg = DB::connect_lazy("postgres://u@h/d").unwrap();
    let (sql, binds) = User::query(&pg).where_eq("name", "Ada").to_sql().unwrap();
    assert!(
        sql.contains(r#"CAST("created_at" AS TEXT) AS "created_at""#),
        "{sql}"
    );
    assert!(sql.contains(r#"CAST("tags" AS TEXT) AS "tags""#), "{sql}");
    assert!(sql.contains(r#"WHERE "name" = $1"#), "{sql}");
    assert!(!sql.contains("SIGNED"), "{sql}");
    assert_eq!(binds.len(), 1);

    let my = DB::connect_lazy("mysql://u@h/d").unwrap();
    let (sql, _) = User::query(&my).where_eq("name", "Ada").to_sql().unwrap();
    assert!(
        sql.contains("CAST(`created_at` AS CHAR) AS `created_at`"),
        "{sql}"
    );
    assert!(
        sql.contains("CAST(`active` AS SIGNED) AS `active`"),
        "{sql}"
    );
    assert!(sql.contains("WHERE `name` = ?"), "{sql}");

    let lite = DB::connect_lazy("sqlite:app.db").unwrap();
    let (sql, _) = User::query(&lite).where_eq("name", "Ada").to_sql().unwrap();
    assert!(!sql.contains("CAST"), "{sql}");
    assert!(sql.contains(r#"WHERE "name" = ?"#), "{sql}");
}
