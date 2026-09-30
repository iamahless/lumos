//! Test databases: migrate once, reset between tests.
//!
//! [`TestDb`] wraps a [`DB`](rusticate::DB) with the setup/teardown cycle
//! every suite repeats: run migrations up front, then empty tables (and
//! restart sqlite ids) between tests so each case starts identical.

use rusticate::{Dialect, Migration, Migrator, Seeder, DB};

/// A migrated test database with reset support.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_testing::TestDb;
/// use rusticate::{async_trait, Migration, Result, Schema};
///
/// struct CreateUsers;
/// #[async_trait]
/// impl Migration for CreateUsers {
///     async fn up(&self, schema: &mut Schema) -> Result<()> {
///         schema.create("users", |t| { t.id(); t.string("name"); }).await
///     }
///     async fn down(&self, schema: &mut Schema) -> Result<()> {
///         schema.drop("users").await
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = TestDb::memory(&[&CreateUsers]).await?;
/// db.reset(&["users"]).await?;
/// # Ok(())
/// # }
/// ```
pub struct TestDb {
    db: DB,
}

impl TestDb {
    /// Opens an in-memory database and runs `migrations`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestDb;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = TestDb::memory(&[]).await?;
    /// assert_eq!(db.db().dialect(), rusticate::Dialect::SQLite);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn memory(migrations: &[&dyn Migration]) -> rusticate::Result<Self> {
        let db = DB::memory().await?;
        Migrator::new(&db).run(migrations).await?;
        Ok(Self { db })
    }

    /// Opens `url` (e.g. a temp-file sqlite URL) and runs `migrations`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestDb;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let path = std::env::temp_dir().join("lumos-testing-doctest.db");
    /// let db = TestDb::open(&format!("sqlite:{}?mode=rwc", path.display()), &[]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn open(url: &str, migrations: &[&dyn Migration]) -> rusticate::Result<Self> {
        let db = DB::connect(url).await?;
        Migrator::new(&db).run(migrations).await?;
        Ok(Self { db })
    }

    /// Borrows the underlying handle for queries and app wiring.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestDb;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = TestDb::memory(&[]).await?;
    /// let _pool = db.db().pool();
    /// # Ok(())
    /// # }
    /// ```
    pub fn db(&self) -> &DB {
        &self.db
    }

    /// Runs each seeder in order.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestDb;
    /// use rusticate::{async_trait, Result, Seeder, DB};
    ///
    /// struct EmptySeeder;
    /// #[async_trait]
    /// impl Seeder for EmptySeeder {
    ///     async fn run(&self, _db: &DB) -> Result<()> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = TestDb::memory(&[]).await?;
    /// db.seed(&[&EmptySeeder]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn seed(&self, seeders: &[&dyn Seeder]) -> rusticate::Result<()> {
        for seeder in seeders {
            seeder.run(&self.db).await?;
        }
        Ok(())
    }

    /// Empties `tables`, given children-first (foreign keys are enforced,
    /// so parents listed before their children fail loudly). On SQLite
    /// the autoincrement counters restart too, keeping ids deterministic
    /// (`1, 2, …` after every reset); other dialects just delete rows.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestDb;
    /// use rusticate::{async_trait, Changeset, Migration, Model, Result, Schema, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub name: String,
    /// # }
    /// # struct CreateUsers;
    /// # #[async_trait]
    /// # impl Migration for CreateUsers {
    /// #     async fn up(&self, schema: &mut Schema) -> Result<()> {
    /// #         schema.create("users", |t| { t.id(); t.string("name"); }).await
    /// #     }
    /// #     async fn down(&self, schema: &mut Schema) -> Result<()> {
    /// #         schema.drop("users").await
    /// #     }
    /// # }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = TestDb::memory(&[&CreateUsers]).await?;
    /// let first = User::create(db.db(), Changeset::new().set("name", "ada")).await?;
    /// assert_eq!(first.id, 1);
    /// db.reset(&["users"]).await?;
    /// let again = User::create(db.db(), Changeset::new().set("name", "ada")).await?;
    /// assert_eq!(again.id, 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reset(&self, tables: &[&str]) -> rusticate::Result<()> {
        let dialect = self.db.dialect();
        for table in tables {
            let sql = format!("DELETE FROM {}", dialect.quote_ident(table));
            self.db.raw(&sql).execute().await?;
        }
        if dialect == Dialect::SQLite {
            for table in tables {
                // `sqlite_sequence` only tracks AUTOINCREMENT tables; the
                // delete is a silent no-op for plain rowid tables.
                let sql = format!(
                    "DELETE FROM sqlite_sequence WHERE name = {}",
                    quote_literal(table)
                );
                self.db.raw(&sql).execute().await?;
            }
        }
        Ok(())
    }
}

/// Quotes a string literal for every dialect (doubled single quotes).
fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use rusticate::{async_trait, Changeset, Model, Result, Schema};

    use super::*;

    #[derive(Model)]
    #[model(table = "reset_users")]
    pub struct ResetUser {
        #[model(id, auto_increment)]
        pub id: i64,
        pub name: String,
    }

    #[derive(Model)]
    #[model(table = "reset_posts")]
    pub struct ResetPost {
        #[model(id, auto_increment)]
        pub id: i64,
        pub user_id: i64,
    }

    struct ResetSchema;

    #[async_trait]
    impl Migration for ResetSchema {
        async fn up(&self, schema: &mut Schema) -> Result<()> {
            schema
                .create("reset_users", |t| {
                    t.id();
                    t.string("name");
                })
                .await?;
            schema
                .create("reset_posts", |t| {
                    t.id();
                    t.foreign_id("user_id").references("id").on("reset_users");
                })
                .await
        }

        async fn down(&self, schema: &mut Schema) -> Result<()> {
            schema.drop("reset_posts").await?;
            schema.drop("reset_users").await
        }
    }

    #[tokio::test]
    async fn reset_empties_children_first_and_restarts_ids() -> rusticate::Result<()> {
        let db = TestDb::memory(&[&ResetSchema]).await?;
        let user = ResetUser::create(db.db(), Changeset::new().set("name", "ada")).await?;
        ResetPost::create(db.db(), Changeset::new().set("user_id", user.id)).await?;

        db.reset(&["reset_posts", "reset_users"]).await?;
        assert_eq!(ResetUser::query(db.db()).count().await?, 0);
        assert_eq!(ResetPost::query(db.db()).count().await?, 0);

        let again = ResetUser::create(db.db(), Changeset::new().set("name", "ada")).await?;
        assert_eq!(again.id, 1);
        Ok(())
    }

    #[tokio::test]
    async fn reset_parents_first_fails_on_foreign_keys() -> rusticate::Result<()> {
        let db = TestDb::memory(&[&ResetSchema]).await?;
        let user = ResetUser::create(db.db(), Changeset::new().set("name", "ada")).await?;
        ResetPost::create(db.db(), Changeset::new().set("user_id", user.id)).await?;

        assert!(db.reset(&["reset_users", "reset_posts"]).await.is_err());
        Ok(())
    }
}
