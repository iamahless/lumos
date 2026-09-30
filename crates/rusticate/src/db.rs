//! Connections, dialects, execution targets, transactions, raw queries.
//!
//! Rusticate runs on sqlx's `Any` driver: one implementation serves SQLite,
//! Postgres, and MySQL, with the backend chosen at runtime from the
//! connection URL. [`Dialect`] centralizes every backend difference the ORM
//! must know: identifier quoting, placeholder syntax (`$N` vs `?` — the `Any`
//! driver does not translate these), and `CAST` wrappers that let timestamp
//! and JSON values bind as portable strings.
//!
//! Query execution always goes through a [`Target`]: either a [`DB`] pool or
//! a [`Transaction`]. Builders own their target (cheap `Arc` clones, no
//! lifetimes), and [`.on()`](crate::Query::on) overrides it —
//! `User::query(&db).on(&tx)` runs inside the transaction.

use std::sync::{Arc, RwLock};

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::any::{Any, AnyPoolOptions, AnyRow};
use sqlx::Pool;

use crate::observers::ObserverRegistry;
use crate::value::{BindValue, EncodeField};
use crate::{Error, Result};

/// Backend dialect, detected from the connection URL.
///
/// Every SQL string Rusticate generates goes through here, so Postgres,
/// MySQL, and SQLite stay correct from one code path. Anything dialect-free
/// (relation `WHERE fk IN (...)` queries, for example) never consults it.
///
/// # Examples
///
/// ```rust
/// use rusticate::Dialect;
///
/// assert_eq!(Dialect::from_url("sqlite::memory:").unwrap(), Dialect::SQLite);
/// assert_eq!(Dialect::from_url("postgres://u@h/db").unwrap(), Dialect::Postgres);
/// assert_eq!(Dialect::from_url("mysql://u@h/db").unwrap(), Dialect::MySQL);
/// assert!(Dialect::from_url("oracle://x").is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// SQLite (`sqlite:` URLs).
    SQLite,
    /// Postgres (`postgres://`, `postgresql://`).
    Postgres,
    /// MySQL / MariaDB (`mysql://`, `mariadb://`).
    MySQL,
}

impl Dialect {
    /// Detects the dialect from a connection URL.
    ///
    /// Unknown schemes are [`Error::Config`] naming the supported ones —
    /// never a silent default.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Dialect;
    ///
    /// assert_eq!(Dialect::from_url("sqlite:app.db").unwrap(), Dialect::SQLite);
    /// assert!(Dialect::from_url("oracle://x").is_err());
    /// ```
    pub fn from_url(url: &str) -> Result<Self> {
        if url.starts_with("sqlite:") {
            Ok(Self::SQLite)
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres)
        } else if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            Ok(Self::MySQL)
        } else {
            Err(Error::Config(format!(
                "unsupported database URL scheme in {url:?}: expected sqlite:, postgres://, or mysql://"
            )))
        }
    }

    /// Quotes an identifier: `"name"` (SQLite/Postgres) or `` `name` `` (MySQL).
    ///
    /// Embedded quote characters are doubled (defense in depth — validated
    /// identifiers cannot contain them in the first place).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Dialect;
    ///
    /// assert_eq!(Dialect::Postgres.quote_ident("email"), "\"email\"");
    /// assert_eq!(Dialect::MySQL.quote_ident("email"), "`email`");
    /// ```
    pub fn quote_ident(&self, name: &str) -> String {
        match self {
            Self::MySQL => format!("`{}`", name.replace('`', "``")),
            Self::SQLite | Self::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// Renders the bind placeholder for 1-based `index`: `$N` on Postgres,
    /// `?` on MySQL/SQLite.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Dialect;
    ///
    /// assert_eq!(Dialect::Postgres.placeholder(2), "$2");
    /// assert_eq!(Dialect::SQLite.placeholder(2), "?");
    /// ```
    pub fn placeholder(&self, index: usize) -> String {
        match self {
            Self::Postgres => format!("${index}"),
            Self::SQLite | Self::MySQL => "?".to_string(),
        }
    }

    /// Formats a timestamp for binding: RFC 3339 with microseconds, except
    /// MySQL, which gets naive `DATETIME(6)` layout (no zone designator —
    /// `DATETIME` carries none; values are UTC by convention).
    pub(crate) fn time_literal(&self, moment: &DateTime<Utc>) -> String {
        match self {
            Self::MySQL => moment.format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
            Self::SQLite | Self::Postgres => moment.to_rfc3339_opts(SecondsFormat::Micros, true),
        }
    }

    /// Wraps a timestamp placeholder so the bound string parses server-side.
    /// Plain on SQLite (ISO text compares and stores directly).
    fn wrap_time(&self, placeholder: &str) -> String {
        match self {
            Self::Postgres => format!("CAST({placeholder} AS TIMESTAMPTZ)"),
            Self::MySQL => format!("CAST({placeholder} AS DATETIME(6))"),
            Self::SQLite => placeholder.to_string(),
        }
    }

    /// Wraps a JSON placeholder so the bound string parses server-side.
    /// Plain on SQLite (JSON lives in `TEXT` columns).
    fn wrap_json(&self, placeholder: &str) -> String {
        match self {
            Self::Postgres => format!("CAST({placeholder} AS JSONB)"),
            Self::MySQL => format!("CAST({placeholder} AS JSON)"),
            Self::SQLite => placeholder.to_string(),
        }
    }

    /// Appends one value's SQL fragment, pushing its bind (if any).
    ///
    /// `NULL` inlines as the keyword; timestamps and JSON convert to text
    /// under a dialect `CAST`; everything else binds with its exact type.
    /// Placeholder indexes derive from the bind count, so numbering stays
    /// correct however fragments interleave.
    pub(crate) fn push_bind(&self, sql: &mut String, binds: &mut Vec<BindValue>, value: BindValue) {
        match value {
            BindValue::Null => sql.push_str("NULL"),
            BindValue::Time(moment) => {
                let fragment = self.wrap_time(&self.placeholder(binds.len() + 1));
                sql.push_str(&fragment);
                binds.push(BindValue::Text(self.time_literal(&moment)));
            }
            BindValue::Json(text) => {
                let fragment = self.wrap_json(&self.placeholder(binds.len() + 1));
                sql.push_str(&fragment);
                binds.push(BindValue::Text(text));
            }
            other => {
                let fragment = self.placeholder(binds.len() + 1);
                sql.push_str(&fragment);
                binds.push(other);
            }
        }
    }
}

/// Shared handle to one transaction's underlying sqlx transaction.
///
/// Cloned into every query running on the transaction; the async mutex
/// serializes queries (a connection executes one at a time anyway) and the
/// `Option` drives commit/rollback exactly once. Rusticate never holds this
/// lock across user code (observer hooks, chunk callbacks), so observers
/// querying on the same transaction cannot deadlock.
#[derive(Clone)]
pub(crate) struct TxConn {
    slot: Arc<tokio::sync::Mutex<Option<TxSlot>>>,
}

impl std::fmt::Debug for TxConn {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let finished = self
            .slot
            .try_lock()
            .map(|guard| guard.as_ref().map(|slot| slot.tx.is_none()).unwrap_or(true))
            .unwrap_or(false);
        formatter
            .debug_struct("TxConn")
            .field("finished", &finished)
            .finish()
    }
}

/// One open transaction: the sqlx transaction plus the pool clone backing it.
///
/// `sqlx::Transaction` borrows the pool it began from, so an owned handle
/// needs a pool reference that outlives the `begin()` call. `pool` is a
/// leaked `Box<AnyPool>` clone backing the `'static` borrow; it is reclaimed
/// exactly once — when the slot is taken on commit/rollback (via [`Drop`]),
/// or when the last handle to an abandoned transaction drops.
pub(crate) struct TxSlot {
    tx: Option<sqlx::Transaction<'static, Any>>,
    pool: *mut Pool<Any>,
}

// SAFETY: the pool pointer is only dereferenced in `DB::begin` (before the
// slot is shared) and in `Drop` through `&mut self` (exclusive). Every
// cross-thread share goes through the mutex, so sending the slot across
// threads cannot alias the pointee.
unsafe impl Send for TxSlot {}

impl Drop for TxSlot {
    fn drop(&mut self) {
        if self.pool.is_null() {
            return;
        }
        // Drop the sqlx transaction first: an open transaction rolls back on
        // drop (sqlx semantics), which must happen before its pool goes away.
        self.tx = None;
        // SAFETY: `pool` came from `Box::into_raw` in `DB::begin`, is only
        // touched here, and is nulled immediately after reclaiming, so this
        // `from_raw` runs exactly once per leaked box.
        unsafe {
            drop(Box::from_raw(self.pool));
        }
        self.pool = std::ptr::null_mut();
    }
}

/// Database handle: pool + dialect + observer registry.
///
/// Cheap to clone (all shared state is behind `Arc`s); the idiomatic shape
/// is one `DB` built at boot, stored in the service container, and injected
/// into services. There is deliberately no global instance.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::DB;
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::connect("sqlite:app.db").await?;
/// assert_eq!(db.dialect(), rusticate::Dialect::SQLite);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct DB {
    pool: Pool<Any>,
    dialect: Dialect,
    observers: Arc<RwLock<ObserverRegistry>>,
}

impl DB {
    /// Connects to the database at `url` (SQLite, Postgres, or MySQL).
    ///
    /// Installs sqlx's compiled-in `Any` drivers idempotently, detects the
    /// dialect, and opens a 5-connection pool.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::DB;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::connect("sqlite:app.db").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let dialect = Dialect::from_url(url)?;
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await
            .map_err(|error| Error::Config(format!("cannot connect to database: {error}")))?;
        Ok(Self::custom(pool, dialect))
    }

    /// Opens an isolated in-memory SQLite database (single connection, so
    /// the `:memory:` store is never split across pooled connections).
    ///
    /// Intended for tests, doctests, and throwaway scripts — never production.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)").execute().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn memory() -> Result<Self> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .map_err(|error| Error::Config(format!("cannot open in-memory database: {error}")))?;
        Ok(Self::custom(pool, Dialect::SQLite))
    }

    /// Opens a lazily-connecting handle: no I/O happens until the first
    /// query. Useful for building (and asserting) SQL offline, and for
    /// processes that may never touch the database.
    ///
    /// Must be called within a Tokio runtime context (pool registration
    /// needs one), even though no I/O happens yet.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() {
    /// use rusticate::{Dialect, DB};
    ///
    /// let db = DB::connect_lazy("postgres://u@h/d").unwrap();
    /// assert_eq!(db.dialect(), Dialect::Postgres);
    /// assert_eq!(db.placeholder(2), "$2");
    /// # }
    /// ```
    pub fn connect_lazy(url: &str) -> Result<Self> {
        sqlx::any::install_default_drivers();
        let dialect = Dialect::from_url(url)?;
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect_lazy(url)
            .map_err(|error: sqlx::Error| {
                Error::Config(format!("cannot parse database URL: {error}"))
            })?;
        Ok(Self::custom(pool, dialect))
    }

    /// Wraps an existing pool with an explicit dialect.
    ///
    /// Escape hatch for custom pool options (timeouts, pool sizing, hooks):
    /// configure `AnyPoolOptions` yourself, then hand the pool over.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Dialect, DB};
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let pool = sqlx::any::AnyPoolOptions::new()
    ///     .max_connections(2)
    ///     .connect("sqlite:app.db")
    ///     .await
    ///     .map_err(rusticate::Error::db)?;
    /// let db = DB::custom(pool, Dialect::SQLite);
    /// # Ok(())
    /// # }
    /// ```
    pub fn custom(pool: Pool<Any>, dialect: Dialect) -> Self {
        Self {
            pool,
            dialect,
            observers: Arc::new(RwLock::new(ObserverRegistry::new())),
        }
    }

    /// Returns the backend dialect.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Dialect, DB};
    ///
    /// let db = DB::memory().await?;
    /// assert_eq!(db.dialect(), Dialect::SQLite);
    /// # Ok(())
    /// # }
    /// ```
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Returns the underlying pool.
    ///
    /// Full escape hatch: anything sqlx can do directly stays possible.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// assert!(!db.pool().is_closed());
    /// # Ok(())
    /// # }
    /// ```
    pub fn pool(&self) -> &Pool<Any> {
        &self.pool
    }

    /// Registers an observer for model `M` (see [`Observer`](crate::Observer)).
    ///
    /// Callable at any time; hooks fire for subsequent writes through this
    /// handle (and its clones, and its transactions).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{async_trait, Model, Observer, Result, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    ///
    /// pub struct AuditObserver;
    ///
    /// #[async_trait]
    /// impl Observer<User> for AuditObserver {
    ///     async fn created(
    ///         &self,
    ///         _target: &rusticate::Target,
    ///         _user: &User,
    ///     ) -> Result<()> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// db.observe(AuditObserver);
    /// # Ok(())
    /// # }
    /// ```
    pub fn observe<M, O>(&self, observer: O)
    where
        M: crate::Model,
        O: crate::Observer<M> + 'static,
    {
        if let Ok(mut registry) = self.observers.write() {
            registry.add::<M, O>(observer);
        }
    }

    /// Begins a transaction for manual commit/rollback.
    ///
    /// Prefer [`DB::transaction`] (automatic commit/rollback); use this when
    /// the transaction must span code that cannot fit one closure.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// tx.rollback().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn begin(&self) -> Result<Transaction> {
        // The sqlx transaction borrows the pool it began from; a leaked clone
        // backs that borrow (reclaimed on commit/rollback, or when the last
        // handle drops — see `TxSlot`).
        let pool = Box::into_raw(Box::new(self.pool.clone()));
        // SAFETY: `pool` was just leaked from a live box and is non-null.
        let begun = unsafe { &*pool }.begin().await;
        match begun {
            Ok(inner) => Ok(Transaction {
                db: self.clone(),
                conn: TxConn {
                    slot: Arc::new(tokio::sync::Mutex::new(Some(TxSlot {
                        tx: Some(inner),
                        pool,
                    }))),
                },
            }),
            Err(error) => {
                // SAFETY: same box as above; `begin` failed, so no slot owns it.
                unsafe {
                    drop(Box::from_raw(pool));
                }
                Err(Error::db(error))
            }
        }
    }

    /// Runs `run` inside a transaction: commit on `Ok`, rollback on `Err`.
    ///
    /// The closure receives an owned [`Transaction`] (cheap clone —
    /// transactions are reference-counted handles); queries join it via
    /// [`.on(&tx)`](crate::Query::on) or take `&tx`/`tx` anywhere a target
    /// is accepted. A rollback failure is logged; the original error is
    /// what the caller receives.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)").execute().await?;
    /// db.transaction(|tx| async move {
    ///     tx.raw("INSERT INTO t (id) VALUES (1)").execute().await?;
    ///     Ok::<(), rusticate::Error>(())
    /// })
    /// .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transaction<F, Fut, T>(&self, run: F) -> Result<T>
    where
        F: FnOnce(Transaction) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let tx = self.begin().await?;
        match run(tx.clone()).await {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback) = tx.rollback().await {
                    tracing::warn!(%rollback, "transaction rollback failed");
                }
                Err(error)
            }
        }
    }

    /// Starts a raw SQL query on this database (see [`RawQuery`]).
    ///
    /// Raw SQL must use the dialect's native placeholders; [`DB::placeholder`]
    /// renders them portably.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)").execute().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn raw(&self, sql: &str) -> RawQuery {
        RawQuery::new(Target::from(self), sql)
    }

    /// Renders the bind placeholder for 1-based `index` in this handle's
    /// dialect — the portable way to write [`RawQuery`] SQL by hand.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?; // SQLite: `?`
    /// assert_eq!(db.placeholder(1), "?");
    /// # Ok(())
    /// # }
    /// ```
    pub fn placeholder(&self, index: usize) -> String {
        self.dialect.placeholder(index)
    }

    /// Returns the observer registry for hook dispatch.
    pub(crate) fn observers(&self) -> Arc<RwLock<ObserverRegistry>> {
        Arc::clone(&self.observers)
    }
}

impl std::fmt::Debug for DB {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DB")
            .field("dialect", &self.dialect)
            .field("observers", &self.observers)
            .finish_non_exhaustive()
    }
}

/// An open transaction: dialect, database handle, and one serialized connection.
///
/// Obtained from [`DB::begin`] (manual) or [`DB::transaction`] (automatic).
/// Cheap to clone — clones share the transaction; queries serialize on its
/// connection, exactly as the database requires.
///
/// Dropping an open transaction rolls it back (sqlx behavior); prefer
/// explicit [`commit`](Transaction::commit) / [`rollback`](Transaction::rollback).
///
/// # Examples
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// use rusticate::DB;
///
/// let db = DB::memory().await?;
/// let tx = db.begin().await?;
/// tx.commit().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Transaction {
    db: DB,
    conn: TxConn,
}

impl Transaction {
    /// Commits the transaction. Exactly once — a second call (or any later
    /// query) fails with [`Error::TransactionFinished`].
    ///
    /// Takes `&self`: interior state drives the once-only guarantee, so
    /// committing through a shared reference (as [`DB::transaction`] does)
    /// is safe.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// tx.commit().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn commit(&self) -> Result<()> {
        let mut slot = self.take_slot().await?;
        let inner = slot.tx.take().ok_or(Error::TransactionFinished)?;
        let outcome = inner.commit().await.map_err(Error::db);
        drop(slot);
        outcome
    }

    /// Rolls the transaction back. Exactly-once semantics mirror [`commit`](Transaction::commit).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// tx.rollback().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rollback(&self) -> Result<()> {
        let mut slot = self.take_slot().await?;
        let inner = slot.tx.take().ok_or(Error::TransactionFinished)?;
        let outcome = inner.rollback().await.map_err(Error::db);
        drop(slot);
        outcome
    }

    /// Returns `true` once committed or rolled back (non-blocking best
    /// effort: `false` while a query is in flight).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// assert!(!tx.is_finished());
    /// tx.rollback().await?;
    /// assert!(tx.is_finished());
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_finished(&self) -> bool {
        self.conn
            .slot
            .try_lock()
            .map(|guard| guard.as_ref().map(|slot| slot.tx.is_none()).unwrap_or(true))
            .unwrap_or(false)
    }

    /// Returns the parent database handle (dialect, observers, pool).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Dialect, DB};
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// assert_eq!(tx.db().dialect(), Dialect::SQLite);
    /// tx.rollback().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn db(&self) -> &DB {
        &self.db
    }

    /// Starts a raw SQL query inside this transaction (see [`RawQuery`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)").execute().await?;
    /// let tx = db.begin().await?;
    /// tx.raw("INSERT INTO t (id) VALUES (1)").execute().await?;
    /// tx.commit().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn raw(&self, sql: &str) -> RawQuery {
        RawQuery::new(Target::from(self), sql)
    }

    /// Takes the slot out exactly once (leaving `None` behind, so every later
    /// commit, rollback, or query fails with [`Error::TransactionFinished`]).
    async fn take_slot(&self) -> Result<TxSlot> {
        let mut guard = self.conn.slot.lock().await;
        guard.take().ok_or(Error::TransactionFinished)
    }
}

impl std::fmt::Debug for Transaction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Transaction")
            .field("dialect", &self.db.dialect)
            .field("finished", &self.is_finished())
            .finish()
    }
}

/// Where a query executes: a [`DB`] pool or a [`Transaction`].
///
/// Every model, builder, relation, and raw entry point takes
/// `impl Into<Target>`, so `&db` and `&tx` work interchangeably —
/// `User::find(&db, 1)` and `User::find(&tx, 1)` are the same method.
/// Builders additionally offer [`.on(&tx)`](crate::Query::on) to retarget
/// after construction.
///
/// Cheap to clone; carries the database handle (dialect + observers) in
/// both cases.
///
/// # Examples
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// use rusticate::{Target, DB};
///
/// let db = DB::memory().await?;
/// let target = Target::from(&db);
/// assert!(!target.is_transaction());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Target {
    db: DB,
    tx: Option<TxConn>,
}

impl From<&DB> for Target {
    fn from(db: &DB) -> Self {
        Self {
            db: db.clone(),
            tx: None,
        }
    }
}

impl From<DB> for Target {
    fn from(db: DB) -> Self {
        Self { db, tx: None }
    }
}

impl From<&Transaction> for Target {
    fn from(tx: &Transaction) -> Self {
        Self {
            db: tx.db.clone(),
            tx: Some(tx.conn.clone()),
        }
    }
}

impl From<Transaction> for Target {
    fn from(tx: Transaction) -> Self {
        Self {
            db: tx.db.clone(),
            tx: Some(tx.conn.clone()),
        }
    }
}

impl From<&Target> for Target {
    /// Clones the target (observer hooks receive `&Target` and query
    /// through it with `Model::query(target)`).
    fn from(target: &Target) -> Self {
        target.clone()
    }
}

impl Target {
    /// Returns the database handle (dialect, observers, pool).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Target, DB};
    ///
    /// let db = DB::memory().await?;
    /// let target = Target::from(&db);
    /// assert!(!target.db().pool().is_closed());
    /// # Ok(())
    /// # }
    /// ```
    pub fn db(&self) -> &DB {
        &self.db
    }

    /// Returns the backend dialect.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Dialect, Target, DB};
    ///
    /// let db = DB::memory().await?;
    /// assert_eq!(Target::from(&db).dialect(), Dialect::SQLite);
    /// # Ok(())
    /// # }
    /// ```
    pub fn dialect(&self) -> Dialect {
        self.db.dialect
    }

    /// Returns `true` when executing inside a transaction.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Target, DB};
    ///
    /// let db = DB::memory().await?;
    /// let tx = db.begin().await?;
    /// assert!(Target::from(&tx).is_transaction());
    /// tx.rollback().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn is_transaction(&self) -> bool {
        self.tx.is_some()
    }

    /// Converts a bind value to its bound form for this target's dialect
    /// (timestamps and JSON become text; everything else is identical).
    /// Used where placeholders are user-written ([`RawQuery::bind`]).
    fn convert_bind(&self, value: BindValue) -> BindValue {
        match value {
            BindValue::Time(moment) => BindValue::Text(self.dialect().time_literal(&moment)),
            BindValue::Json(text) => BindValue::Text(text),
            other => other,
        }
    }

    /// Runs a `SELECT` returning all rows.
    pub(crate) async fn fetch_all(&self, sql: &str, binds: Vec<BindValue>) -> Result<Vec<AnyRow>> {
        match &self.tx {
            None => bind_and_fetch(&self.db.pool, sql, binds).await,
            Some(conn) => {
                let mut guard = conn.slot.lock().await;
                let slot = guard.as_mut().ok_or(Error::TransactionFinished)?;
                let tx = slot.tx.as_mut().ok_or(Error::TransactionFinished)?;
                let conn = &mut **tx;
                bind_and_fetch(conn, sql, binds).await
            }
        }
    }

    /// Runs a `SELECT` returning at most one row.
    pub(crate) async fn fetch_optional(
        &self,
        sql: &str,
        binds: Vec<BindValue>,
    ) -> Result<Option<AnyRow>> {
        match &self.tx {
            None => bind_and_fetch_optional(&self.db.pool, sql, binds).await,
            Some(conn) => {
                let mut guard = conn.slot.lock().await;
                let slot = guard.as_mut().ok_or(Error::TransactionFinished)?;
                let tx = slot.tx.as_mut().ok_or(Error::TransactionFinished)?;
                let conn = &mut **tx;
                bind_and_fetch_optional(conn, sql, binds).await
            }
        }
    }

    /// Runs an `INSERT`/`UPDATE`/`DELETE`/DDL statement, returning affected rows.
    pub(crate) async fn execute(&self, sql: &str, binds: Vec<BindValue>) -> Result<u64> {
        match &self.tx {
            None => bind_and_execute(&self.db.pool, sql, binds).await,
            Some(conn) => {
                let mut guard = conn.slot.lock().await;
                let slot = guard.as_mut().ok_or(Error::TransactionFinished)?;
                let tx = slot.tx.as_mut().ok_or(Error::TransactionFinished)?;
                let conn = &mut **tx;
                bind_and_execute(conn, sql, binds).await
            }
        }
    }
}

/// Binds every value with its concrete type (so drivers see exact types on
/// every backend) and fetches all rows. Generic over pool and connection:
/// both implement `Executor<Database = Any>`.
async fn bind_and_fetch<'e, E>(exec: E, sql: &str, binds: Vec<BindValue>) -> Result<Vec<AnyRow>>
where
    E: sqlx::Executor<'e, Database = Any>,
{
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = apply_bind(query, bind);
    }
    query.fetch_all(exec).await.map_err(Error::db)
}

async fn bind_and_fetch_optional<'e, E>(
    exec: E,
    sql: &str,
    binds: Vec<BindValue>,
) -> Result<Option<AnyRow>>
where
    E: sqlx::Executor<'e, Database = Any>,
{
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = apply_bind(query, bind);
    }
    query.fetch_optional(exec).await.map_err(Error::db)
}

async fn bind_and_execute<'e, E>(exec: E, sql: &str, binds: Vec<BindValue>) -> Result<u64>
where
    E: sqlx::Executor<'e, Database = Any>,
{
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = apply_bind(query, bind);
    }
    let done = query.execute(exec).await.map_err(Error::db)?;
    Ok(done.rows_affected())
}

/// Applies one bind with its concrete type. `Null`/`Time`/`Json` never reach
/// here from builders (inlined/converted at SQL-render time); the arms below
/// are safe defaults for hand-built bind lists.
fn apply_bind<'q>(
    query: sqlx::query::Query<'q, Any, sqlx::any::AnyArguments<'q>>,
    bind: BindValue,
) -> sqlx::query::Query<'q, Any, sqlx::any::AnyArguments<'q>> {
    match bind {
        BindValue::Null => query.bind(None::<String>),
        BindValue::Bool(value) => query.bind(value),
        BindValue::I64(value) => query.bind(value),
        BindValue::F64(value) => query.bind(value),
        BindValue::Text(value) => query.bind(value),
        BindValue::Blob(value) => query.bind(value),
        BindValue::Time(moment) => query.bind(moment.to_rfc3339_opts(SecondsFormat::Micros, true)),
        BindValue::Json(text) => query.bind(text),
    }
}

/// Hand-written SQL with bound values — the ORM's escape hatch.
///
/// Placeholders must be native to the dialect (`$N` on Postgres, `?`
/// elsewhere; [`DB::placeholder`] renders them portably). Values bind with
/// exact types; timestamps convert to the dialect's text form (add an
/// explicit `CAST(? AS TIMESTAMPTZ)` on Postgres/MySQL when comparing
/// against timestamp columns).
///
/// The `Any` driver returns only booleans, integers, floats, blobs, and
/// text: cast timestamps, UUIDs, and JSON to text in raw `SELECT`s on
/// Postgres/MySQL (the query builder does this automatically).
///
/// # Examples
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// use rusticate::DB;
///
/// let db = DB::memory().await?;
/// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)").execute().await?;
/// db.raw("INSERT INTO t (v) VALUES (?)").bind("hi").execute().await?;
/// let row = db.raw("SELECT v FROM t WHERE id = ?").bind(1i64).fetch_one().await?;
/// # Ok(())
/// # }
/// ```
pub struct RawQuery {
    target: Target,
    sql: String,
    binds: Vec<BindValue>,
}

impl RawQuery {
    fn new(target: Target, sql: &str) -> Self {
        Self {
            target,
            sql: sql.to_string(),
            binds: Vec::new(),
        }
    }

    /// Binds one value to the next placeholder, with its exact type.
    ///
    /// Accepts anything implementing [`EncodeField`] (primitives, `String`,
    /// timestamps, UUIDs, JSON, `Option`s). Timestamps convert to the
    /// dialect's text form at bind time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)").execute().await?;
    /// db.raw("INSERT INTO t (v) VALUES (?)").bind("hi").execute().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn bind(mut self, value: impl EncodeField) -> Self {
        self.binds
            .push(self.target.convert_bind(value.encode_field()));
        self
    }

    /// Runs the query, returning all rows.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)").execute().await?;
    /// let rows = db.raw("SELECT * FROM t").fetch_all().await?;
    /// assert!(rows.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_all(self) -> Result<Vec<AnyRow>> {
        self.target.fetch_all(&self.sql, self.binds).await
    }

    /// Runs the query, returning at most one row.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let row = db.raw("SELECT 1 AS one").fetch_optional().await?;
    /// assert!(row.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_optional(self) -> Result<Option<AnyRow>> {
        self.target.fetch_optional(&self.sql, self.binds).await
    }

    /// Runs the query, expecting exactly one row ([`Error::NotFound`] otherwise).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// let row = db.raw("SELECT 1 AS one").fetch_one().await?;
    /// let _ = row;
    /// assert!(db.raw("SELECT 1 AS one WHERE 1 = 0").fetch_one().await.is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_one(self) -> Result<AnyRow> {
        self.fetch_optional()
            .await?
            .ok_or_else(|| Error::not_found("row"))
    }

    /// Runs the statement, returning affected rows.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::DB;
    ///
    /// let db = DB::memory().await?;
    /// db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)").execute().await?;
    /// let affected = db.raw("INSERT INTO t (id) VALUES (1)").execute().await?;
    /// assert_eq!(affected, 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(self) -> Result<u64> {
        self.target.execute(&self.sql, self.binds).await
    }
}

impl std::fmt::Debug for RawQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RawQuery")
            .field("sql", &self.sql)
            .field("binds", &self.binds.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialects_detect_from_urls() {
        assert_eq!(
            Dialect::from_url("sqlite::memory:").unwrap(),
            Dialect::SQLite
        );
        assert_eq!(Dialect::from_url("sqlite:app.db").unwrap(), Dialect::SQLite);
        assert_eq!(
            Dialect::from_url("postgres://u@h/d").unwrap(),
            Dialect::Postgres
        );
        assert_eq!(
            Dialect::from_url("postgresql://u@h/d").unwrap(),
            Dialect::Postgres
        );
        assert_eq!(Dialect::from_url("mysql://u@h/d").unwrap(), Dialect::MySQL);
        assert_eq!(
            Dialect::from_url("mariadb://u@h/d").unwrap(),
            Dialect::MySQL
        );
        assert!(Dialect::from_url("oracle://x").is_err());
    }

    #[test]
    fn placeholders_are_native_per_dialect() {
        assert_eq!(Dialect::Postgres.placeholder(1), "$1");
        assert_eq!(Dialect::Postgres.placeholder(12), "$12");
        assert_eq!(Dialect::MySQL.placeholder(1), "?");
        assert_eq!(Dialect::SQLite.placeholder(3), "?");
    }

    #[test]
    fn identifiers_quote_per_dialect() {
        assert_eq!(Dialect::Postgres.quote_ident("email"), "\"email\"");
        assert_eq!(Dialect::SQLite.quote_ident("email"), "\"email\"");
        assert_eq!(Dialect::MySQL.quote_ident("email"), "`email`");
    }

    #[test]
    fn time_and_json_casts_wrap_per_dialect() {
        assert_eq!(Dialect::Postgres.wrap_time("$1"), "CAST($1 AS TIMESTAMPTZ)");
        assert_eq!(Dialect::Postgres.wrap_json("$2"), "CAST($2 AS JSONB)");
        assert_eq!(Dialect::MySQL.wrap_time("?"), "CAST(? AS DATETIME(6))");
        assert_eq!(Dialect::MySQL.wrap_json("?"), "CAST(? AS JSON)");
        assert_eq!(Dialect::SQLite.wrap_time("?"), "?");
        assert_eq!(Dialect::SQLite.wrap_json("?"), "?");
    }

    #[test]
    fn push_bind_inlines_null_and_numbers_placeholders() {
        let dialect = Dialect::Postgres;
        let (mut sql, mut binds) = (String::new(), Vec::new());
        dialect.push_bind(&mut sql, &mut binds, BindValue::Null);
        assert_eq!(sql, "NULL");
        assert!(binds.is_empty());

        let (mut sql, mut binds) = (String::new(), Vec::new());
        dialect.push_bind(&mut sql, &mut binds, BindValue::I64(1));
        sql.push_str(", ");
        dialect.push_bind(&mut sql, &mut binds, BindValue::Text("x".to_string()));
        assert_eq!(sql, "$1, $2");
        assert_eq!(binds.len(), 2);
    }

    #[test]
    fn push_bind_converts_time_and_json_to_text() {
        let moment = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let (mut sql, mut binds) = (String::new(), Vec::new());
        Dialect::Postgres.push_bind(&mut sql, &mut binds, BindValue::Time(moment));
        assert_eq!(sql, "CAST($1 AS TIMESTAMPTZ)");
        assert!(matches!(binds[0], BindValue::Text(_)));

        let (mut sql, mut binds) = (String::new(), Vec::new());
        Dialect::MySQL.push_bind(&mut sql, &mut binds, BindValue::Time(moment));
        assert_eq!(sql, "CAST(? AS DATETIME(6))");
        assert_eq!(
            binds[0],
            BindValue::Text("2023-11-14 22:13:20.000000".to_string())
        );

        let (mut sql, mut binds) = (String::new(), Vec::new());
        Dialect::SQLite.push_bind(&mut sql, &mut binds, BindValue::Json("{}".to_string()));
        assert_eq!(sql, "?");
        assert_eq!(binds[0], BindValue::Text("{}".to_string()));
    }

    #[tokio::test]
    async fn memory_database_round_trips_raw_queries() {
        let db = DB::memory().await.unwrap();
        db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .execute()
            .await
            .unwrap();
        let affected = db
            .raw("INSERT INTO t (v) VALUES (?)")
            .bind("hi")
            .execute()
            .await
            .unwrap();
        assert_eq!(affected, 1);
        let row = db
            .raw("SELECT v FROM t WHERE id = ?")
            .bind(1i64)
            .fetch_one()
            .await
            .unwrap();
        let value: String = sqlx::Row::try_get(&row, "v").unwrap();
        assert_eq!(value, "hi");
    }

    #[tokio::test]
    async fn transactions_commit_and_roll_back() {
        let db = DB::memory().await.unwrap();
        db.raw("CREATE TABLE t (id INTEGER PRIMARY KEY)")
            .execute()
            .await
            .unwrap();

        db.transaction(|tx| async move {
            tx.raw("INSERT INTO t (id) VALUES (1)").execute().await?;
            Ok::<(), Error>(())
        })
        .await
        .unwrap();
        let count: i64 = {
            let row = db
                .raw("SELECT COUNT(*) AS c FROM t")
                .fetch_one()
                .await
                .unwrap();
            sqlx::Row::try_get(&row, "c").unwrap()
        };
        assert_eq!(count, 1);

        let failed: Result<()> = db
            .transaction(|tx| async move {
                tx.raw("INSERT INTO t (id) VALUES (2)").execute().await?;
                Err(Error::not_found("boom"))
            })
            .await;
        assert!(failed.is_err());
        let count: i64 = {
            let row = db
                .raw("SELECT COUNT(*) AS c FROM t")
                .fetch_one()
                .await
                .unwrap();
            sqlx::Row::try_get(&row, "c").unwrap()
        };
        assert_eq!(count, 1, "rolled back");

        // Double-commit is an explicit error, not a silent no-op.
        let tx = db.begin().await.unwrap();
        tx.commit().await.unwrap();
        assert!(tx.commit().await.is_err());
        assert!(tx.is_finished());
    }
}
