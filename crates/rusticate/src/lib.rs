//! Rusticate: an Eloquent-inspired ORM for Rust.
//!
//! This standalone crate depends on nothing from `lumos-core`,
//! so it works in any Tokio application. The `lumos` facade re-exports it
//! behind the `orm` feature and maps [`Error`] onto HTTP statuses
//! (`NotFound` → 404, `UniqueViolation` → 409, everything else → 500).
//!
//! One implementation serves SQLite, Postgres, and MySQL via sqlx's `Any`
//! driver; the backend is chosen at runtime from the connection URL, and
//! [`Dialect`] centralizes every SQL difference (quoting, `$N` vs `?`
//! placeholders, timestamp/JSON casts).
//!
//! There are no globals: every query takes an explicit [`Target`] (`&db` or
//! `&tx`), and every builder can be retargeted onto a transaction with
//! [`.on(&tx)`](Query::on). All framework magic is compile-time proc-macro
//! codegen ([`Model`], [`scopes`]); there is no runtime reflection.
//!
//! # Example
//!
//! ```rust
//! # #[tokio::main]
//! # async fn main() -> rusticate::Result<()> {
//! use rusticate::{Changeset, Model, Schema, DB};
//!
//! #[derive(Model)]
//! #[model(table = "users")]
//! pub struct User {
//!     #[model(id, auto_increment)]
//!     pub id: i64,
//!     pub name: String,
//! }
//!
//! let db = DB::memory().await?;
//! Schema::new(&db)
//!     .create("users", |t| {
//!         t.id();
//!         t.string("name");
//!     })
//!     .await?;
//!
//! let user = User::create(&db, Changeset::new().set("name", "Ada")).await?;
//! assert_eq!(user.name, "Ada");
//! let found = User::find_or_fail(&db, user.id).await?;
//! assert_eq!(found.name, "Ada");
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod db;
mod error;
mod factory;
mod migrate;
mod model;
mod observers;
mod query;
mod value;

/// Relationship loading internals (see [`HasMany`] / [`BelongsTo`]).
///
/// Public only because generated `load_relation` code calls into it;
/// applications use [`.with([...])`](Query::with) instead.
pub mod relations;

pub use db::{Dialect, RawQuery, Target, Transaction, DB};
pub use error::{Error, Result};
pub use factory::Seeder;
pub use migrate::{
    ColumnDef, ColumnType, DefaultValue, Migration, MigrationStatus, Migrator, Schema, Table,
};
pub use model::{Changeset, Model, Page};
pub use observers::Observer;
pub use query::Query;
pub use relations::{BelongsTo, HasMany};
pub use value::{
    decode_display, decode_display_opt, decode_json, encode_display, encode_json, parse_datetime,
    parse_display, parse_display_opt, parse_json, validate_identifier, BindValue, DecodeField,
    EncodeField, FromBind,
};

// Re-exported for generated code and user ergonomics: model derives emit
// `::rusticate::chrono` / `::rusticate::serde_json` paths, observers and
// migrations are implemented with `#[rusticate::async_trait]`, and rows
// surface as `AnyRow` through raw queries.
pub use async_trait::async_trait;
pub use chrono;
pub use serde_json;
pub use sqlx::any::AnyRow;

// Derives share their names with runtime items by design: `Model` the trait
// (type namespace) and `Model` the derive (macro namespace) are imported
// together with a single `use rusticate::Model;`.
pub use lumos_macros::{scopes, Model};
