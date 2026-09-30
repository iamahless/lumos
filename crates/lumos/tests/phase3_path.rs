//! The Phase 3 path: validation → 422, session auth → 401/403, views → HTML.
//! All three features exercised end-to-end through facade paths and a real
//! router (`auth` implies `validation`, so two gates cover all three).

#![cfg(all(feature = "auth", feature = "views"))]

use lumos::axum::Extension;
use lumos::{
    hash_password, login, logout, post, verify_password, CurrentUser, MemorySessionStore, Router,
    SessionStore, StatusCode, Template, Validate, Validated, View,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

#[derive(Deserialize, Validate)]
struct NewUser {
    #[validate(length(min = 1, max = 80))]
    name: String,
    #[validate(email)]
    email: String,
}

async fn store(Validated(input): Validated<NewUser>) -> String {
    format!("hello {}", input.name)
}

#[tokio::test]
async fn invalid_input_renders_422_with_field_pointers() {
    let app = Router::new().route("/users", post(store));

    let response = app
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/users")
                .header("content-type", "application/json")
                .body(lumos::axum::body::Body::from(
                    r#"{"name":"","email":"nope"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = lumos::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["errors"][0]["code"], "validation_failed");
    assert_eq!(
        json["errors"][0]["source"]["pointer"],
        "/data/attributes/email"
    );
    assert_eq!(
        json["errors"][1]["source"]["pointer"],
        "/data/attributes/name"
    );
}

#[tokio::test]
async fn password_round_trip_through_facade() {
    let hash = hash_password("correct horse").unwrap();
    assert!(verify_password("correct horse", &hash));
    assert!(!verify_password("wrong horse", &hash));
}

async fn me(user: CurrentUser) -> String {
    user.0.user_id.clone()
}

async fn admin(user: CurrentUser) -> Result<String, lumos::AppError> {
    user.require("admin")?;
    Ok("welcome".to_string())
}

#[tokio::test]
async fn login_guard_logout_with_401_403_split() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let app = Router::new()
        .route("/me", post(me))
        .route("/admin", post(admin))
        .layer(Extension(Arc::clone(&store)));

    // Anonymous → 401.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/me")
                .body(lumos::axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Login → guard passes.
    let session = login(
        store.as_ref(),
        "user-1",
        vec!["read".to_string()],
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/me")
                .header("authorization", format!("Bearer {}", session.token))
                .body(lumos::axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = lumos::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, "user-1");

    // Authenticated without the scope → 403, not 401.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/admin")
                .header("authorization", format!("Bearer {}", session.token))
                .body(lumos::axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Logout revokes: the same token is now 401.
    logout(store.as_ref(), &session.token).await.unwrap();
    let response = app
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/me")
                .header("authorization", format!("Bearer {}", session.token))
                .body(lumos::axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[derive(Template)]
#[template(source = "<h1>Hello, {{ name }}!</h1>", ext = "html")]
struct Hello<'a> {
    name: &'a str,
}

async fn hello() -> View<Hello<'static>> {
    View(Hello { name: "Ada" })
}

#[tokio::test]
async fn template_renders_html_through_facade() {
    let app = Router::new().route("/hello", post(hello));
    let response = app
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/hello")
                .body(lumos::axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/html; charset=utf-8"
    );
    let body = lumos::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, "<h1>Hello, Ada!</h1>");
}
