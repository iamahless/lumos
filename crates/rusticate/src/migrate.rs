//! Migrations: versioned schema changes with dialect-aware DDL.
//!
//! Implement [`Migration`] (async `up`/`down` over a [`Schema`]), then run
//! them with [`Migrator`]. Applied migrations are tracked in a `migrations`
//! table with batch numbers, so [`rollback`](Migrator::rollback) and
//! [`fresh`](Migrator::fresh) behave like their Artisan counterparts.
//!
//! DDL renders per dialect from one portable vocabulary: auto-incrementing
//! keys, strings, integers, booleans, JSON, datetimes, UUIDs, enums (native
//! on MySQL, `CHECK`-constrained `VARCHAR` elsewhere), foreign keys,
//! indexes, and uniques. Each migration runs inside a transaction on
//! Postgres/SQLite; on MySQL (where DDL implicitly commits) it runs direct.
//!
//! Migration lists are explicit slices (`&[&dyn Migration]`) — no
//! auto-discovery magic, mirroring explicit provider registration.

use async_trait::async_trait;

use crate::db::{Dialect, Target};
use crate::value::DecodeField;
use crate::{Error, Result};

/// Portable column type, rendered per dialect by [`Schema`].
///
/// Built through [`Table`] builders in practice; the variants below show the
/// full vocabulary.
///
/// # Examples
///
/// ```rust
/// use rusticate::ColumnType;
///
/// assert!(matches!(ColumnType::String(255), ColumnType::String(255)));
/// ```
#[derive(Debug, Clone)]
pub enum ColumnType {
    /// Auto-incrementing 64-bit primary key (`id()`).
    BigIncrements,
    /// Auto-incrementing 32-bit primary key.
    Increments,
    /// `VARCHAR(n)`.
    String(usize),
    /// Unbounded text.
    Text,
    /// 32-bit integer.
    Integer,
    /// 64-bit integer.
    BigInteger,
    /// Boolean (`BOOLEAN` / `TINYINT(1)` / `INTEGER`).
    Boolean,
    /// JSON (`JSONB` / `JSON` / `TEXT`).
    Json,
    /// Timestamp with microseconds (`TIMESTAMPTZ` / `DATETIME(6)` / `TEXT`).
    DateTime,
    /// Calendar date.
    Date,
    /// UUID (`UUID` / `CHAR(36)` / `TEXT`; values bind as hyphenated strings).
    Uuid,
    /// String enum: native `ENUM` on MySQL, `VARCHAR(255)` + `CHECK` elsewhere.
    Enum(Vec<String>),
}

/// Column `DEFAULT` values.
///
/// Passed to [`ColumnDef::default`]; strings, integers, floats, and booleans
/// convert with `Into`.
///
/// # Examples
///
/// ```rust
/// use rusticate::DefaultValue;
///
/// assert!(matches!(DefaultValue::from("member"), DefaultValue::Text(_)));
/// assert!(matches!(DefaultValue::from(true), DefaultValue::Bool(true)));
/// ```
#[derive(Debug, Clone)]
pub enum DefaultValue {
    /// String literal (quoted + escaped).
    Text(String),
    /// Integer literal.
    Int(i64),
    /// Float literal.
    Float(f64),
    /// Boolean literal (`TRUE`/`FALSE` on Postgres, `1`/`0` elsewhere).
    Bool(bool),
    /// `NULL`.
    Null,
    /// `CURRENT_TIMESTAMP` (via [`.use_current()`](ColumnDef::use_current)).
    CurrentTimestamp,
}

impl From<&str> for DefaultValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for DefaultValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<i64> for DefaultValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for DefaultValue {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<bool> for DefaultValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<f64> for DefaultValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

/// One column definition with chainable modifiers.
///
/// Obtained from [`Table`] builders (`t.string("email").unique()`); every
/// modifier returns `&mut Self` for chaining.
///
/// # Examples
///
/// ```rust
/// use rusticate::Table;
///
/// let mut table = Table::default();
/// table.string("email").unique().nullable();
/// ```
#[derive(Debug, Clone)]
pub struct ColumnDef {
    name: String,
    ty: ColumnType,
    nullable: bool,
    default: Option<DefaultValue>,
    unique: bool,
    index: bool,
    references: Option<(String, String)>,
    on_delete: Option<String>,
    on_update: Option<String>,
}

impl ColumnDef {
    fn new(name: &str, ty: ColumnType) -> Self {
        Self {
            name: name.to_string(),
            ty,
            nullable: false,
            default: None,
            unique: false,
            index: false,
            references: None,
            on_delete: None,
            on_update: None,
        }
    }

    /// Allows `NULL` (columns are `NOT NULL` by default).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string("nickname").nullable();
    /// ```
    pub fn nullable(&mut self) -> &mut Self {
        self.nullable = true;
        self
    }

    /// Sets a `DEFAULT` literal.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string("role").default("member");
    /// table.integer("age").default(0);
    /// ```
    pub fn default(&mut self, value: impl Into<DefaultValue>) -> &mut Self {
        self.default = Some(value.into());
        self
    }

    /// Adds an inline `UNIQUE` constraint.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string("email").unique();
    /// ```
    pub fn unique(&mut self) -> &mut Self {
        self.unique = true;
        self
    }

    /// Creates a plain index after table creation (`{table}_{column}_index`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string("email").index();
    /// ```
    pub fn index(&mut self) -> &mut Self {
        self.index = true;
        self
    }

    /// Adds `REFERENCES {table}({column})` (pair with [`.on()`](ColumnDef::on)).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.foreign_id("team_id").references("id").on("teams");
    /// ```
    pub fn references(&mut self, column: &str) -> &mut Self {
        let table = self
            .references
            .take()
            .map(|(table, _)| table)
            .unwrap_or_default();
        self.references = Some((table, column.to_string()));
        self
    }

    /// Completes [`.references()`](ColumnDef::references) with the target table.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.foreign_id("team_id").references("id").on("teams");
    /// ```
    pub fn on(&mut self, table: &str) -> &mut Self {
        let column = self
            .references
            .take()
            .map(|(_, column)| column)
            .unwrap_or_default();
        self.references = Some((table.to_string(), column));
        self
    }

    /// Adds `ON DELETE` (`CASCADE`, `SET NULL`, `RESTRICT`, `NO ACTION`,
    /// `SET DEFAULT`; anything else fails at render time).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.foreign_id("team_id").references("id").on("teams").on_delete("cascade");
    /// ```
    pub fn on_delete(&mut self, action: &str) -> &mut Self {
        self.on_delete = Some(action.to_string());
        self
    }

    /// Adds `ON UPDATE` (same actions as [`.on_delete()`](ColumnDef::on_delete)).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.foreign_id("team_id").references("id").on("teams").on_update("cascade");
    /// ```
    pub fn on_update(&mut self, action: &str) -> &mut Self {
        self.on_update = Some(action.to_string());
        self
    }

    /// Defaults to `CURRENT_TIMESTAMP` (datetime columns).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.datetime("published_at").use_current();
    /// ```
    pub fn use_current(&mut self) -> &mut Self {
        self.default = Some(DefaultValue::CurrentTimestamp);
        self
    }
}

/// Table blueprint: accumulates columns inside `Schema::create` / `Schema::table`.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{Schema, DB};
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let schema = Schema::new(&db);
/// schema.create("users", |t| {
///     t.id();
///     t.string("email").unique();
///     t.timestamps();
/// }).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct Table {
    columns: Vec<ColumnDef>,
}

impl Table {
    /// Auto-incrementing `id` primary key (64-bit).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.id();
    /// ```
    pub fn id(&mut self) -> &mut ColumnDef {
        self.push("id", ColumnType::BigIncrements)
    }

    /// Auto-incrementing primary key with a custom name (64-bit).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.big_increments("team_id");
    /// ```
    pub fn big_increments(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::BigIncrements)
    }

    /// Auto-incrementing primary key with a custom name (32-bit).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.increments("legacy_id");
    /// ```
    pub fn increments(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Increments)
    }

    /// `VARCHAR(255)` column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string("email");
    /// ```
    pub fn string(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::String(255))
    }

    /// `VARCHAR(n)` column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.string_len("code", 8);
    /// ```
    pub fn string_len(&mut self, name: &str, len: usize) -> &mut ColumnDef {
        self.push(name, ColumnType::String(len))
    }

    /// Unbounded text column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.text("bio");
    /// ```
    pub fn text(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Text)
    }

    /// 32-bit integer column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.integer("age");
    /// ```
    pub fn integer(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Integer)
    }

    /// 64-bit integer column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.big_integer("counter");
    /// ```
    pub fn big_integer(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::BigInteger)
    }

    /// Boolean column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.boolean("active");
    /// ```
    pub fn boolean(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Boolean)
    }

    /// JSON column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.json("meta");
    /// ```
    pub fn json(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Json)
    }

    /// Timestamp column with microsecond precision.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.datetime("published_at");
    /// ```
    pub fn datetime(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::DateTime)
    }

    /// Date column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.date("birthday");
    /// ```
    pub fn date(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Date)
    }

    /// UUID column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.uuid("uid");
    /// ```
    pub fn uuid(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::Uuid)
    }

    /// String enum column (native on MySQL, checked `VARCHAR` elsewhere).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.enum_("role", &["admin", "member"]);
    /// ```
    pub fn enum_(&mut self, name: &str, values: &[&str]) -> &mut ColumnDef {
        self.push(
            name,
            ColumnType::Enum(values.iter().map(ToString::to_string).collect()),
        )
    }

    /// Foreign-key id column (64-bit integer; pair with
    /// [`.references().on()`](ColumnDef::references)).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.foreign_id("team_id");
    /// ```
    pub fn foreign_id(&mut self, name: &str) -> &mut ColumnDef {
        self.push(name, ColumnType::BigInteger)
    }

    /// Nullable `created_at` + `updated_at` timestamp columns.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.timestamps();
    /// ```
    pub fn timestamps(&mut self) {
        self.datetime("created_at").nullable();
        self.datetime("updated_at").nullable();
    }

    /// Nullable `deleted_at` timestamp column (soft deletes).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Table;
    ///
    /// let mut table = Table::default();
    /// table.soft_deletes();
    /// ```
    pub fn soft_deletes(&mut self) {
        self.datetime("deleted_at").nullable();
    }

    fn push(&mut self, name: &str, ty: ColumnType) -> &mut ColumnDef {
        self.columns.push(ColumnDef::new(name, ty));
        let last = self.columns.len() - 1;
        &mut self.columns[last]
    }
}

/// Schema builder: executes DDL against a target.
///
/// Constructed directly or handed to [`Migration`] hooks by [`Migrator`].
/// Beyond the builders below, [`target`](Schema::target) exposes the target
/// for hand-written DDL via [`RawQuery`](crate::RawQuery).
///
/// # Examples
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// use rusticate::{Schema, DB};
///
/// let db = DB::memory().await?;
/// let schema = Schema::new(&db);
/// assert!(!schema.has_table("users").await?);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Schema {
    target: Target,
}

impl Schema {
    /// Builds a schema handle on `&db` or `&tx`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// assert!(!schema.has_table("users").await?);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(target: impl Into<Target>) -> Self {
        Self {
            target: target.into(),
        }
    }

    /// Returns the underlying target (escape hatch for custom DDL).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// assert!(!schema.target().is_transaction());
    /// # Ok(())
    /// # }
    /// ```
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Returns the backend dialect.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Dialect, Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// assert_eq!(Schema::new(&db).dialect(), Dialect::SQLite);
    /// # Ok(())
    /// # }
    /// ```
    pub fn dialect(&self) -> Dialect {
        self.target.dialect()
    }

    /// Creates a table from a blueprint, then its indexes.
    ///
    /// Identifiers validate before anything executes; an empty blueprint is
    /// an explicit error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// schema
    ///     .create("users", |t| {
    ///         t.id();
    ///         t.string("email");
    ///     })
    ///     .await?;
    /// assert!(schema.has_table("users").await?);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create(&self, table: &str, build: impl FnOnce(&mut Table)) -> Result<()> {
        crate::validate_identifier(table)?;
        let mut blueprint = Table::default();
        build(&mut blueprint);
        if blueprint.columns.is_empty() {
            return Err(Error::invalid_query(format!(
                "cannot create table `{table}` with no columns"
            )));
        }
        for column in &blueprint.columns {
            crate::validate_identifier(&column.name)?;
        }
        let dialect = self.dialect();
        let mut definitions = Vec::with_capacity(blueprint.columns.len());
        for column in &blueprint.columns {
            definitions.push(render_column(dialect, column)?);
        }
        let sql = format!(
            "CREATE TABLE {} ({})",
            dialect.quote_ident(table),
            definitions.join(", ")
        );
        self.target.execute(&sql, Vec::new()).await?;
        for column in blueprint.columns.iter().filter(|column| column.index) {
            let index = format!("{table}_{}_index", column.name);
            let sql = format!(
                "CREATE INDEX {} ON {} ({})",
                dialect.quote_ident(&index),
                dialect.quote_ident(table),
                dialect.quote_ident(&column.name)
            );
            self.target.execute(&sql, Vec::new()).await?;
        }
        Ok(())
    }

    /// Adds columns to an existing table (`ALTER TABLE ... ADD COLUMN` per
    /// column; auto-increment keys are rejected — they only make sense at
    /// creation).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// schema.create("users", |t| { t.id(); }).await?;
    /// schema.table("users", |t| { t.string("nickname").nullable(); }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn table(&self, table: &str, build: impl FnOnce(&mut Table)) -> Result<()> {
        crate::validate_identifier(table)?;
        let mut blueprint = Table::default();
        build(&mut blueprint);
        for column in &blueprint.columns {
            crate::validate_identifier(&column.name)?;
            if matches!(
                column.ty,
                ColumnType::BigIncrements | ColumnType::Increments
            ) {
                return Err(Error::invalid_query(
                    "auto-increment keys cannot be added via table()",
                ));
            }
            let sql = format!(
                "ALTER TABLE {} ADD COLUMN {}",
                self.dialect().quote_ident(table),
                render_column(self.dialect(), column)?
            );
            self.target.execute(&sql, Vec::new()).await?;
        }
        Ok(())
    }

    /// Drops a table (missing tables are an error — see [`drop_if_exists`](Schema::drop_if_exists)).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// schema.create("users", |t| { t.id(); }).await?;
    /// schema.drop("users").await?;
    /// assert!(!schema.has_table("users").await?);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn drop(&self, table: &str) -> Result<()> {
        crate::validate_identifier(table)?;
        let sql = format!("DROP TABLE {}", self.dialect().quote_ident(table));
        self.target.execute(&sql, Vec::new()).await?;
        Ok(())
    }

    /// Drops a table unless missing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// schema.drop_if_exists("missing").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn drop_if_exists(&self, table: &str) -> Result<()> {
        crate::validate_identifier(table)?;
        let sql = format!("DROP TABLE IF EXISTS {}", self.dialect().quote_ident(table));
        self.target.execute(&sql, Vec::new()).await?;
        Ok(())
    }

    /// Whether a table exists (per-dialect catalog probe).
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Schema, DB};
    ///
    /// let db = DB::memory().await?;
    /// let schema = Schema::new(&db);
    /// assert!(!schema.has_table("users").await?);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn has_table(&self, table: &str) -> Result<bool> {
        crate::validate_identifier(table)?;
        let (sql, binds) = has_table_query(self.dialect(), table);
        Ok(self.target.fetch_optional(&sql, binds).await?.is_some())
    }
}

/// Renders one column definition for the dialect.
fn render_column(dialect: Dialect, column: &ColumnDef) -> Result<String> {
    let name = dialect.quote_ident(&column.name);
    let mut out = format!("{name} {}", column_type_sql(dialect, &column.ty, &name)?);
    if matches!(
        column.ty,
        ColumnType::BigIncrements | ColumnType::Increments
    ) {
        match dialect {
            Dialect::Postgres => out.push_str(" PRIMARY KEY"),
            Dialect::MySQL => out.push_str(" NOT NULL AUTO_INCREMENT PRIMARY KEY"),
            Dialect::SQLite => out.push_str(" PRIMARY KEY AUTOINCREMENT"),
        }
        return Ok(out);
    }
    out.push_str(if column.nullable {
        " NULL"
    } else {
        " NOT NULL"
    });
    if let Some(default) = &column.default {
        out.push_str(&format!(" DEFAULT {}", default_sql(dialect, default)));
    }
    if column.unique {
        out.push_str(" UNIQUE");
    }
    if let ColumnType::Enum(values) = &column.ty {
        if dialect != Dialect::MySQL {
            if values.is_empty() {
                return Err(Error::invalid_query(format!(
                    "enum `{}` needs at least one value",
                    column.name
                )));
            }
            let choices: Vec<String> = values.iter().map(|value| sql_string(value)).collect();
            out.push_str(&format!(" CHECK ({name} IN ({}))", choices.join(", ")));
        }
    }
    // Actions validate even without a foreign key: a dangling on_delete is a
    // caller bug, and silently dropping it would hide the mistake.
    if let Some(action) = &column.on_delete {
        fk_action(action)?;
    }
    if let Some(action) = &column.on_update {
        fk_action(action)?;
    }
    if let Some((table, target)) = &column.references {
        if table.is_empty() || target.is_empty() {
            return Err(Error::invalid_query(format!(
                "foreign key on `{}` needs both references() and on()",
                column.name
            )));
        }
        crate::validate_identifier(table)?;
        crate::validate_identifier(target)?;
        out.push_str(&format!(
            " REFERENCES {} ({})",
            dialect.quote_ident(table),
            dialect.quote_ident(target)
        ));
        if let Some(action) = &column.on_delete {
            out.push_str(&format!(" ON DELETE {}", fk_action(action)?));
        }
        if let Some(action) = &column.on_update {
            out.push_str(&format!(" ON UPDATE {}", fk_action(action)?));
        }
    }
    Ok(out)
}

/// Renders a portable type for the dialect.
fn column_type_sql(dialect: Dialect, ty: &ColumnType, quoted_name: &str) -> Result<String> {
    let _ = quoted_name;
    Ok(match (dialect, ty) {
        (_, ColumnType::BigIncrements) => match dialect {
            Dialect::Postgres => "BIGSERIAL".to_string(),
            Dialect::MySQL => "BIGINT UNSIGNED".to_string(),
            Dialect::SQLite => "INTEGER".to_string(),
        },
        (_, ColumnType::Increments) => match dialect {
            Dialect::Postgres => "SERIAL".to_string(),
            Dialect::MySQL => "INT UNSIGNED".to_string(),
            Dialect::SQLite => "INTEGER".to_string(),
        },
        (_, ColumnType::String(len)) => format!("VARCHAR({len})"),
        (_, ColumnType::Text) => "TEXT".to_string(),
        (_, ColumnType::Integer) => match dialect {
            Dialect::MySQL => "INT".to_string(),
            _ => "INTEGER".to_string(),
        },
        (_, ColumnType::BigInteger) => match dialect {
            Dialect::SQLite => "INTEGER".to_string(),
            _ => "BIGINT".to_string(),
        },
        (_, ColumnType::Boolean) => match dialect {
            Dialect::Postgres => "BOOLEAN".to_string(),
            Dialect::MySQL => "TINYINT(1)".to_string(),
            Dialect::SQLite => "INTEGER".to_string(),
        },
        (_, ColumnType::Json) => match dialect {
            Dialect::Postgres => "JSONB".to_string(),
            Dialect::MySQL => "JSON".to_string(),
            Dialect::SQLite => "TEXT".to_string(),
        },
        (_, ColumnType::DateTime) => match dialect {
            Dialect::Postgres => "TIMESTAMPTZ".to_string(),
            Dialect::MySQL => "DATETIME(6)".to_string(),
            Dialect::SQLite => "TEXT".to_string(),
        },
        (_, ColumnType::Date) => "DATE".to_string(),
        (_, ColumnType::Uuid) => match dialect {
            Dialect::Postgres => "UUID".to_string(),
            Dialect::MySQL => "CHAR(36)".to_string(),
            Dialect::SQLite => "TEXT".to_string(),
        },
        (Dialect::MySQL, ColumnType::Enum(values)) => {
            if values.is_empty() {
                return Err(Error::invalid_query("enum needs at least one value"));
            }
            let choices: Vec<String> = values.iter().map(|value| sql_string(value)).collect();
            format!("ENUM({})", choices.join(", "))
        }
        (_, ColumnType::Enum(_)) => "VARCHAR(255)".to_string(),
    })
}

/// Renders a `DEFAULT` literal for the dialect.
fn default_sql(dialect: Dialect, default: &DefaultValue) -> String {
    match default {
        DefaultValue::Text(text) => sql_string(text),
        DefaultValue::Int(int) => int.to_string(),
        DefaultValue::Float(float) => float.to_string(),
        DefaultValue::Bool(true) => match dialect {
            Dialect::Postgres => "TRUE".to_string(),
            _ => "1".to_string(),
        },
        DefaultValue::Bool(false) => match dialect {
            Dialect::Postgres => "FALSE".to_string(),
            _ => "0".to_string(),
        },
        DefaultValue::Null => "NULL".to_string(),
        DefaultValue::CurrentTimestamp => "CURRENT_TIMESTAMP".to_string(),
    }
}

/// Quotes a string literal (`'` doubling).
fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Validates a foreign-key action (case-insensitive input, canonical output).
fn fk_action(action: &str) -> Result<String> {
    const VALID: &[&str] = &[
        "CASCADE",
        "SET NULL",
        "RESTRICT",
        "NO ACTION",
        "SET DEFAULT",
    ];
    let upper = action.to_ascii_uppercase();
    if VALID.contains(&upper.as_str()) {
        Ok(upper)
    } else {
        Err(Error::invalid_query(format!(
            "invalid foreign key action {action:?}: expected one of CASCADE, SET NULL, RESTRICT, NO ACTION, SET DEFAULT"
        )))
    }
}

/// Builds the `has_table` catalog probe per dialect.
fn has_table_query(dialect: Dialect, table: &str) -> (String, Vec<crate::BindValue>) {
    use crate::BindValue;
    let mut sql;
    let mut binds = Vec::new();
    match dialect {
        Dialect::SQLite => {
            sql =
                "SELECT 1 AS probe FROM sqlite_master WHERE type = 'table' AND name = ".to_string();
            dialect.push_bind(&mut sql, &mut binds, BindValue::Text(table.to_string()));
        }
        Dialect::Postgres => {
            sql = "SELECT 1 AS probe FROM pg_tables WHERE schemaname = 'public' AND tablename = "
                .to_string();
            dialect.push_bind(&mut sql, &mut binds, BindValue::Text(table.to_string()));
        }
        Dialect::MySQL => {
            sql = "SELECT 1 AS probe FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = "
                .to_string();
            dialect.push_bind(&mut sql, &mut binds, BindValue::Text(table.to_string()));
        }
    }
    (sql, binds)
}

/// One versioned schema change.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{async_trait, Migration, Result, Schema};
///
/// pub struct CreateUsers;
///
/// #[async_trait]
/// impl Migration for CreateUsers {
///     async fn up(&self, schema: &mut Schema) -> Result<()> {
///         schema.create("users", |t| {
///             t.id();
///             t.string("email").unique();
///             t.timestamps();
///         }).await
///     }
///
///     async fn down(&self, schema: &mut Schema) -> Result<()> {
///         schema.drop_if_exists("users").await
///     }
/// }
/// ```
#[async_trait]
pub trait Migration: Send + Sync {
    /// Stable record name (defaults to the type path; override for prettier output).
    fn name(&self) -> String {
        std::any::type_name::<Self>().to_string()
    }

    /// Applies the change.
    async fn up(&self, schema: &mut Schema) -> Result<()>;

    /// Reverts the change.
    async fn down(&self, schema: &mut Schema) -> Result<()>;
}

/// One row of [`Migrator::status`].
///
/// # Examples
///
/// ```rust
/// use rusticate::MigrationStatus;
///
/// let status = MigrationStatus { name: "CreateUsers".to_string(), batch: Some(1) };
/// assert!(status.ran());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Migration name.
    pub name: String,
    /// Batch number, when applied.
    pub batch: Option<i64>,
}

impl MigrationStatus {
    /// Whether this migration has been applied.
    pub fn ran(&self) -> bool {
        self.batch.is_some()
    }
}

/// Runs migrations, tracking applied batches in a `migrations` table.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{Migrator, DB};
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let applied = Migrator::new(&db).run(&[]).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Migrator {
    target: Target,
}

impl Migrator {
    /// Builds a migrator on `&db` or `&tx`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Migrator, DB};
    ///
    /// let db = DB::memory().await?;
    /// let applied = Migrator::new(&db).run(&[]).await?;
    /// assert!(applied.is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(target: impl Into<Target>) -> Self {
        Self {
            target: target.into(),
        }
    }

    /// Runs pending migrations in slice order, recording one new batch.
    /// Returns the applied names.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{async_trait, Migration, Migrator, Result, Schema, DB};
    ///
    /// pub struct CreateUsers;
    ///
    /// #[async_trait]
    /// impl Migration for CreateUsers {
    ///     async fn up(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.create("users", |t| { t.id(); }).await
    ///     }
    ///
    ///     async fn down(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.drop("users").await
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let migrations: &[&dyn Migration] = &[&CreateUsers];
    /// let applied = Migrator::new(&db).run(migrations).await?;
    /// assert_eq!(applied.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&self, migrations: &[&dyn Migration]) -> Result<Vec<String>> {
        self.ensure_table().await?;
        let applied = self.applied().await?;
        let mut batch = applied.values().max().copied().unwrap_or(0) + 1;
        if batch < 1 {
            batch = 1;
        }
        let mut ran = Vec::new();
        for migration in migrations {
            if applied.contains_key(&migration.name()) {
                continue;
            }
            self.run_one(*migration, true).await.map_err(|error| {
                Error::Migration(format!("{} failed up: {error}", migration.name()))
            })?;
            let dialect = self.target.dialect();
            let mut sql = format!(
                "INSERT INTO {} ({}, {}) VALUES (",
                dialect.quote_ident("migrations"),
                dialect.quote_ident("migration"),
                dialect.quote_ident("batch")
            );
            let mut binds = Vec::new();
            dialect.push_bind(
                &mut sql,
                &mut binds,
                crate::BindValue::Text(migration.name()),
            );
            sql.push_str(", ");
            dialect.push_bind(&mut sql, &mut binds, crate::BindValue::I64(batch));
            sql.push(')');
            self.target.execute(&sql, binds).await?;
            ran.push(migration.name());
        }
        Ok(ran)
    }

    /// Rolls back the last `batches` batches (each batch in reverse order).
    /// Migrations missing from the slice fail explicitly — pass them all.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{async_trait, Migration, Migrator, Result, Schema, DB};
    ///
    /// pub struct CreateUsers;
    ///
    /// #[async_trait]
    /// impl Migration for CreateUsers {
    ///     async fn up(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.create("users", |t| { t.id(); }).await
    ///     }
    ///
    ///     async fn down(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.drop("users").await
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let migrations: &[&dyn Migration] = &[&CreateUsers];
    /// let migrator = Migrator::new(&db);
    /// migrator.run(migrations).await?;
    /// let rolled = migrator.rollback(migrations, 1).await?;
    /// assert_eq!(rolled.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn rollback(
        &self,
        migrations: &[&dyn Migration],
        batches: usize,
    ) -> Result<Vec<String>> {
        if batches < 1 {
            return Err(Error::invalid_query("rollback needs at least 1 batch"));
        }
        self.ensure_table().await?;
        let applied = self.applied().await?;
        let mut distinct: Vec<i64> = applied.values().copied().collect();
        distinct.sort_unstable();
        distinct.dedup();
        distinct.reverse();
        let doomed: Vec<i64> = distinct.into_iter().take(batches).collect();
        let mut names: Vec<String> = applied
            .iter()
            .filter(|(_, batch)| doomed.contains(batch))
            .map(|(name, _)| name.clone())
            .collect();
        // Reverse slice order within the doomed set (dependents first).
        let order: std::collections::HashMap<String, usize> = migrations
            .iter()
            .enumerate()
            .map(|(index, migration)| (migration.name(), index))
            .collect();
        names.sort_by_key(|name| std::cmp::Reverse(order.get(name).copied().unwrap_or(usize::MAX)));
        let mut rolled = Vec::new();
        for name in names {
            let migration = migrations
                .iter()
                .find(|candidate| candidate.name() == name)
                .ok_or_else(|| {
                    Error::Migration(format!(
                        "cannot roll back `{name}`: pass every migration to rollback()"
                    ))
                })?;
            self.run_one(*migration, false)
                .await
                .map_err(|error| Error::Migration(format!("{name} failed down: {error}")))?;
            let dialect = self.target.dialect();
            let mut sql = format!(
                "DELETE FROM {} WHERE {} = ",
                dialect.quote_ident("migrations"),
                dialect.quote_ident("migration")
            );
            let mut binds = Vec::new();
            dialect.push_bind(&mut sql, &mut binds, crate::BindValue::Text(name.clone()));
            self.target.execute(&sql, binds).await?;
            rolled.push(name);
        }
        Ok(rolled)
    }

    /// Drops every table, then runs all migrations from scratch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{async_trait, Migration, Migrator, Result, Schema, DB};
    ///
    /// pub struct CreateUsers;
    ///
    /// #[async_trait]
    /// impl Migration for CreateUsers {
    ///     async fn up(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.create("users", |t| { t.id(); }).await
    ///     }
    ///
    ///     async fn down(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.drop("users").await
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let migrations: &[&dyn Migration] = &[&CreateUsers];
    /// let applied = Migrator::new(&db).fresh(migrations).await?;
    /// assert_eq!(applied.len(), 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fresh(&self, migrations: &[&dyn Migration]) -> Result<Vec<String>> {
        for table in self.tables().await? {
            let dialect = self.target.dialect();
            let mut sql = format!("DROP TABLE {}", dialect.quote_ident(&table));
            if dialect == Dialect::Postgres {
                sql.push_str(" CASCADE");
            }
            if dialect == Dialect::MySQL {
                self.target
                    .execute("SET FOREIGN_KEY_CHECKS = 0", Vec::new())
                    .await?;
                self.target.execute(&sql, Vec::new()).await?;
                self.target
                    .execute("SET FOREIGN_KEY_CHECKS = 1", Vec::new())
                    .await?;
            } else {
                self.target.execute(&sql, Vec::new()).await?;
            }
        }
        self.run(migrations).await
    }

    /// Reports ran/pending state for the slice (in slice order).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{async_trait, Migration, Migrator, Result, Schema, DB};
    ///
    /// pub struct CreateUsers;
    ///
    /// #[async_trait]
    /// impl Migration for CreateUsers {
    ///     async fn up(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.create("users", |t| { t.id(); }).await
    ///     }
    ///
    ///     async fn down(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.drop("users").await
    ///     }
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let migrations: &[&dyn Migration] = &[&CreateUsers];
    /// let migrator = Migrator::new(&db);
    /// assert!(!migrator.status(migrations).await?[0].ran());
    /// migrator.run(migrations).await?;
    /// assert!(migrator.status(migrations).await?[0].ran());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn status(&self, migrations: &[&dyn Migration]) -> Result<Vec<MigrationStatus>> {
        self.ensure_table().await?;
        let applied = self.applied().await?;
        Ok(migrations
            .iter()
            .map(|migration| {
                let batch = applied.get(&migration.name()).copied();
                MigrationStatus {
                    name: migration.name(),
                    batch,
                }
            })
            .collect())
    }

    /// Runs one direction, transaction-wrapped on Postgres/SQLite (MySQL DDL
    /// implicitly commits, so it runs direct there).
    async fn run_one(&self, migration: &dyn Migration, up: bool) -> Result<()> {
        if self.target.dialect() == Dialect::MySQL || self.target.is_transaction() {
            let mut schema = Schema::new(self.target.clone());
            if up {
                migration.up(&mut schema).await
            } else {
                migration.down(&mut schema).await
            }
        } else {
            self.target
                .db()
                .transaction(|tx| async move {
                    let mut schema = Schema::new(tx);
                    if up {
                        migration.up(&mut schema).await
                    } else {
                        migration.down(&mut schema).await
                    }
                })
                .await
        }
    }

    /// Creates the bookkeeping table unless present.
    async fn ensure_table(&self) -> Result<()> {
        if Schema::new(self.target.clone())
            .has_table("migrations")
            .await?
        {
            return Ok(());
        }
        let schema = Schema::new(self.target.clone());
        schema
            .create("migrations", |t| {
                t.id();
                t.string("migration");
                t.integer("batch");
            })
            .await
    }

    /// Reads applied migrations (name → batch).
    async fn applied(&self) -> Result<std::collections::HashMap<String, i64>> {
        let dialect = self.target.dialect();
        let sql = format!(
            "SELECT {}, {} FROM {}",
            dialect.quote_ident("migration"),
            dialect.quote_ident("batch"),
            dialect.quote_ident("migrations")
        );
        let rows = self.target.fetch_all(&sql, Vec::new()).await?;
        let mut map = std::collections::HashMap::new();
        for row in &rows {
            let name = String::decode_field(row, "migration")?;
            let batch = i64::decode_field(row, "batch")?;
            map.insert(name, batch);
        }
        Ok(map)
    }

    /// Lists user tables for [`fresh`](Migrator::fresh).
    async fn tables(&self) -> Result<Vec<String>> {
        let sql = match self.target.dialect() {
            Dialect::SQLite => {
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
            }
            // `tablename` is of type `name`, which the `Any` driver cannot
            // return — cast to text.
            Dialect::Postgres => {
                "SELECT CAST(tablename AS TEXT) AS name FROM pg_tables WHERE schemaname = 'public'"
            }
            Dialect::MySQL => {
                "SELECT table_name AS name FROM information_schema.tables WHERE table_schema = DATABASE()"
            }
        };
        let rows = self.target.fetch_all(sql, Vec::new()).await?;
        rows.iter()
            .map(|row| String::decode_field(row, "name"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(dialect: Dialect, build: impl FnOnce(&mut Table)) -> Vec<String> {
        let mut table = Table::default();
        build(&mut table);
        table
            .columns
            .iter()
            .map(|column| render_column(dialect, column).unwrap())
            .collect()
    }

    #[test]
    fn kitchen_sink_renders_per_dialect() {
        let build = |t: &mut Table| {
            t.id();
            t.string("email").unique();
            t.string_len("code", 8).nullable();
            t.text("bio").nullable();
            t.integer("age").default(0);
            t.big_integer("counter");
            t.boolean("active").default(true);
            t.json("meta").nullable();
            t.datetime("at").use_current();
            t.date("day").nullable();
            t.uuid("uid").unique();
            t.enum_("role", &["admin", "member"]).default("member");
            t.foreign_id("team_id")
                .nullable()
                .references("id")
                .on("teams")
                .on_delete("cascade");
            t.timestamps();
            t.soft_deletes();
        };
        let pg = rendered(Dialect::Postgres, build);
        assert!(pg[0].contains("BIGSERIAL PRIMARY KEY"), "{}", pg[0]);
        assert!(
            pg[1].contains("\"email\" VARCHAR(255) NOT NULL UNIQUE"),
            "{}",
            pg[1]
        );
        assert!(pg[6].contains("BOOLEAN NOT NULL DEFAULT TRUE"), "{}", pg[6]);
        assert!(pg[7].contains("JSONB NULL"), "{}", pg[7]);
        assert!(
            pg[8].contains("TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP"),
            "{}",
            pg[8]
        );
        assert!(pg[10].contains("UUID NOT NULL UNIQUE"), "{}", pg[10]);
        assert!(
            pg[11].contains("CHECK (\"role\" IN ('admin', 'member'))"),
            "{}",
            pg[11]
        );
        assert!(
            pg[12].contains("REFERENCES \"teams\" (\"id\") ON DELETE CASCADE"),
            "{}",
            pg[12]
        );

        let my = rendered(Dialect::MySQL, build);
        assert!(
            my[0].contains("BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY"),
            "{}",
            my[0]
        );
        assert!(my[6].contains("TINYINT(1) NOT NULL DEFAULT 1"), "{}", my[6]);
        assert!(my[7].contains("JSON NULL"), "{}", my[7]);
        assert!(my[11].contains("ENUM('admin', 'member')"), "{}", my[11]);

        let lite = rendered(Dialect::SQLite, build);
        assert!(
            lite[0].contains("INTEGER PRIMARY KEY AUTOINCREMENT"),
            "{}",
            lite[0]
        );
        assert!(lite[7].contains("TEXT NULL"), "{}", lite[7]);
        assert!(lite[10].contains("TEXT NOT NULL UNIQUE"), "{}", lite[10]);
    }

    #[test]
    fn invalid_blueprints_fail_before_io() {
        let mut table = Table::default();
        table.string("x").on_delete("explode");
        assert!(render_column(Dialect::SQLite, &table.columns[0]).is_err());

        let mut table = Table::default();
        table.string("x").references("id");
        assert!(render_column(Dialect::SQLite, &table.columns[0]).is_err());

        let mut table = Table::default();
        table.enum_("e", &[]);
        assert!(render_column(Dialect::Postgres, &table.columns[0]).is_err());
    }

    #[test]
    fn migration_names_default_to_type_paths() {
        struct Demo;
        #[async_trait]
        impl Migration for Demo {
            async fn up(&self, _schema: &mut Schema) -> Result<()> {
                Ok(())
            }
            async fn down(&self, _schema: &mut Schema) -> Result<()> {
                Ok(())
            }
        }
        assert!(Demo.name().contains("Demo"));
    }
}
