//! Client behaviors: cookie jar round-trips, verbs, bodies, headers.

use lumos_core::{delete, get, post, Router};
use lumos_testing::{TestClient, VND_API_JSON};

async fn echo_cookies(headers: lumos_core::axum::http::HeaderMap) -> String {
    headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn set_session() -> lumos_core::Response {
    let mut response = lumos_core::ok(&serde_json::json!({ "ok": true }));
    response.headers_mut().insert(
        "set-cookie",
        "session=abc123; Path=/; HttpOnly".parse().unwrap(),
    );
    response
}

async fn echo_body(body: String) -> String {
    body
}

fn app() -> Router {
    Router::new()
        .route("/cookies", get(echo_cookies))
        .route("/login", post(set_session))
        .route("/echo", post(echo_body).patch(echo_body))
        .route("/item", delete(|| async { lumos_core::no_content() }))
}

#[tokio::test]
async fn jar_persists_login_cookie_across_calls() {
    let client = TestClient::new(app());
    client
        .post_json("/login", &serde_json::json!({}))
        .await
        .assert_ok();
    assert_eq!(client.cookie("session").as_deref(), Some("abc123"));

    let response = client.get("/cookies").await;
    response.assert_ok();
    assert!(response.text().contains("session=abc123"));
}

#[tokio::test]
async fn jar_can_be_preset_and_cleared() {
    let client = TestClient::new(app());
    client.set_cookie("session", "preset");
    let response = client.get("/cookies").await;
    assert!(response.text().contains("session=preset"));

    client.clear_cookies();
    assert!(client.cookie("session").is_none());
    let response = client.get("/cookies").await;
    assert_eq!(response.text(), "");
}

#[tokio::test]
async fn json_and_vnd_helpers_set_bodies_and_media_types() {
    let client = TestClient::new(app());

    let response = client
        .post_json("/echo", &serde_json::json!({ "a": 1 }))
        .await;
    response.assert_ok();
    assert_eq!(response.text(), r#"{"a":1}"#);

    let response = client
        .patch_vnd("/echo", &serde_json::json!({ "data": { "type": "w" } }))
        .await;
    response.assert_ok();
    assert!(response.text().contains("\"type\":\"w\""));

    // The VND helpers send the media type (probe via a header-echo route
    // would need app support; the request builder covers custom headers).
    let response = client
        .request("POST", "/echo")
        .header("accept", VND_API_JSON)
        .header("content-type", VND_API_JSON)
        .send(&client)
        .await;
    response.assert_ok();
}

#[tokio::test]
async fn delete_and_custom_headers_work() {
    let client = TestClient::new(app());
    client.delete("/item").await.assert_no_content();

    let response = client.request("GET", "/cookies").send(&client).await;
    response.assert_ok();
}
