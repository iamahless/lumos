//! These tests exercise the blog over an in-memory database. They cover
//! login/logout, post CRUD, comments, validation, auth splits,
//! negotiation, and collection queries.

use blog::database::migrations::{CreateComments, CreatePosts, CreateUsers};
use blog::database::seeders::DemoSeeder;
use lumos::StatusCode;
use lumos_testing::{TestClient, TestDb, TestResponse};

async fn setup() -> TestClient {
    let db = TestDb::memory(&[&CreateUsers, &CreatePosts, &CreateComments])
        .await
        .unwrap();
    db.seed(&[&DemoSeeder]).await.unwrap();
    TestClient::new(blog::build(db.db().clone()).unwrap().0)
}

fn post_document(title: &str, body: &str) -> serde_json::Value {
    serde_json::json!({
        "data": { "type": "posts", "attributes": { "title": title, "body": body } }
    })
}

/// Logs in on a client sharing the app; the jar carries the session.
async fn login(client: &TestClient, email: &str, password: &str) -> TestResponse {
    client
        .post_json(
            "/sessions",
            &serde_json::json!({ "email": email, "password": password }),
        )
        .await
}

#[tokio::test]
async fn health_answers_without_auth() {
    setup().await.get("/health").await.assert_ok();
}

#[tokio::test]
async fn login_rejects_bad_credentials_and_shapes() {
    let client = setup().await;

    // Wrong password → 401.
    login(&client, "ada@example.com", "wrong")
        .await
        .assert_unauthorized();

    // Unknown email → 401 (same status, no enumeration).
    login(&client, "nobody@example.com", "x")
        .await
        .assert_unauthorized();

    // Malformed email → 422 with a field pointer.
    login(&client, "not-an-email", "x")
        .await
        .assert_unprocessable()
        .assert_error_pointer("/data/attributes/email");
}

#[tokio::test]
async fn login_logout_round_trip_kills_the_cookie() {
    let client = setup().await;
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();
    assert!(client.cookie("lumos_session").is_some());

    client.delete("/sessions").await.assert_no_content();

    // The cookie is dead now.
    client
        .post_vnd("/posts", &post_document("Late", "Too late."))
        .await
        .assert_unauthorized();
}

#[tokio::test]
async fn posts_crud_tour() {
    let client = setup().await;
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();

    // Create → 201 + Location.
    let response = client
        .post_vnd("/posts", &post_document("Tour post", "Written on tour."))
        .await;
    response.assert_created().assert_data("posts", "3");
    let id = response.json().unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        response.header("location"),
        Some(format!("/posts/{id}").as_str())
    );

    // Read back.
    client
        .request("GET", &format!("/posts/{id}"))
        .header("accept", lumos_testing::VND_API_JSON)
        .send(&client)
        .await
        .assert_ok();

    // Replace → 200.
    client
        .patch_vnd(
            &format!("/posts/{id}"),
            &serde_json::json!({
                "data": {
                    "type": "posts",
                    "id": id,
                    "attributes": { "title": "Edited", "body": "Edited body." }
                }
            }),
        )
        .await
        .assert_ok()
        .assert_data("posts", &id);
    let response = client
        .request("GET", &format!("/posts/{id}"))
        .header("accept", lumos_testing::VND_API_JSON)
        .send(&client)
        .await;
    assert_eq!(
        response.json().unwrap()["data"]["attributes"]["title"],
        "Edited"
    );

    // Body id disagreeing with the URL → 409.
    client
        .patch_vnd(
            &format!("/posts/{id}"),
            &serde_json::json!({
                "data": {
                    "type": "posts",
                    "id": "999",
                    "attributes": { "title": "X", "body": "Y" }
                }
            }),
        )
        .await
        .assert_status(StatusCode::CONFLICT);

    // Delete → 204, then 404.
    client
        .delete(&format!("/posts/{id}"))
        .await
        .assert_no_content();
    client
        .request("GET", &format!("/posts/{id}"))
        .header("accept", lumos_testing::VND_API_JSON)
        .send(&client)
        .await
        .assert_not_found();
}

#[tokio::test]
async fn writes_need_a_session_and_edits_need_ownership() {
    let client = setup().await;

    // Anonymous write → 401.
    client
        .post_vnd("/posts", &post_document("Anon", "Nope."))
        .await
        .assert_unauthorized();

    // Seeded post 1 belongs to Ada (id 2); admin (id 1) may not touch it.
    login(&client, "admin@example.com", "password")
        .await
        .assert_ok();
    client
        .patch_vnd("/posts/1", &post_document("Hijack", "Not mine."))
        .await
        .assert_forbidden();
    client.delete("/posts/1").await.assert_forbidden();
}

#[tokio::test]
async fn invalid_post_attributes_fail_422() {
    let client = setup().await;
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();

    let response = client.post_vnd("/posts", &post_document("", "")).await;
    response
        .assert_unprocessable()
        .assert_error_pointer("/data/attributes/title")
        .assert_error_pointer("/data/attributes/body");
}

#[tokio::test]
async fn negotiation_guards_stay_strict() {
    let client = setup().await;

    // Hostile Accept → 406 (an absent header means */* and passes).
    client
        .request("GET", "/posts")
        .header("accept", "text/html")
        .send(&client)
        .await
        .assert_status(StatusCode::NOT_ACCEPTABLE);

    // Wrong Content-Type on write → 415.
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();
    client
        .request("POST", "/posts")
        .header("accept", lumos_testing::VND_API_JSON)
        .header("content-type", "application/json")
        .json(&post_document("Hi", "There."))
        .send(&client)
        .await
        .assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn collections_filter_sort_page_and_sparse() {
    let client = setup().await;
    let vnd = lumos_testing::VND_API_JSON;

    // Seeded: Ada (id 2) owns post 1, admin (id 1) owns post 2.
    let response = client
        .request("GET", "/posts?filter[user_id]=2")
        .header("accept", vnd)
        .send(&client)
        .await;
    response.assert_ok();
    let json = response.json().unwrap();
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"][0]["id"], "1");

    // Sort newest-first + second page of size 1 → post 1.
    let response = client
        .request("GET", "/posts?sort=-id&page[size]=1&page[number]=2")
        .header("accept", vnd)
        .send(&client)
        .await;
    response.assert_ok();
    let json = response.json().unwrap();
    assert_eq!(json["data"][0]["id"], "1");
    assert!(json["links"]["last"]
        .as_str()
        .unwrap()
        .contains("page[number]=2"));

    // Sparse fieldsets + author include.
    let response = client
        .request("GET", "/posts?fields[posts]=title&include=author")
        .header("accept", vnd)
        .send(&client)
        .await;
    response.assert_ok();
    let json = response.json().unwrap();
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
    let client = setup().await;
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();

    // Comment on post 1 → 201.
    let response = client
        .post_vnd(
            "/comments",
            &serde_json::json!({
                "data": {
                    "type": "comments",
                    "attributes": { "post_id": 1, "body": "Nice post!" }
                }
            }),
        )
        .await;
    response.assert_created();
    let id = response.json().unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Filter by post.
    let response = client
        .request("GET", "/comments?filter[post_id]=1")
        .header("accept", lumos_testing::VND_API_JSON)
        .send(&client)
        .await;
    response.assert_ok();
    assert!(response.json().unwrap()["data"].as_array().unwrap().len() >= 2);

    // Comments are immutable: no PATCH route → 405.
    client
        .patch_vnd(
            &format!("/comments/{id}"),
            &serde_json::json!({
                "data": {
                    "type": "comments",
                    "attributes": { "post_id": 1, "body": "Edited" }
                }
            }),
        )
        .await
        .assert_status(StatusCode::METHOD_NOT_ALLOWED);

    // A stranger may not delete it (re-login as admin on the same
    // client, after clearing the jar); the author may.
    client.clear_cookies();
    login(&client, "admin@example.com", "password")
        .await
        .assert_ok();
    client
        .delete(&format!("/comments/{id}"))
        .await
        .assert_forbidden();

    client.clear_cookies();
    login(&client, "ada@example.com", "password")
        .await
        .assert_ok();
    client
        .delete(&format!("/comments/{id}"))
        .await
        .assert_no_content();

    // Commenting on a missing post → 404.
    client
        .post_vnd(
            "/comments",
            &serde_json::json!({
                "data": {
                    "type": "comments",
                    "attributes": { "post_id": 999, "body": "Ghost" }
                }
            }),
        )
        .await
        .assert_not_found();
}

#[tokio::test]
async fn missing_post_renders_jsonapi_404() {
    let client = setup().await;
    let response = client
        .request("GET", "/posts/999")
        .header("accept", lumos_testing::VND_API_JSON)
        .send(&client)
        .await;
    response
        .assert_not_found()
        .assert_header("content-type", lumos_testing::VND_API_JSON);
    assert_eq!(response.json().unwrap()["errors"][0]["status"], "404");
}
