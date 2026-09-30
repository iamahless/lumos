//! JSON:API serialization and content negotiation for Lumos.
//!
//! # Status
//!
//! Scaffolded in Phase 1. The full implementation lands in Phase 4:
//! `JsonApiResource` derive, content negotiation (406/415 + `Vary`),
//! `include` / sparse fieldsets / sorting / filtering / pagination,
//! spec-compliant pagination links, and error documents.

#[cfg(all(feature = "jsonapi", feature = "jsonapi-lite"))]
compile_error!(
    "lumos-jsonapi features `jsonapi` and `jsonapi-lite` are mutually exclusive; enable only one."
);
