//! Models: the [`Model`] trait, [`Changeset`], and [`Page`].
//!
//! A model is a struct deriving [`Model`](rusticate::Model) (the derive
//! lives in `lumos-macros`; this module is its runtime contract). The derive
//! generates metadata, row hydration, changesets, timestamp handling, and
//! relation loading; the trait below provides every query, persistence, and
//! lifecycle method as defaults, so models work the moment they compile.
//!
//! Every entry point takes `impl Into<Target>`: `&db` and `&tx`
//! interchangeably. There are no globals — the connection is always explicit.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use serde::Serialize;

use crate::db::{Dialect, Target};
use crate::query::Query;
use crate::value::{BindValue, EncodeField};
use crate::{Error, Result};
use sqlx::Row as _;

/// A paginated result set.
///
/// `last_page` is the exact ceiling (`0` when empty — arithmetically honest
/// rather than Laravel's minimum of 1); `has_more` tells clients whether to
/// render a next link.
///
/// # Examples
///
/// ```rust
/// use rusticate::Page;
///
/// let page: Page<String> = Page {
///     items: vec!["a".to_string()],
///     total: 41,
///     page: 2,
///     per_page: 20,
///     last_page: 3,
///     has_more: true,
/// };
/// assert!(page.has_more);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<M> {
    /// Items on this page.
    pub items: Vec<M>,
    /// Total matching rows across all pages.
    pub total: u64,
    /// Current page (1-based).
    pub page: u64,
    /// Requested page size.
    pub per_page: u64,
    /// Exact page count (`total.div_ceil(per_page)`).
    pub last_page: u64,
    /// Whether a further page exists (`page < last_page`).
    pub has_more: bool,
}

/// Column writes: an ordered map of column → value.
///
/// Built by hand ([`Changeset::set`]), from JSON ([`Changeset::from_json`]),
/// or from any serializable struct ([`Changeset::from_struct`]), then passed
/// to [`Model::create`] / [`Model::update`]. Ordering is deterministic
/// (`BTreeMap`), so generated SQL is stable run to run.
///
/// `None` values become SQL `NULL` (inlined, never bound).
///
/// # Examples
///
/// ```rust
/// use rusticate::Changeset;
///
/// let changeset = Changeset::new()
///     .set("name", "Ada")
///     .set("age", 36i32)
///     .set("nickname", None::<String>);
/// assert!(!changeset.is_empty());
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changeset {
    values: BTreeMap<String, BindValue>,
}

impl Changeset {
    /// Creates an empty changeset.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// assert!(Changeset::new().is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets one column (builder style).
    ///
    /// Accepts anything implementing [`EncodeField`]. Identifiers are
    /// validated when SQL renders; unknown columns fail fast in
    /// [`apply`](Model::apply).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// let changeset = Changeset::new().set("email", "a@b.c");
    /// assert_eq!(changeset.len(), 1);
    /// ```
    pub fn set(mut self, column: &str, value: impl EncodeField) -> Self {
        self.put(column, value);
        self
    }

    /// Sets one column mutably (for generated code and loops).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// let mut changeset = Changeset::new();
    /// changeset.put("a", 1i64).put("b", 2i64);
    /// assert_eq!(changeset.len(), 2);
    /// ```
    pub fn put(&mut self, column: &str, value: impl EncodeField) -> &mut Self {
        self.values.insert(column.to_string(), value.encode_field());
        self
    }

    /// Builds a changeset from a JSON object, mapping JSON types to bind
    /// types naturally (numbers stay numeric; arrays/objects become JSON
    /// documents). Non-objects are an error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{BindValue, Changeset};
    ///
    /// let changeset = Changeset::from_json(serde_json::json!({
    ///     "name": "Ada",
    ///     "age": 36,
    ///     "tags": ["x"],
    ///     "gone": null,
    /// }))
    /// .unwrap();
    /// assert_eq!(changeset.get("age"), Some(&BindValue::I64(36)));
    /// assert_eq!(changeset.get("gone"), Some(&BindValue::Null));
    /// ```
    pub fn from_json(value: serde_json::Value) -> Result<Self> {
        let serde_json::Value::Object(map) = value else {
            return Err(Error::Encode(
                "changeset JSON must be an object".to_string(),
            ));
        };
        let mut changeset = Self::new();
        for (column, item) in map {
            changeset.values.insert(column, json_to_bind(item)?);
        }
        Ok(changeset)
    }

    /// Builds a changeset from any serializable struct (`NewUser`-style
    /// inputs): serialize → object → [`Changeset::from_json`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct NewUser {
    ///     name: String,
    /// }
    ///
    /// let changeset = Changeset::from_struct(&NewUser { name: "Ada".to_string() }).unwrap();
    /// assert_eq!(changeset.len(), 1);
    /// ```
    pub fn from_struct<T: Serialize>(data: &T) -> Result<Self> {
        let value = serde_json::to_value(data).map_err(|error| {
            Error::Encode(format!("cannot serialize changeset struct: {error}"))
        })?;
        Self::from_json(value)
    }

    /// Returns the bound value for `column`, if present.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{BindValue, Changeset};
    ///
    /// let changeset = Changeset::new().set("a", 1i64);
    /// assert_eq!(changeset.get("a"), Some(&BindValue::I64(1)));
    /// assert_eq!(changeset.get("b"), None);
    /// ```
    pub fn get(&self, column: &str) -> Option<&BindValue> {
        self.values.get(column)
    }

    /// Returns the number of columns set.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// assert_eq!(Changeset::new().set("a", 1i64).len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns `true` when no columns are set.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// assert!(Changeset::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Iterates columns in deterministic order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Changeset;
    ///
    /// let changeset = Changeset::new().set("b", 1i64).set("a", 2i64);
    /// let columns: Vec<&str> = changeset.iter().map(|(col, _)| col.as_str()).collect();
    /// assert_eq!(columns, vec!["a", "b"]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&String, &BindValue)> {
        self.values.iter()
    }
}

/// Converts one JSON value to its natural bind form. Integers that fit `i64`
/// stay integers; wider unsigned values become an explicit error (no silent
/// truncation); floats stay floats.
fn json_to_bind(value: serde_json::Value) -> Result<BindValue> {
    match value {
        serde_json::Value::Null => Ok(BindValue::Null),
        serde_json::Value::Bool(flag) => Ok(BindValue::Bool(flag)),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(BindValue::I64(int))
            } else if let Some(uint) = number.as_u64() {
                i64::try_from(uint)
                    .map(BindValue::I64)
                    .map_err(|_| Error::Encode(format!("integer {uint} exceeds i64 range")))
            } else if let Some(float) = number.as_f64() {
                Ok(BindValue::F64(float))
            } else {
                Err(Error::Encode("unsupported JSON number".to_string()))
            }
        }
        serde_json::Value::String(text) => Ok(BindValue::Text(text)),
        array_or_object => serde_json::to_string(&array_or_object)
            .map(BindValue::Json)
            .map_err(|error| Error::Encode(format!("cannot encode nested JSON: {error}"))),
    }
}

/// Persistent domain object: the ORM's central contract.
///
/// Derive it (see the crate docs for the full attribute reference):
///
/// # Examples
///
/// ```rust
/// use rusticate::Model;
///
/// #[derive(Model)]
/// #[model(table = "users")]
/// pub struct User {
///     #[model(id, auto_increment)]
///     pub id: i64,
///     pub name: String,
/// }
/// ```
///
/// The derive generates metadata, hydration, changesets, timestamps, and
/// relation loading. Everything below — querying, creating, saving,
/// deleting — is provided as default methods.
///
/// # Soft deletes
///
/// With `soft_deletes = true`, queries automatically exclude trashed rows;
/// [`Query::with_trashed`] disables the scope, [`Query::only_trashed`]
/// inverts it. [`Model::delete`] then stamps instead of deleting; use
/// [`force_delete`](Model::force_delete) for a real `DELETE`.
///
/// Native `async fn` (not `async_trait`): this trait is never used as a
/// trait object, and every future is `Send` in practice — the only captured
/// types are `Target`, `Changeset`, and model references, all `Send`.
#[allow(async_fn_in_trait)]
pub trait Model: Sized + Send + Sync + 'static {
    /// Table name, from `#[model(table = "...")]`.
    fn table() -> &'static str;

    /// Primary-key column. Defaults to `"id"`.
    fn primary_key() -> &'static str {
        "id"
    }

    /// Database columns in struct order, excluding relation fields.
    fn columns() -> &'static [&'static str];

    /// Whether the primary key auto-increments (excluded from `INSERT`s,
    /// read back afterwards). Defaults to `true`.
    fn auto_increment_pk() -> bool {
        true
    }

    /// Whether `created_at`/`updated_at` stamp automatically.
    fn uses_timestamps() -> bool {
        false
    }

    /// `created_at` column, when timestamping.
    fn created_at_column() -> Option<&'static str> {
        None
    }

    /// `updated_at` column, when timestamping.
    fn updated_at_column() -> Option<&'static str> {
        None
    }

    /// Whether deletes soft-delete (stamp `deleted_at`) instead of deleting.
    fn soft_deletes() -> bool {
        false
    }

    /// `deleted_at` column, when soft-deleting.
    fn deleted_at_column() -> Option<&'static str> {
        None
    }

    /// Columns excluded from [`to_value`](Model::to_value)
    /// (`#[model(hidden)]`).
    fn hidden_columns() -> &'static [&'static str] {
        &[]
    }

    /// Declared relation names, for `with()` validation and error messages.
    fn relation_names() -> &'static [&'static str] {
        &[]
    }

    /// Columns the `Any` driver cannot return natively (timestamps, UUIDs,
    /// JSON): selected as `CAST(col AS TEXT)` on Postgres/MySQL, plain on
    /// SQLite (where they already live in `TEXT` columns). The derive fills
    /// this from field types (`DateTime`, `Uuid`, `serde_json::Value`) and
    /// `casts = "json" | "datetime"`.
    fn text_cast_columns() -> &'static [&'static str] {
        &[]
    }

    /// Boolean columns: selected as `CAST(col AS SIGNED)` on MySQL (whose
    /// `TINYINT` the `Any` driver cannot return), plain elsewhere. The derive
    /// fills this from `bool` field types and `casts = "bool"`.
    fn bool_columns() -> &'static [&'static str] {
        &[]
    }

    /// Renders the `SELECT` column list for `dialect`: plain identifiers,
    /// except [`text_cast_columns`](Model::text_cast_columns) (cast to text
    /// with a same-name alias, so hydration by name keeps working) and
    /// [`bool_columns`](Model::bool_columns) on MySQL (cast to signed).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::{Dialect, Model};
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub name: String,
    /// # }
    ///
    /// assert_eq!(User::select_list(Dialect::SQLite), r#""id", "name""#);
    /// ```
    fn select_list(dialect: Dialect) -> String {
        Self::columns()
            .iter()
            .map(|column| {
                let quoted = dialect.quote_ident(column);
                if Self::text_cast_columns().contains(column) {
                    match dialect {
                        Dialect::Postgres => {
                            format!(r#"CAST({quoted} AS TEXT) AS {quoted}"#)
                        }
                        Dialect::MySQL => {
                            format!(r#"CAST({quoted} AS CHAR) AS {quoted}"#)
                        }
                        Dialect::SQLite => quoted,
                    }
                } else if dialect == Dialect::MySQL && Self::bool_columns().contains(column) {
                    format!(r#"CAST({quoted} AS SIGNED) AS {quoted}"#)
                } else {
                    quoted
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Hydrates one instance from a result row (generated).
    fn from_row(row: &crate::AnyRow) -> Result<Self>;

    /// Collects non-primary-key columns for `UPDATE`s (generated).
    fn to_changeset(&self) -> Result<Changeset>;

    /// Reads the primary-key value for `WHERE` clauses (generated).
    fn pk_value(&self) -> BindValue;

    /// Applies a changeset to this instance, for [`update`](Model::update).
    /// Unknown columns and type mismatches are explicit errors (generated).
    fn apply(&mut self, changeset: &Changeset) -> Result<()>;

    /// Builds an instance from a creation changeset (generated): regular
    /// fields come from the changeset (absent non-nullable fields are an
    /// error; absent `Option` fields become `None`); timestamps default to
    /// now unless supplied; relations start unloaded; the auto-increment key
    /// starts as a placeholder replaced after insert.
    fn from_changeset(changeset: &Changeset) -> Result<Self>;

    /// Writes a freshly-inserted auto-increment id back (generated).
    /// Non-incrementing keys ignore the argument (their value was supplied).
    fn set_pk_from_i64(&mut self, id: i64) -> Result<()>;

    /// Stamps `updated_at` to now, if timestamping (generated).
    fn stamp_update(&mut self);

    /// Reads the current `updated_at` for `UPDATE`s, if timestamping (generated).
    fn updated_at_value(&self) -> Option<BindValue> {
        let _ = Self::updated_at_column()?;
        None
    }

    /// Writes `deleted_at` (`None` restores), if soft-deleting (generated).
    fn set_deleted_at(&mut self, _deleted: Option<chrono::DateTime<chrono::Utc>>);

    /// Whether this instance is soft-deleted (generated; always `false`
    /// without soft deletes).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users", soft_deletes = true)]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let user = User::find_or_fail(&db, 1i64).await?;
    /// assert!(!user.trashed());
    /// # Ok(())
    /// # }
    /// ```
    fn trashed(&self) -> bool {
        false
    }

    /// Serializes columns to JSON: hidden columns excluded, `appends`
    /// methods included, *loaded* relations embedded recursively (unloaded
    /// relations are absent, never an error). Fallible: exotic field values
    /// that cannot serialize fail honestly instead of emitting `null`.
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
    /// let user = User::find_or_fail(&db, 1i64).await?;
    /// let value = user.to_value()?;
    /// # Ok(())
    /// # }
    /// ```
    fn to_value(&self) -> Result<serde_json::Value>;

    /// Eager-loads one dotted relation path (`["posts"]`, `["posts",
    /// "comments"]`) into already-fetched models (generated). Boxed rather
    /// than `async fn`: bidirectional relations recurse through this method
    /// (`User` loads `Post`s which load their `User`s), and only a boxed
    /// future has finite type across that cycle. Callers just `.await` it.
    fn load_relation<'a>(
        target: &'a Target,
        models: &'a mut [Self],
        path: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Starts a query for this model on `&db` or `&tx`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    ///
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let users = User::query(&db).where_eq("id", 1i64).get().await?;
    /// # Ok(())
    /// # }
    /// ```
    fn query(target: impl Into<Target>) -> Query<Self> {
        Query::new(target.into())
    }

    /// Fetches every row (soft-deleted excluded unless the model opts out).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[derive(rusticate::Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// use rusticate::{Model, DB};
    ///
    /// let db = DB::memory().await?;
    /// let users = User::all(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn all(target: impl Into<Target>) -> Result<Vec<Self>> {
        Self::query(target).get().await
    }

    /// Finds one row by primary key, or `None`.
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
    /// let user: Option<User> = User::find(&db, 1i64).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn find(target: impl Into<Target>, id: impl EncodeField) -> Result<Option<Self>> {
        Self::query(target)
            .where_eq(Self::primary_key(), id)
            .first()
            .await
    }

    /// Finds one row by primary key, or [`Error::NotFound`].
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
    /// let user = User::find_or_fail(&db, 1i64).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn find_or_fail(target: impl Into<Target>, id: impl EncodeField) -> Result<Self> {
        let key = id.encode_field();
        Self::find(target, key.clone()).await?.ok_or_else(|| {
            Error::not_found(format!(
                "{} with {} = {key:?}",
                Self::table(),
                Self::primary_key()
            ))
        })
    }

    /// Inserts a row from a changeset and returns the complete instance.
    ///
    /// Flow: build → `saving`/`creating` hooks → `INSERT` (auto-increment
    /// keys excluded; timestamps/UUIDs already stamped) → read the key back
    /// (`RETURNING` on Postgres, last-insert id elsewhere) → `created`/`saved`
    /// hooks.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Changeset, Model, DB};
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
    /// let user = User::create(&db, Changeset::new().set("name", "Ada")).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn create(target: impl Into<Target>, changeset: Changeset) -> Result<Self> {
        if changeset.is_empty() {
            return Err(Error::invalid_query(
                "cannot create with an empty changeset",
            ));
        }
        let target = target.into();
        let mut model = Self::from_changeset(&changeset)?;
        crate::observers::fire_saving(&target, &mut model).await?;
        crate::observers::fire_creating(&target, &mut model).await?;

        let mut columns = model.to_changeset()?;
        if !Self::auto_increment_pk() {
            columns.put(Self::primary_key(), model.pk_value());
        }
        if target.dialect() == Dialect::Postgres {
            let row = insert_returning::<Self>(&target, Self::table(), &columns).await?;
            model = Self::from_row(&row)?;
        } else {
            insert(&target, Self::table(), &columns).await?;
            if Self::auto_increment_pk() {
                let id = last_insert_id(&target).await?;
                model.set_pk_from_i64(id)?;
            }
        }

        crate::observers::fire_created(&target, &model).await?;
        crate::observers::fire_saved(&target, &model).await?;
        Ok(model)
    }

    /// Inserts a row from a JSON object (see [`Changeset::from_json`]).
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
    /// let user = User::create_json(&db, serde_json::json!({ "name": "Ada" })).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn create_json(target: impl Into<Target>, value: serde_json::Value) -> Result<Self> {
        Self::create(target, Changeset::from_json(value)?).await
    }

    /// Inserts a row from a serializable struct (`NewUser`-style inputs).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// use serde::Serialize;
    /// # #[derive(Model)]
    /// # #[model(table = "users")]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub name: String,
    /// # }
    ///
    /// #[derive(Serialize)]
    /// struct NewUser {
    ///     name: String,
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let input = NewUser { name: "Ada".to_string() };
    /// let user = User::create_struct(&db, &input).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn create_struct<T: Serialize>(target: impl Into<Target>, data: &T) -> Result<Self> {
        Self::create(target, Changeset::from_struct(data)?).await
    }

    /// Persists this instance's columns (`UPDATE`, never `INSERT` — creation
    /// goes through [`create`](Model::create), keeping the two paths explicit).
    ///
    /// Flow: stamp `updated_at` → `saving`/`updating` hooks → `UPDATE` all
    /// non-key columns → `updated`/`saved` hooks.
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
    /// let mut user = User::find_or_fail(&db, 1i64).await?;
    /// user.name = "Grace".to_string();
    /// user.save(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn save(&mut self, target: impl Into<Target>) -> Result<()> {
        let target = target.into();
        self.stamp_update();
        crate::observers::fire_saving(&target, self).await?;
        crate::observers::fire_updating(&target, self).await?;
        let changeset = self.to_changeset()?;
        if !changeset.is_empty() {
            update_by_pk(
                &target,
                Self::table(),
                Self::primary_key(),
                self.pk_value(),
                &changeset,
            )
            .await?;
        }
        crate::observers::fire_updated(&target, self).await?;
        crate::observers::fire_saved(&target, self).await?;
        Ok(())
    }

    /// Applies a changeset to this instance and persists it (plus an
    /// `updated_at` touch). Empty changesets are an explicit error.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Changeset, Model, DB};
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
    /// let mut user = User::find_or_fail(&db, 1i64).await?;
    /// user.update(&db, Changeset::new().set("name", "Grace")).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn update(&mut self, target: impl Into<Target>, changeset: Changeset) -> Result<()> {
        if changeset.is_empty() {
            return Err(Error::invalid_query(
                "cannot update with an empty changeset",
            ));
        }
        let target = target.into();
        self.apply(&changeset)?;
        self.stamp_update();
        let mut columns = changeset;
        if let (Some(column), Some(stamp)) = (Self::updated_at_column(), self.updated_at_value()) {
            columns.put(column, stamp);
        }
        crate::observers::fire_saving(&target, self).await?;
        crate::observers::fire_updating(&target, self).await?;
        update_by_pk(
            &target,
            Self::table(),
            Self::primary_key(),
            self.pk_value(),
            &columns,
        )
        .await?;
        crate::observers::fire_updated(&target, self).await?;
        crate::observers::fire_saved(&target, self).await?;
        Ok(())
    }

    /// Deletes this row: stamps `deleted_at` when soft-deleting, otherwise a
    /// real `DELETE`. Fires `deleting`/`deleted` either way.
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
    /// let mut user = User::find_or_fail(&db, 1i64).await?;
    /// user.delete(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn delete(&mut self, target: impl Into<Target>) -> Result<()> {
        let target = target.into();
        crate::observers::fire_deleting(&target, self).await?;
        if let Some(column) = Self::deleted_at_column().filter(|_| Self::soft_deletes()) {
            let now = chrono::Utc::now();
            let changeset = Changeset::new().set(column, now);
            update_by_pk(
                &target,
                Self::table(),
                Self::primary_key(),
                self.pk_value(),
                &changeset,
            )
            .await?;
            self.set_deleted_at(Some(now));
        } else {
            delete_by_pk(&target, Self::table(), Self::primary_key(), self.pk_value()).await?;
        }
        crate::observers::fire_deleted(&target, self).await?;
        Ok(())
    }

    /// Always issues a real `DELETE`, even for soft-deleting models.
    /// Fires `deleting`/`deleted`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users", soft_deletes = true)]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let mut user = User::query(&db).with_trashed().first_or_fail().await?;
    /// user.force_delete(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn force_delete(&mut self, target: impl Into<Target>) -> Result<()> {
        let target = target.into();
        crate::observers::fire_deleting(&target, self).await?;
        delete_by_pk(&target, Self::table(), Self::primary_key(), self.pk_value()).await?;
        crate::observers::fire_deleted(&target, self).await?;
        Ok(())
    }

    /// Clears `deleted_at` on soft-deleting models (idempotent). Calling it
    /// on a model without soft deletes is an explicit error.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use rusticate::{Model, DB};
    /// # #[derive(Model)]
    /// # #[model(table = "users", soft_deletes = true)]
    /// # pub struct User {
    /// #     #[model(id, auto_increment)]
    /// #     pub id: i64,
    /// #     pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// # }
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = DB::memory().await?;
    /// let mut user = User::query(&db).only_trashed().first_or_fail().await?;
    /// user.restore(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn restore(&mut self, target: impl Into<Target>) -> Result<()> {
        if !(Self::soft_deletes() && Self::deleted_at_column().is_some()) {
            return Err(Error::invalid_query(format!(
                "model `{}` does not use soft deletes",
                Self::table()
            )));
        }
        let target = target.into();
        crate::observers::fire_restoring(&target, self).await?;
        let column = Self::deleted_at_column().unwrap_or("deleted_at");
        let changeset = Changeset::new().set(column, None::<String>);
        update_by_pk(
            &target,
            Self::table(),
            Self::primary_key(),
            self.pk_value(),
            &changeset,
        )
        .await?;
        self.set_deleted_at(None);
        crate::observers::fire_restored(&target, self).await?;
        Ok(())
    }

    /// Re-reads this row by primary key, replacing the instance in place.
    /// Errors with [`Error::NotFound`] when the row is gone.
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
    /// let mut user = User::find_or_fail(&db, 1i64).await?;
    /// user.refresh(&db).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn refresh(&mut self, target: impl Into<Target>) -> Result<()> {
        *self = Self::find_or_fail(target, self.pk_value()).await?;
        Ok(())
    }
}

/// Renders `INSERT INTO table (cols) VALUES (...)` and runs it.
async fn insert(target: &Target, table: &str, changeset: &Changeset) -> Result<()> {
    let (sql, binds) = render_insert(target.dialect(), table, changeset)?;
    target.execute(&sql, binds).await?;
    Ok(())
}

/// Postgres `INSERT ... RETURNING <select list>`, hydrating the stored row.
/// An explicit list (never `*`): timestamp/UUID/JSON columns need their
/// text casts, exactly like `SELECT`.
async fn insert_returning<M: Model>(
    target: &Target,
    table: &str,
    changeset: &Changeset,
) -> Result<crate::AnyRow> {
    let (mut sql, binds) = render_insert(target.dialect(), table, changeset)?;
    sql.push_str(" RETURNING ");
    sql.push_str(&M::select_list(target.dialect()));
    target
        .fetch_optional(&sql, binds)
        .await?
        .ok_or_else(|| Error::not_found(format!("inserted row in {table}")))
}

/// Shared `INSERT` renderer (quoted identifiers, native placeholders).
/// An empty column set (pk-only models, whose key is always excluded)
/// renders as a default-values insert.
fn render_insert(
    dialect: Dialect,
    table: &str,
    changeset: &Changeset,
) -> Result<(String, Vec<BindValue>)> {
    crate::validate_identifier(table)?;
    if changeset.is_empty() {
        let sql = match dialect {
            Dialect::MySQL => format!("INSERT INTO {} VALUES ()", dialect.quote_ident(table)),
            Dialect::Postgres | Dialect::SQLite => {
                format!("INSERT INTO {} DEFAULT VALUES", dialect.quote_ident(table))
            }
        };
        return Ok((sql, Vec::new()));
    }
    let mut sql = format!("INSERT INTO {} (", dialect.quote_ident(table));
    let mut values = String::from(" VALUES (");
    let mut binds = Vec::new();
    let mut first = true;
    for (column, value) in changeset.iter() {
        crate::validate_identifier(column)?;
        if !first {
            sql.push_str(", ");
            values.push_str(", ");
        }
        first = false;
        sql.push_str(&dialect.quote_ident(column));
        dialect.push_bind(&mut values, &mut binds, value.clone());
    }
    sql.push(')');
    values.push(')');
    sql.push_str(&values);
    Ok((sql, binds))
}

/// Reads the last auto-increment id on SQLite/MySQL (Postgres uses `RETURNING`).
async fn last_insert_id(target: &Target) -> Result<i64> {
    let sql = match target.dialect() {
        Dialect::SQLite => "SELECT last_insert_rowid() AS id",
        Dialect::MySQL => "SELECT LAST_INSERT_ID() AS id",
        Dialect::Postgres => {
            return Err(Error::invalid_query(
                "last_insert_id is unavailable on Postgres (uses RETURNING)",
            ))
        }
    };
    let row = target
        .fetch_optional(sql, Vec::new())
        .await?
        .ok_or_else(|| Error::not_found("last insert id"))?;
    row.try_get::<i64, _>("id")
        .map_err(|_| Error::Decode("last insert id is not an integer".to_string()))
}

/// Renders and runs `UPDATE table SET ... WHERE pk = ?`.
async fn update_by_pk(
    target: &Target,
    table: &str,
    pk: &str,
    id: BindValue,
    changeset: &Changeset,
) -> Result<()> {
    crate::validate_identifier(table)?;
    crate::validate_identifier(pk)?;
    let dialect = target.dialect();
    let mut sql = format!("UPDATE {} SET ", dialect.quote_ident(table));
    let mut binds = Vec::new();
    let mut first = true;
    for (column, value) in changeset.iter() {
        crate::validate_identifier(column)?;
        if !first {
            sql.push_str(", ");
        }
        first = false;
        sql.push_str(&dialect.quote_ident(column));
        sql.push_str(" = ");
        dialect.push_bind(&mut sql, &mut binds, value.clone());
    }
    sql.push_str(" WHERE ");
    sql.push_str(&dialect.quote_ident(pk));
    sql.push_str(" = ");
    dialect.push_bind(&mut sql, &mut binds, id);
    target.execute(&sql, binds).await?;
    Ok(())
}

/// Renders and runs `DELETE FROM table WHERE pk = ?`.
async fn delete_by_pk(target: &Target, table: &str, pk: &str, id: BindValue) -> Result<()> {
    crate::validate_identifier(table)?;
    crate::validate_identifier(pk)?;
    let dialect = target.dialect();
    let mut sql = format!("DELETE FROM {} WHERE ", dialect.quote_ident(table));
    sql.push_str(&dialect.quote_ident(pk));
    sql.push_str(" = ");
    let mut binds = Vec::new();
    dialect.push_bind(&mut sql, &mut binds, id);
    target.execute(&sql, binds).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_changesets_map_types_naturally() {
        let changeset = Changeset::from_json(serde_json::json!({
            "int": 3,
            "float": 1.5,
            "text": "x",
            "flag": true,
            "nothing": null,
            "doc": {"a": [1]},
        }))
        .unwrap();
        assert_eq!(changeset.get("int"), Some(&BindValue::I64(3)));
        assert_eq!(changeset.get("float"), Some(&BindValue::F64(1.5)));
        assert_eq!(
            changeset.get("text"),
            Some(&BindValue::Text("x".to_string()))
        );
        assert_eq!(changeset.get("flag"), Some(&BindValue::Bool(true)));
        assert_eq!(changeset.get("nothing"), Some(&BindValue::Null));
        assert_eq!(
            changeset.get("doc"),
            Some(&BindValue::Json(r#"{"a":[1]}"#.to_string()))
        );
    }

    #[test]
    fn json_changesets_reject_non_objects_and_huge_ints() {
        assert!(Changeset::from_json(serde_json::json!([1])).is_err());
        assert!(Changeset::from_json(serde_json::json!(1)).is_err());
        let huge = serde_json::json!({ "n": 18446744073709551615u64 });
        assert!(Changeset::from_json(huge).is_err());
    }

    #[test]
    fn insert_renders_per_dialect() {
        let changeset = Changeset::new().set("name", "Ada").set("age", 36i32);
        let (pg, _) = render_insert(Dialect::Postgres, "users", &changeset).unwrap();
        assert_eq!(pg, r#"INSERT INTO "users" ("age", "name") VALUES ($1, $2)"#);
        let (lite, _) = render_insert(Dialect::SQLite, "users", &changeset).unwrap();
        assert_eq!(lite, r#"INSERT INTO "users" ("age", "name") VALUES (?, ?)"#);
        let (my, _) = render_insert(Dialect::MySQL, "users", &changeset).unwrap();
        assert_eq!(my, "INSERT INTO `users` (`age`, `name`) VALUES (?, ?)");
    }
}
