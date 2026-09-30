//! Test helpers for Lumos applications: migrated test databases,
//! an in-process HTTP client with a cookie jar, and chainable response
//! assertions.
//!
//! ```rust,no_run
//! use lumos_testing::{TestClient, TestDb};
//!
//! # #[tokio::main]
//! # async fn main() -> rusticate::Result<()> {
//! let db = TestDb::memory(&[]).await?;
//! db.reset(&[]).await?;
//! let client = TestClient::new(lumos_core::Router::new());
//! client.get("/health").await.assert_not_found();
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

pub mod client;
pub mod db;
pub mod response;

pub use client::{TestClient, TestRequest, APPLICATION_JSON, VND_API_JSON};
pub use db::TestDb;
pub use response::TestResponse;

// `factory!` lives in rusticate (fixture builders predate this crate);
// re-exported so test suites import one crate.
pub use rusticate::factory;
