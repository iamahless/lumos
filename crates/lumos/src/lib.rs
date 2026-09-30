//! Lumos: a lightweight, MVC, batteries-included Rust web framework.
//!
//! Lumos includes routing, request/response handling, and error handling by default, with
//! every other capability behind an opt-in feature flag:
//!
//! | Feature        | Enables                                              |
//! |----------------|------------------------------------------------------|
//! | `orm`          | Rusticate ORM (`rusticate` crate)                    |
//! | `migrations`   | ORM migrations (implies `orm`)                       |
//! | `validation`   | `Validated` extractors + `Validate` derive (→ 422) |
//! | `auth`         | Argon2 passwords, token sessions, guards (→ `validation`) |
//! | `views`        | Askama templates via the `View` responder            |
//! | `cache`        | Cache drivers                                        |
//! | `queue`        | Queue drivers (v0.2)                                 |
//! | `http-cache`   | ETag / cache-control middleware                      |
//! | `jsonapi`      | Full JSON:API v1.1 (implies `orm`)                   |
//! | `jsonapi-lite` | Simplified JSON:API (implies `orm`)                  |
//!
//! `jsonapi` and `jsonapi-lite` are mutually exclusive: enabling both is a
//! compile error. API routes get JSON:API negotiation; web routes never do.
//!
//! # Example
//!
//! ```rust,no_run
//! use lumos::{get, Router, serve};
//!
//! async fn hello() -> &'static str {
//!     "Hello, Lumos!"
//! }
//!
//! #[tokio::main]
//! async fn main() -> lumos::Result<()> {
//!     serve(Router::new().route("/", get(hello)), "127.0.0.1:3000").await
//! }
//! ```

#![warn(missing_docs)]

#[cfg(all(feature = "jsonapi", feature = "jsonapi-lite"))]
compile_error!(
    "lumos features `jsonapi` and `jsonapi-lite` are mutually exclusive; enable only one."
);

mod macros;

// --- Core kernel (always on) -----------------------------------------------
pub use lumos_core::{
    async_trait, created, delete, from_fn, get, json, no_content, ok, patch, post, put, redirect,
    see_other, serve, AppError, Application, Config, Container, ErrorDocument, ErrorObject,
    ErrorSource, Json, Log, MiddlewareRegistry, Next, Path, Query, Request, Response, Result,
    RouteEntry, RouteRegistry, Router, ServiceProvider, State, StatusCode, ValidationError,
};
// Pinned escape hatches: framework-compatible upstream versions.
pub use lumos_core::{axum, serde, tokio};
// Compile-time codegen.
pub use lumos_macros::controller;
pub use lumos_macros::JsonApiResource;

// --- Opt-in features --------------------------------------------------------
#[cfg(feature = "orm")]
pub use rusticate;

#[cfg(feature = "auth")]
pub use lumos_core::{
    clear_session_cookie, hash_password, login, logout, session_cookie, verify_password,
    CurrentUser, MemorySessionStore, Session, SessionStore, SESSION_COOKIE,
};
#[cfg(feature = "validation")]
pub use lumos_core::{field_errors, validate, Validate, Validated, ValidatedQuery};
#[cfg(feature = "views")]
pub use lumos_core::{Template, View};

#[cfg(any(feature = "jsonapi", feature = "jsonapi-lite"))]
pub use lumos_jsonapi;

// JSON:API surface at root (either flag; renderers come from the active
// one). `created` is the deliberate exception: core's plain-JSON `created`
// already lives here, so the JSON:API one stays module-only at
// `lumos_jsonapi::created`.
#[cfg(any(feature = "jsonapi", feature = "jsonapi-lite"))]
pub use lumos_jsonapi::{
    collection, collection_memory, encode, memory_page, single, to_attribute, ApiQuery, ApiResult,
    AttributeMap, DynResource, Filter, JsonApiBody, JsonApiError, NamedRelationship, Page,
    PageLinks, RelationTarget, Relationship, Resource, SortField, ToMany, ToOne, DEFAULT_PAGE_SIZE,
    JSON_API_MIME, MAX_BODY_BYTES, MAX_PAGE_SIZE,
};
