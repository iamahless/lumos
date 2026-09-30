//! Lumos: a lightweight, MVC, batteries-included Rust web framework.
//!
//! Minimal by default — routing, request/response, and error handling — with
//! every other capability behind an opt-in feature flag:
//!
//! | Feature        | Enables                                              |
//! |----------------|------------------------------------------------------|
//! | `orm`          | Rusticate ORM (`rusticate` crate)                    |
//! | `migrations`   | ORM migrations (implies `orm`)                       |
//! | `validation`   | Form-request validation (Phase 3)                    |
//! | `auth`         | Session + JWT + policies (Phase 3, implies `validation`) |
//! | `views`        | Blade-like templates (Phase 3)                       |
//! | `cache`        | Cache drivers                                        |
//! | `queue`        | Queue drivers (v0.2)                                 |
//! | `http-cache`   | ETag / cache-control middleware                      |
//! | `jsonapi`      | Full JSON:API v1.1 (Phase 4, implies `orm`)          |
//! | `jsonapi-lite` | Simplified JSON:API (Phase 4, implies `orm`)         |
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
    Router, ServiceProvider, State, StatusCode, ValidationError,
};
// Pinned escape hatches: framework-compatible upstream versions.
pub use lumos_core::{axum, serde, tokio};
// Compile-time codegen.
pub use lumos_macros::controller;

// --- Opt-in features --------------------------------------------------------
#[cfg(feature = "orm")]
pub use rusticate;

#[cfg(any(feature = "jsonapi", feature = "jsonapi-lite"))]
pub use lumos_jsonapi;
