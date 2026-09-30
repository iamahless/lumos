//! The blog tour: the whole product over an in-memory database —
//! login/logout, post CRUD, comments, validation, auth splits,
//! negotiation, and collection queries.

use blog::database::migrations::{CreateComments, CreatePosts, CreateUsers};
use blog::database::seeders::DemoSeeder;
use lumos::rusticate::{Migrator, Seeder, DB};
use lumos::{Router, StatusCode};
use tower::ServiceExt;

const VND: &str = "application/vnd.api+json";

async fn setup() -> Router {
    let db = DB::memory().await.unwrap();
    Migrator::new(&db)
        .run(&[&CreateUsers, &CreatePosts, &CreateComments])
        .await
        .unwrap();
    DemoSeeder.run(&db).await.unwrap();
    blog::build(db).unwrap().0
}

fn vnd(body: &str) -> lumos::axum::body::Body {
    lumos::axum::body::Body::from(body.to_string())
}

async fn body_json(response: lumos::Response) -> serde_json::Value {
    let bytes = lumos::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post_document(title: &str, body: &str) -> String {
    format!(
        "{{\"data\":{{\"type\":\"posts\",\"attributes\":{{\"title\":{title:?},\"body\":{body:?}}}}}}}"
    )
}

/// Logs in, returning the raw `Set-Cookie` value for later requests.
async fn login(app: &Router, email: &str, password: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(vnd(&format!(
                    "{{\"email\":{email:?},\"password\":{password:?}}}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn health_answers_without_auth() {
    let response = setup()
        .await
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/health")
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn login_rejects_bad_credentials_and_shapes() {
    let app = setup().await;

    // Wrong password → 401.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(vnd(r#"{"email":"ada@example.com","password":"wrong"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Unknown email → 401 (same status, no enumeration).
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(vnd(r#"{"email":"nobody@example.com","password":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Malformed email → 422 with a field pointer.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(vnd(r#"{"email":"not-an-email","password":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(
        json["errors"][0]["source"]["pointer"],
        "/data/attributes/email"
    );
}

#[tokio::test]
async fn login_logout_round_trip_kills_the_cookie() {
    let app = setup().await;
    let cookie = login(&app, "ada@example.com", "password").await;
    assert!(cookie.contains("session="));

    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("DELETE")
                .uri("/sessions")
                .header("cookie", &cookie)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // The cookie is dead now.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/posts")
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &cookie)
                .body(vnd(&post_document("Late", "Too late.")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn posts_crud_tour() {
    let app = setup().await;
    let cookie = login(&app, "ada@example.com", "password").await;

    // Create → 201 + Location.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/posts")
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &cookie)
                .body(vnd(&post_document("Tour post", "Written on tour.")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let location = response.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with("/posts/"));
    let json = body_json(response).await;
    assert_eq!(json["data"]["type"], "posts");
    assert_eq!(json["data"]["attributes"]["title"], "Tour post");
    let id = json["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(location, format!("/posts/{id}"));

    // Read back.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri(format!("/posts/{id}"))
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Replace → 200.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("PATCH")
                .uri(format!("/posts/{id}"))
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &cookie)
                .body(vnd(&format!(
                    "{{\"data\":{{\"type\":\"posts\",\"id\":\"{id}\",\"attributes\":{{\"title\":\"Edited\",\"body\":\"Edited body.\"}}}}}}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["attributes"]["title"], "Edited");

    // Body id disagreeing with the URL → 409.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("PATCH")
                .uri(format!("/posts/{id}"))
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &cookie)
                .body(vnd(
                    r#"{"data":{"type":"posts","id":"999","attributes":{"title":"X","body":"Y"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Delete → 204, then 404.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/posts/{id}"))
                .header("accept", VND)
                .header("cookie", &cookie)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri(format!("/posts/{id}"))
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn writes_need_a_session_and_edits_need_ownership() {
    let app = setup().await;

    // Anonymous write → 401.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/posts")
                .header("accept", VND)
                .header("content-type", VND)
                .body(vnd(&post_document("Anon", "Nope.")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Seeded post 1 belongs to Ada (id 2); admin (id 1) may not touch it.
    let admin = login(&app, "admin@example.com", "password").await;
    for (method, status) in [
        ("PATCH", StatusCode::FORBIDDEN),
        ("DELETE", StatusCode::FORBIDDEN),
    ] {
        let response = app
            .clone()
            .oneshot(
                lumos::axum::http::Request::builder()
                    .method(method)
                    .uri("/posts/1")
                    .header("accept", VND)
                    .header("content-type", VND)
                    .header("cookie", &admin)
                    .body(vnd(&post_document("Hijack", "Not mine.")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{method}");
    }
}

#[tokio::test]
async fn invalid_post_attributes_fail_422() {
    let app = setup().await;
    let cookie = login(&app, "ada@example.com", "password").await;

    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/posts")
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &cookie)
                .body(vnd(&post_document("", "")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    let pointers: Vec<&str> = json["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["source"]["pointer"].as_str().unwrap())
        .collect();
    assert!(pointers.contains(&"/data/attributes/title"));
    assert!(pointers.contains(&"/data/attributes/body"));
}

#[tokio::test]
async fn negotiation_guards_stay_strict() {
    let app = setup().await;

    // Hostile Accept → 406 (an absent header means */* and passes).
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/posts")
                .header("accept", "text/html")
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);

    // Wrong Content-Type on write → 415.
    let cookie = login(&app, "ada@example.com", "password").await;
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/posts")
                .header("accept", VND)
                .header("content-type", "application/json")
                .header("cookie", &cookie)
                .body(vnd(&post_document("Hi", "There.")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn collections_filter_sort_page_and_sparse() {
    let app = setup().await;

    // Seeded: Ada (id 2) owns post 1, admin (id 1) owns post 2.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/posts?filter[user_id]=2")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"][0]["id"], "1");

    // Sort newest-first + second page of size 1 → post 1.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/posts?sort=-id&page[size]=1&page[number]=2")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"][0]["id"], "1");
    assert!(json["links"]["last"]
        .as_str()
        .unwrap()
        .contains("page[number]=2"));

    // Sparse fieldsets + author include.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/posts?fields[posts]=title&include=author")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let attributes = &json["data"][0]["attributes"];
    assert!(attributes.get("title").is_some());
    assert!(attributes.get("body").is_none());
    let included = json["included"].as_array().unwrap();
    assert!(included
        .iter()
        .any(|item| item["type"] == "users" && item["attributes"].get("password_hash").is_none()));
}

#[tokio::test]
async fn comments_belong_to_posts_and_authors() {
    let app = setup().await;
    let ada = login(&app, "ada@example.com", "password").await;

    // Comment on post 1 → 201.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/comments")
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &ada)
                .body(vnd(
                    r#"{"data":{"type":"comments","attributes":{"post_id":1,"body":"Nice post!"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    let id = json["data"]["id"].as_str().unwrap().to_string();

    // Filter by post.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/comments?filter[post_id]=1")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["data"].as_array().unwrap().len() >= 2);

    // Comments are immutable: no PATCH route → 405.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("PATCH")
                .uri(format!("/comments/{id}"))
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &ada)
                .body(vnd(
                    r#"{"data":{"type":"comments","attributes":{"post_id":1,"body":"Edited"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    // A stranger may not delete it; the author may.
    let admin = login(&app, "admin@example.com", "password").await;
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/comments/{id}"))
                .header("accept", VND)
                .header("cookie", &admin)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/comments/{id}"))
                .header("accept", VND)
                .header("cookie", &ada)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // Commenting on a missing post → 404.
    let response = app
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/comments")
                .header("accept", VND)
                .header("content-type", VND)
                .header("cookie", &ada)
                .body(vnd(
                    r#"{"data":{"type":"comments","attributes":{"post_id":999,"body":"Ghost"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_post_renders_jsonapi_404() {
    let response = setup()
        .await
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/posts/999")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["content-type"], VND);
    let json = body_json(response).await;
    assert_eq!(json["errors"][0]["status"], "404");
}
