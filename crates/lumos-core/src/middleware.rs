//! Middleware: `tower` composition with a named registry.
//!
//! Custom middleware is an `async fn` over [`Request`] and [`Next`],
//! adapted with [`from_fn`]:
//!
//! ```rust
//! use lumos_core::{from_fn, Next, Request, Response, Router};
//!
//! async fn tag(request: Request, next: Next) -> Response {
//!     next.run(request).await
//! }
//!
//! let _router: Router = Router::new().layer(from_fn(tag));
//! ```
//!
//! [`MiddlewareRegistry`] gives those layers names (`"auth"`, `"throttle"`)
//! so providers register them once and routes apply them by name. Applying
//! an unknown name is an error, never a silent skip.

pub use axum::extract::Request;
pub use axum::middleware::{from_fn, Next};
pub use tower::{Layer, Service};

use std::collections::HashMap;
use std::sync::Arc;

use crate::{AppError, Result, Router};

/// A type-erased router transform: `|router| router.layer(...)`.
pub type RouteTransform = Arc<dyn Fn(Router) -> Router + Send + Sync>;

/// Named middleware registry.
///
/// Providers register layers under `'static` names; routes (or groups)
/// apply them by name. Names keep `routes/api.rs` readable while the
/// registry keeps construction in one place.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{from_fn, MiddlewareRegistry, Router};
///
/// let mut registry = MiddlewareRegistry::new();
/// registry.register("noop", |router| router.layer(from_fn(
///     |request: lumos_core::Request, next: lumos_core::Next| async move {
///         next.run(request).await
///     },
/// )));
///
/// let router = registry.apply(Router::new(), &["noop"]).unwrap();
/// assert!(registry.apply(Router::new(), &["missing"]).is_err());
/// ```
#[derive(Clone, Default)]
pub struct MiddlewareRegistry {
    entries: HashMap<&'static str, RouteTransform>,
}

impl MiddlewareRegistry {
    /// Creates an empty registry.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::MiddlewareRegistry;
    ///
    /// let registry = MiddlewareRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `transform` under `name`, replacing any previous entry.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::MiddlewareRegistry;
    ///
    /// let mut registry = MiddlewareRegistry::new();
    /// registry.register("passthrough", |router| router);
    /// assert!(!registry.is_empty());
    /// ```
    pub fn register(
        &mut self,
        name: &'static str,
        transform: impl Fn(Router) -> Router + Send + Sync + 'static,
    ) -> &mut Self {
        let erased: RouteTransform = Arc::new(transform);
        self.entries.insert(name, erased);
        self
    }

    /// Applies the named layers to `router`, in order.
    ///
    /// An unknown name is an [`AppError::Internal`] naming the offender:
    /// middleware must never silently vanish.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{MiddlewareRegistry, Router};
    ///
    /// let mut registry = MiddlewareRegistry::new();
    /// registry.register("a", |router| router).register("b", |router| router);
    /// let _router = registry.apply(Router::new(), &["a", "b"]).unwrap();
    /// ```
    pub fn apply(&self, mut router: Router, names: &[&str]) -> Result<Router> {
        for name in names {
            let transform = self
                .entries
                .get(*name)
                .ok_or_else(|| AppError::internal(format!("unknown middleware '{name}'")))?;
            router = transform(router);
        }
        Ok(router)
    }

    /// Returns `true` when no middleware is registered.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::MiddlewareRegistry;
    ///
    /// assert!(MiddlewareRegistry::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Debug for MiddlewareRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&&str> = self.entries.keys().collect();
        names.sort();
        formatter
            .debug_struct("MiddlewareRegistry")
            .field("names", &names)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_middleware_names_the_offender() {
        let registry = MiddlewareRegistry::new();
        let error = registry.apply(Router::new(), &["auth"]).unwrap_err();
        assert!(error.to_string().contains("auth"));
    }

    #[test]
    fn debug_lists_names() {
        let mut registry = MiddlewareRegistry::new();
        registry
            .register("b", |router| router)
            .register("a", |router| router);
        assert!(format!("{registry:?}").contains(r#"["a", "b"]"#));
    }
}
