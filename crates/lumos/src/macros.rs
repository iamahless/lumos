//! Route-building macros: `routes!` and `group!`.
//!
//! These live in the facade (not `lumos-core`) so `$crate` paths resolve to
//! `lumos`, which every application depends on directly.

/// Builds a router from resources and plain routes, in written order.
///
/// Each item is either a `resource(...)` mount — delegating to the
/// controller's generated `routes()` constructor — or a `route(...)` pair of
/// a path and an axum method router:
///
/// ```rust
/// use lumos::{get, routes};
///
/// async fn health() -> &'static str {
///     "ok"
/// }
///
/// // (The annotation pins the router state; `mount`/`serve` do this in real apps.)
/// let router: lumos::Router = routes! {
///     route("/health", get(health)),
/// };
/// ```
///
/// With a controller (see `#[controller]` for the full setup):
///
/// ```ignore
/// let users = std::sync::Arc::new(UserController::from_container(app.container())?);
/// let api = routes! {
///     resource("/users", UserController, users),
///     route("/health", get(health)),
/// };
/// ```
#[macro_export]
macro_rules! routes {
    (@one $router:ident, resource($path:expr, $controller:ty, $instance:expr)) => {
        $router = $router.nest($path, <$controller>::routes($instance));
    };
    (@one $router:ident, route($path:expr, $method:expr)) => {
        $router = $router.route($path, $method);
    };
    ($( $kind:ident $args:tt ),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut __router = $crate::Router::new();
        $( $crate::routes!(@one __router, $kind $args); )*
        __router
    }};
}

/// Nests a router under a prefix and layers middleware over it.
///
/// Layers apply in written order (each `.layer` wraps the previous ones).
/// Any axum layer works: `from_fn` middleware, tower layers, or closures
/// from [`MiddlewareRegistry`](crate::MiddlewareRegistry) — though the
/// registry's [`apply`](crate::MiddlewareRegistry::apply) is usually clearer
/// for named middleware.
///
/// # Examples
///
/// ```rust
/// use lumos::{from_fn, get, group, Router};
///
/// async fn tag(request: lumos::Request, next: lumos::Next) -> lumos::Response {
///     next.run(request).await
/// }
///
/// let inner = Router::new().route("/ping", get(|| async { "pong" }));
/// // (The annotation pins the router state; `mount`/`serve` do this in real apps.)
/// let router: lumos::Router = group!("/admin", inner, [from_fn(tag)]);
/// ```
#[macro_export]
macro_rules! group {
    ($prefix:expr, $router:expr, [$($layer:expr),* $(,)?]) => {{
        #[allow(unused_mut)]
        let mut __router = $crate::Router::new().nest($prefix, $router);
        $( __router = __router.layer($layer); )*
        __router
    }};
}
