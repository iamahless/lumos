//! Blog application: models, resources, controllers, routes, registry.
//!
//! [`build`] wires everything from one database handle: the container
//! resolves controller fields, `routes!` mounts the routers, and the
//! registry mirrors every mount so `route:list` stays truthful.

pub mod controllers;
pub mod database;
pub mod models;
pub mod resources;

use std::sync::Arc;

use controllers::comments::CommentsController;
use controllers::posts::PostsController;
use controllers::sessions::SessionsController;
use lumos::rusticate::DB;
use lumos::{
    routes, Container, MemorySessionStore, RouteRegistry, Router, SessionStore, Validated,
};

use crate::resources::LoginInput;

/// Builds the router and its registry side by side.
///
/// The session store lives in memory: sessions vanish on restart, which
/// is fine for the example (a production app binds a Redis store here).
pub fn build(db: DB) -> lumos::Result<(Router, RouteRegistry)> {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let mut container = Container::new();
    container
        .singleton_value(db)
        .singleton_value(Arc::clone(&store));

    let posts = Arc::new(PostsController::from_container(&container)?);
    let comments = Arc::new(CommentsController::from_container(&container)?);
    let sessions = Arc::new(SessionsController::from_container(&container)?);

    let login = {
        let sessions = Arc::clone(&sessions);
        move |input: Validated<LoginInput>| {
            let sessions = Arc::clone(&sessions);
            async move { sessions.login(input).await }
        }
    };
    let logout = {
        let sessions = Arc::clone(&sessions);
        move |user: lumos::CurrentUser| {
            let sessions = Arc::clone(&sessions);
            async move { sessions.logout(user).await }
        }
    };

    let router: Router = routes! {
        resource("/posts", PostsController, posts),
        resource("/comments", CommentsController, comments),
        route("/sessions", lumos::post(login)),
        route("/health", lumos::get(health)),
    };
    let router = router
        .route("/sessions", lumos::delete(logout))
        .layer(lumos::axum::Extension(store));

    let mut registry = RouteRegistry::new();
    registry.resource("/posts", PostsController::route_entries());
    registry.resource("/comments", CommentsController::route_entries());
    registry.route("POST", "/sessions", "SessionsController::login");
    registry.route("DELETE", "/sessions", "SessionsController::logout");
    registry.route("GET", "/health", "health");

    Ok((router, registry))
}

/// Liveness probe: no database, no auth.
async fn health() -> &'static str {
    "ok"
}

/// Lifts an ORM result into an API result, mapping ORM errors onto
/// their HTTP statuses first (404/409/…) so JSON:API renders them as
/// error documents instead of 500s.
pub(crate) fn into_api<T>(result: Result<T, lumos::rusticate::Error>) -> lumos::ApiResult<T> {
    result.map_err(|error| lumos::JsonApiError::from(lumos::AppError::from(error)))
}
