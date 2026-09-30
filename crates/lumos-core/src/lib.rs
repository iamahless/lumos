//! Lumos core kernel: routing, HTTP primitives, service container,
//! configuration, errors, middleware, and the application boot lifecycle.
//!
//! Most applications depend on the `lumos` facade instead of this crate
//! directly; everything public here is re-exported from there, plus the
//! `routes!` / `group!` macros and `#[controller]` codegen.
//!
//! # Feature flags
//!
//! `orm` maps `rusticate::Error` onto [`AppError`] (`NotFound` → 404,
//! `UniqueViolation` → 409, the rest → 500). `validation` adds the
//! `validate` module, `auth` the `auth` module (implies `validation`),
//! `views` the `views` module. `cache`, `queue`, `jsonapi`,
//! `jsonapi-lite`, and `http-cache` exist so the facade's feature matrix is
//! real and CI-testable; their modules land in later phases. `jsonapi` and
//! `jsonapi-lite` are mutually exclusive (compile error).
//!
//! # Example
//!
//! ```rust,no_run
//! use lumos_core::{get, serve, Router};
//!
//! # #[tokio::main]
//! # async fn main() -> lumos_core::Result<()> {
//! let router = Router::new().route("/", get(|| async { "Hello, Lumos!" }));
//! serve(router, "127.0.0.1:3000").await
//! # }
//! ```

#![warn(missing_docs)]

#[cfg(all(feature = "jsonapi", feature = "jsonapi-lite"))]
compile_error!(
    "lumos-core features `jsonapi` and `jsonapi-lite` are mutually exclusive; enable only one."
);

pub mod app;
#[cfg(feature = "auth")]
pub mod auth;
pub mod config;
pub mod container;
pub mod error;
pub mod extract;
pub mod log;
pub mod middleware;
pub mod provider;
pub mod response;
pub mod router;
#[cfg(feature = "validation")]
pub mod validate;
#[cfg(feature = "views")]
pub mod views;

// Pinned escape hatches: framework-compatible versions usable without extra
// dependencies of your own (cargo unifies these with yours when present).
pub use axum;
pub use serde;
pub use tokio;

pub use app::Application;
#[cfg(feature = "auth")]
pub use auth::{
    clear_session_cookie, hash_password, login, logout, session_cookie, verify_password,
    CurrentUser, MemorySessionStore, Session, SessionStore, SESSION_COOKIE,
};
pub use config::Config;
pub use container::Container;
pub use error::{AppError, ErrorDocument, ErrorObject, ErrorSource, Result, ValidationError};
pub use extract::{Json, Path, Query, State};
pub use log::Log;
pub use middleware::{from_fn, MiddlewareRegistry, Next, Request};
pub use provider::{async_trait, ServiceProvider};
pub use response::{created, json, no_content, ok, redirect, see_other, Response};
pub use router::{delete, get, patch, post, put, serve, Router, StatusCode};
#[cfg(feature = "validation")]
pub use validate::{field_errors, validate, Validated, ValidatedQuery};
#[cfg(feature = "validation")]
pub use validator::Validate;
#[cfg(feature = "views")]
pub use views::{Template, View};
