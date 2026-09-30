//! JSON:API serialization and content negotiation for Lumos.
//!
//! Apps define [`Resource`]s (by hand or via `JsonApiResource`), take
//! [`ApiQuery`] in handlers, and render with `single` / `collection` /
//! `collection_memory` / `created`. Opted-in routes negotiate strictly:
//! `Accept` must allow `application/vnd.api+json` (406 otherwise),
//! write bodies must declare it (415 otherwise), and every response —
//! errors included — carries the JSON:API content type plus `Vary: Accept`.
//!
//! # Feature flags
//!
//! `jsonapi` renders full JSON:API v1.1 (linkage, deduped compound
//! `included`, sparse fieldsets, sort/filter/page, spec pagination links).
//! `jsonapi-lite` renders the simplified subset (flat documents, no
//! `include` / `fields`). The two are mutually exclusive (compile error);
//! the trait, query parsing, negotiation, and bodies are shared.
//!
//! # Example
//!
//! Renderers live behind the `jsonapi` / `jsonapi-lite` flags, so the example
//! below only builds when one is active (nothing to show without a renderer).
//!
//! ```rust,no_run
//! # #[cfg(any(feature = "jsonapi", feature = "jsonapi-lite"))]
//! # fn main() {
//! use lumos_jsonapi::{collection_memory, ApiQuery, AttributeMap, NamedRelationship, Resource};
//!
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//!
//! impl Resource for User {
//!     const TYPE: &'static str = "users";
//!     fn resource_id(&self) -> String {
//!         self.id.to_string()
//!     }
//!     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
//!         let mut map = AttributeMap::new();
//!         map.insert("name".to_string(), self.name.clone().into());
//!         Ok(map)
//!     }
//!     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
//!         Vec::new()
//!     }
//! }
//!
//! async fn index(query: ApiQuery) -> lumos_jsonapi::ApiResult<lumos_core::Response> {
//!     let users = vec![User { id: 1, name: "Ada".to_string() }];
//!     collection_memory(&users, &query)
//! }
//! # }
//! # #[cfg(not(any(feature = "jsonapi", feature = "jsonapi-lite")))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]

#[cfg(all(feature = "jsonapi", feature = "jsonapi-lite"))]
compile_error!(
    "lumos-jsonapi features `jsonapi` and `jsonapi-lite` are mutually exclusive; enable only one."
);

pub mod negotiate;
pub mod query;
pub mod resource;

/// Core result alias, re-exported for generated derive code.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::Result;
///
/// let ok: Result<u32> = Ok(1);
/// assert_eq!(ok.unwrap(), 1);
/// ```
pub use lumos_core::Result;
#[cfg(feature = "jsonapi")]
pub mod document;
#[cfg(feature = "jsonapi-lite")]
pub mod lite;

#[cfg(feature = "jsonapi")]
pub use document::{collection, collection_memory, created, single};
#[cfg(feature = "jsonapi-lite")]
pub use lite::{collection, collection_memory, created, single};
pub use negotiate::{ApiResult, JsonApiBody, JsonApiError, JSON_API_MIME, MAX_BODY_BYTES};
pub use query::{
    encode, memory_page, ApiQuery, Filter, Page, PageLinks, SortField, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};
pub use resource::{
    to_attribute, AttributeMap, DynResource, NamedRelationship, RelationTarget, Relationship,
    Resource, ToMany, ToOne,
};
