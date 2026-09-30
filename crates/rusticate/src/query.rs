//! Chainable, Eloquent-flavored query builder.
//!
//! [`Query`] is constructed with a target ([`Model::query`] takes `&db` or
//! `&tx`), refined with `where_*` / ordering / paging calls, and executed
//! with a terminal (`get`, `first`, `count`, `paginate`, …). Builders own
//! their target (cheap clones, no lifetimes) and can be retargeted onto a
//! transaction with [`.on(&tx)`](Query::on).
//!
//! Identifiers from user code are validated when SQL renders (before any
//! I/O); values always bind — the only interpolated literals are `NULL`,
//! `LIMIT`/`OFFSET` integers (digits-only, injection-proof, and required
//! inline for MySQL prepared-statement compatibility), and the `0 = 1`
//! an empty `where_in` degrades to.

use std::future::Future;
use std::marker::PhantomData;

use crate::db::{Dialect, Target};
use crate::model::{Model, Page};
use crate::value::{BindValue, DecodeField, EncodeField};
use crate::{Error, Result};

/// One `WHERE` condition (validated and rendered at build time).
#[derive(Debug, Clone)]
enum Where {
    Eq(String, BindValue),
    Like(String, String),
    In(String, Vec<BindValue>),
    IsNull(String),
    IsNotNull(String),
}

/// Sort direction (separate methods, never raw strings).
#[derive(Debug, Clone, Copy)]
enum Order {
    Asc,
    Desc,
}

/// Query builder for model `M`. See the [module docs](self) for the flow.
///
/// Cloning is cheap (shared target, cloned conditions) — [`paginate`](Query::paginate)
/// clones internally to count and fetch from the same base.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{Model, DB};
/// # #[derive(Model)]
/// # #[model(table = "users")]
/// # pub struct User {
/// #     #[model(id, auto_increment)]
/// #     pub id: i64,
/// #     pub name: String,
/// # }
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let users = User::query(&db).where_eq("name", "Ada").order_by("id").get().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Query<M> {
    target: Target,
    wheres: Vec<Where>,
    orders: Vec<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
    includes: Vec<String>,
    with_trashed: bool,
    only_trashed: bool,
    model: PhantomData<M>,
}

/// Manual `Clone` without a `M: Clone` bound (a derive would demand one,
/// and models are never required to be `Clone`).
impl<M> Clone for Query<M> {
    fn clone(&self) -> Self {
        Self {
            target: self.target.clone(),
            wheres: self.wheres.clone(),
            orders: self.orders.clone(),
            limit: self.limit,
            offset: self.offset,
            includes: self.includes.clone(),
            with_trashed: self.with_trashed,
            only_trashed: self.only_trashed,
            model: PhantomData,
        }
    }
}

impl<M: Model> Query<M> {
    /// Starts a query on `target`. Prefer [`Model::query`].
    pub(crate) fn new(target: Target) -> Self {
        Self {
            target,
            wheres: Vec::new(),
            orders: Vec::new(),
            limit: None,
            offset: None,
            includes: Vec::new(),
            with_trashed: false,
            only_trashed: false,
            model: PhantomData,
        }
    }

    /// Retargets this query onto a transaction (replaces any previous target).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// let users = User::query(&db).on(&tx).get().await?;
    /// tx.commit().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn on(mut self, tx: &crate::Transaction) -> Self {
        self.target = Target::from(tx);
        self
    }

    /// Adds `column = value`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub role: String,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let admins = User::query(&db).where_eq("role", "admin").get().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn where_eq(mut self, column: &str, value: impl EncodeField) -> Self {
        self.wheres
            .push(Where::Eq(column.to_string(), value.encode_field()));
        self
    }

    /// Adds `column LIKE pattern` (`%` wildcards, as written).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub name: String,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, binds) = User::query(&db).where_like("name", "A%").to_sql()?;
    /// assert!(sql.contains("LIKE"), "{sql}");
    /// assert_eq!(binds.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn where_like(mut self, column: &str, pattern: &str) -> Self {
        self.wheres
            .push(Where::Like(column.to_string(), pattern.to_string()));
        self
    }

    /// Adds `column IN (...)`. An empty set renders `0 = 1` (matches nothing),
    /// mirroring Eloquent — never invalid `IN ()` SQL.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, binds) = User::query(&db).where_in("id", [1i64, 2]).to_sql()?;
    /// assert!(sql.contains("IN (?, ?)"), "{sql}");
    /// assert_eq!(binds.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn where_in<I, V>(mut self, column: &str, values: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: EncodeField,
    {
        let binds = values
            .into_iter()
            .map(|value| value.encode_field())
            .collect();
        self.wheres.push(Where::In(column.to_string(), binds));
        self
    }

    /// Adds `column IS NULL`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub nickname: Option<String>,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).where_null("nickname").to_sql()?;
    /// assert!(sql.contains("IS NULL"), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn where_null(mut self, column: &str) -> Self {
        self.wheres.push(Where::IsNull(column.to_string()));
        self
    }

    /// Adds `column IS NOT NULL`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub nickname: Option<String>,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).where_not_null("nickname").to_sql()?;
    /// assert!(sql.contains("IS NOT NULL"), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn where_not_null(mut self, column: &str) -> Self {
        self.wheres.push(Where::IsNotNull(column.to_string()));
        self
    }

    /// Appends `ORDER BY column ASC`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).order_by("id").to_sql()?;
    /// assert!(sql.contains(r#"ORDER BY "id" ASC"#), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn order_by(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), Order::Asc));
        self
    }

    /// Appends `ORDER BY column DESC`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).order_by_desc("id").to_sql()?;
    /// assert!(sql.contains(r#"ORDER BY "id" DESC"#), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn order_by_desc(mut self, column: &str) -> Self {
        self.orders.push((column.to_string(), Order::Desc));
        self
    }

    /// Sets `LIMIT n`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).limit(10).to_sql()?;
    /// assert!(sql.contains("LIMIT 10"), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn limit(mut self, count: u64) -> Self {
        self.limit = Some(count);
        self
    }

    /// Sets `OFFSET n`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).limit(10).offset(20).to_sql()?;
    /// assert!(sql.contains("OFFSET 20"), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn offset(mut self, count: u64) -> Self {
        self.offset = Some(count);
        self
    }

    /// Eager-loads relations by field name, dot-nested allowed
    /// (`["posts", "posts.comments", "team"]`). Unknown names fail with
    /// [`Error::RelationNotFound`] listing the valid ones; loading runs
    /// one batched query per relation level (no N+1).
    ///
    /// Accepts slices, arrays, and vectors of strings alike.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{BelongsTo, HasMany, Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "posts")]
    /// # pub struct Post {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub user_id: i64,
    /// # }
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     #[model(has_many = "Post")]
    /// #     pub posts: HasMany<Post>,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let users = User::query(&db).with(["posts"]).get().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with<I>(mut self, includes: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.includes
            .extend(includes.into_iter().map(|item| item.as_ref().to_string()));
        self
    }

    /// Disables the soft-delete scope (trashed rows included). A no-op for
    /// models without soft deletes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users", soft_deletes = true)]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// # }
    /// let db = DB::memory().await?;
    /// let (scoped, _) = User::query(&db).to_sql()?;
    /// assert!(scoped.contains("IS NULL"), "{scoped}");
    /// let (unscoped, _) = User::query(&db).with_trashed().to_sql()?;
    /// assert!(!unscoped.contains("IS NULL"), "{unscoped}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_trashed(mut self) -> Self {
        self.with_trashed = true;
        self
    }

    /// Inverts the soft-delete scope (only trashed rows). An explicit error
    /// for models without soft deletes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users", soft_deletes = true)]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// # }
    /// let db = DB::memory().await?;
    /// let (sql, _) = User::query(&db).only_trashed().to_sql()?;
    /// assert!(sql.contains("IS NOT NULL"), "{sql}");
    /// # Ok(())
    /// # }
    /// ```
    pub fn only_trashed(mut self) -> Self {
        self.only_trashed = true;
        self
    }

    /// Fetches all matching rows, then eager-loads [`with`](Query::with) includes.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let users: Vec<User> = User::query(&db).get().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get(self) -> Result<Vec<M>> {
        for include in &self.includes {
            if include.is_empty() || include.split('.').any(str::is_empty) {
                return Err(Error::invalid_query(format!(
                    "malformed include {include:?}"
                )));
            }
        }
        let (sql, binds) = self.render_select()?;
        let rows = self.target.fetch_all(&sql, binds).await?;
        let mut models = Vec::with_capacity(rows.len());
        for row in &rows {
            models.push(M::from_row(row)?);
        }
        for include in &self.includes {
            let segments: Vec<String> = include.split('.').map(str::to_string).collect();
            M::load_relation(&self.target, &mut models, &segments).await?;
        }
        Ok(models)
    }

    /// Fetches the first matching row, or `None`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let user: Option<User> = User::query(&db).order_by("id").first().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn first(mut self) -> Result<Option<M>> {
        self.limit = Some(1);
        self.get().await.map(|mut models| models.pop())
    }

    /// Fetches the first matching row, or [`Error::NotFound`].
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let user = User::query(&db).where_eq("id", 1i64).first_or_fail().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn first_or_fail(self) -> Result<M> {
        self.first()
            .await?
            .ok_or_else(|| Error::not_found(format!("{} matching query", M::table())))
    }

    /// Counts matching rows (wheres and soft-delete scope apply; ordering,
    /// paging, and includes do not).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub active: bool,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let total = User::query(&db).where_eq("active", true).count().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn count(&self) -> Result<i64> {
        let (filter, binds) = self.render_filter()?;
        let dialect = self.target.dialect();
        let sql = format!(
            "SELECT COUNT(*) AS aggregate FROM {}{filter}",
            dialect.quote_ident(M::table())
        );
        let row = self
            .target
            .fetch_optional(&sql, binds)
            .await?
            .ok_or_else(|| Error::not_found("count aggregate"))?;
        i64::decode_field(&row, "aggregate")
    }

    /// Averages a column (`None` when no rows match). The average is cast
    /// to a double on Postgres/MySQL (whose native average types the `Any`
    /// driver cannot return).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub age: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let average: Option<f64> = User::query(&db).avg("age").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn avg(&self, column: &str) -> Result<Option<f64>> {
        crate::validate_identifier(column)?;
        let (filter, binds) = self.render_filter()?;
        let dialect = self.target.dialect();
        let quoted = dialect.quote_ident(column);
        let average = match dialect {
            Dialect::Postgres => format!("CAST(AVG({quoted}) AS DOUBLE PRECISION)"),
            Dialect::MySQL => format!("CAST(AVG({quoted}) AS DOUBLE)"),
            Dialect::SQLite => format!("AVG({quoted})"),
        };
        let sql = format!(
            "SELECT {average} AS aggregate FROM {}{filter}",
            dialect.quote_ident(M::table())
        );
        let row = self
            .target
            .fetch_optional(&sql, binds)
            .await?
            .ok_or_else(|| Error::not_found("avg aggregate"))?;
        Option::<f64>::decode_field(&row, "aggregate")
    }

    /// Whether any row matches (efficient `LIMIT 1` probe).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub email: String,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let taken = User::query(&db).where_eq("email", "a@b.c").exists().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn exists(&self) -> Result<bool> {
        let (filter, binds) = self.render_filter()?;
        let dialect = self.target.dialect();
        let sql = format!(
            "SELECT 1 AS probe FROM {}{filter} LIMIT 1",
            dialect.quote_ident(M::table())
        );
        Ok(self.target.fetch_optional(&sql, binds).await?.is_some())
    }

    /// Paginates: page `page` (1-based) of `per_page` rows, with an exact
    /// total. Both arguments must be at least 1.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let page = User::query(&db).order_by("id").paginate(20, 1).await?;
    /// assert_eq!(page.per_page, 20);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn paginate(self, per_page: u64, page: u64) -> Result<Page<M>> {
        if per_page < 1 {
            return Err(Error::invalid_query("per_page must be at least 1"));
        }
        if page < 1 {
            return Err(Error::invalid_query("page must be at least 1"));
        }
        // `COUNT(*)` never fails here without failing the fetch below too,
        // and never returns negative — but neither fact is trusted blindly.
        let total = self.count().await?.max(0) as u64;
        let mut items = self.clone();
        items.limit = Some(per_page);
        items.offset = Some((page - 1).saturating_mul(per_page));
        let items = items.get().await?;
        let last_page = total.div_ceil(per_page);
        Ok(Page {
            items,
            total,
            page,
            per_page,
            last_page,
            has_more: page < last_page,
        })
    }

    /// Streams the result set in `size`-row batches, calling `f` per batch.
    /// Offset-based; `size` must be at least 1.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// User::query(&db).order_by("id").chunk(1000, |users| async move {
    ///     println!("batch of {}", users.len());
    ///     Ok(())
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn chunk<F, Fut>(self, size: u64, mut f: F) -> Result<()>
    where
        F: FnMut(Vec<M>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if size < 1 {
            return Err(Error::invalid_query("chunk size must be at least 1"));
        }
        let mut done = 0u64;
        loop {
            let mut batch = self.clone();
            batch.limit = Some(size);
            batch.offset = Some(done);
            let models = batch.get().await?;
            if models.is_empty() {
                return Ok(());
            }
            let fetched = models.len() as u64;
            f(models).await?;
            done += fetched;
            if fetched < size {
                return Ok(());
            }
        }
    }

    /// Renders the full `SELECT` (columns, filter, order, paging).
    ///
    /// This is Eloquent's `toSql()` + bindings in one call: the SQL string
    /// plus the values its placeholders consume, in order. Useful for
    /// debugging and for asserting generated SQL per dialect without a live
    /// server (see [`DB::connect_lazy`](crate::DB::connect_lazy)).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub name: String,
    /// # }
    ///
    /// let db = DB::memory().await?;
    /// let (sql, binds) = User::query(&db).where_eq("name", "Ada").to_sql()?;
    /// assert!(sql.contains("WHERE"));
    /// assert_eq!(binds.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub fn to_sql(&self) -> Result<(String, Vec<BindValue>)> {
        self.render_select()
    }

    /// Renders the full `SELECT` (columns, filter, order, paging).
    fn render_select(&self) -> Result<(String, Vec<BindValue>)> {
        let dialect = self.target.dialect();
        let (filter, binds) = self.render_filter()?;
        let mut sql = format!(
            "SELECT {} FROM {}{filter}",
            M::select_list(dialect),
            dialect.quote_ident(M::table())
        );
        if !self.orders.is_empty() {
            let mut parts = Vec::with_capacity(self.orders.len());
            for (column, order) in &self.orders {
                crate::validate_identifier(column)?;
                let direction = match order {
                    Order::Asc => "ASC",
                    Order::Desc => "DESC",
                };
                parts.push(format!("{} {direction}", dialect.quote_ident(column)));
            }
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }
        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        Ok((sql, binds))
    }

    /// Renders the shared `WHERE` filter (conditions + soft-delete scope),
    /// used by selects and every aggregate.
    fn render_filter(&self) -> Result<(String, Vec<BindValue>)> {
        let dialect = self.target.dialect();
        let mut parts: Vec<String> = Vec::new();
        let mut binds = Vec::new();
        for entry in &self.wheres {
            match entry {
                Where::Eq(column, value) => {
                    crate::validate_identifier(column)?;
                    let mut fragment = format!("{} = ", dialect.quote_ident(column));
                    dialect.push_bind(&mut fragment, &mut binds, value.clone());
                    parts.push(fragment);
                }
                Where::Like(column, pattern) => {
                    crate::validate_identifier(column)?;
                    let mut fragment = format!("{} LIKE ", dialect.quote_ident(column));
                    dialect.push_bind(&mut fragment, &mut binds, BindValue::Text(pattern.clone()));
                    parts.push(fragment);
                }
                Where::In(column, values) => {
                    crate::validate_identifier(column)?;
                    if values.is_empty() {
                        parts.push("0 = 1".to_string());
                    } else {
                        let mut fragment = format!("{} IN (", dialect.quote_ident(column));
                        let mut first = true;
                        for value in values {
                            if !first {
                                fragment.push_str(", ");
                            }
                            first = false;
                            dialect.push_bind(&mut fragment, &mut binds, value.clone());
                        }
                        fragment.push(')');
                        parts.push(fragment);
                    }
                }
                Where::IsNull(column) => {
                    crate::validate_identifier(column)?;
                    parts.push(format!("{} IS NULL", dialect.quote_ident(column)));
                }
                Where::IsNotNull(column) => {
                    crate::validate_identifier(column)?;
                    parts.push(format!("{} IS NOT NULL", dialect.quote_ident(column)));
                }
            }
        }
        match (M::soft_deletes(), M::deleted_at_column()) {
            (true, Some(column)) => {
                if self.only_trashed {
                    parts.push(format!("{} IS NOT NULL", dialect.quote_ident(column)));
                } else if !self.with_trashed {
                    parts.push(format!("{} IS NULL", dialect.quote_ident(column)));
                }
            }
            _ => {
                if self.only_trashed {
                    return Err(Error::invalid_query(format!(
                        "model `{}` does not use soft deletes",
                        M::table()
                    )));
                }
            }
        }
        if parts.is_empty() {
            Ok((String::new(), binds))
        } else {
            Ok((format!(" WHERE {}", parts.join(" AND ")), binds))
        }
    }
}
