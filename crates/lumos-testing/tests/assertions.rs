//! Negative controls: every assertion passes on matching input and
//! panics otherwise. A `should_panic` that stops panicking fails loudly,
//! so these prove the assertions actually bite.

use lumos_core::axum::response::IntoResponse;
use lumos_core::{get, Router, StatusCode};
use lumos_testing::{TestClient, TestResponse};

async fn ok_json() -> lumos_core::Response {
    lumos_core::ok(&serde_json::json!({
        "data": { "type": "widgets", "id": "7", "attributes": { "name": "w" } }
    }))
}

async fn invalid() -> Result<String, lumos_core::AppError> {
    Err(lumos_core::AppError::validation(vec![
        lumos_core::ValidationError::new("email", "is invalid"),
    ]))
}

async fn empty() -> lumos_core::Response {
    lumos_core::no_content()
}

async fn bare_created() -> lumos_core::Response {
    StatusCode::CREATED.into_response()
}

fn app() -> Router {
    Router::new()
        .route("/ok", get(ok_json))
        .route("/invalid", get(invalid))
        .route("/empty", get(empty))
        .route("/bare", get(bare_created))
}

async fn fetch(path: &str) -> TestResponse {
    TestClient::new(app()).get(path).await
}

#[tokio::test]
async fn status_assertions_match() {
    fetch("/ok").await.assert_ok().assert_status(StatusCode::OK);
    fetch("/empty").await.assert_no_content();
    fetch("/invalid").await.assert_unprocessable();
    fetch("/missing").await.assert_not_found();
}

#[tokio::test]
#[should_panic(expected = "expected status 404")]
async fn assert_status_fails_loudly() {
    fetch("/ok").await.assert_not_found();
}

#[tokio::test]
async fn data_assertion_matches() {
    fetch("/ok").await.assert_data("widgets", "7");
}

#[tokio::test]
#[should_panic(expected = "data.type")]
async fn data_assertion_rejects_wrong_type() {
    fetch("/ok").await.assert_data("gadgets", "7");
}

#[tokio::test]
#[should_panic(expected = "data.id")]
async fn data_assertion_rejects_wrong_id() {
    fetch("/ok").await.assert_data("widgets", "8");
}

#[tokio::test]
#[should_panic(expected = "expected a JSON body")]
async fn data_assertion_rejects_empty_body() {
    fetch("/empty").await.assert_data("widgets", "7");
}

#[tokio::test]
async fn error_pointer_assertion_matches() {
    fetch("/invalid")
        .await
        .assert_error_pointer("/data/attributes/email");
}

#[tokio::test]
#[should_panic(expected = "error pointer")]
async fn error_pointer_assertion_rejects_absent_pointer() {
    fetch("/invalid")
        .await
        .assert_error_pointer("/data/attributes/name");
}

#[tokio::test]
async fn header_assertion_matches() {
    fetch("/ok")
        .await
        .assert_header("content-type", "application/json");
}

#[tokio::test]
#[should_panic(expected = "content-type")]
async fn header_assertion_rejects_mismatch() {
    fetch("/ok")
        .await
        .assert_header("content-type", "text/html");
}

#[tokio::test]
#[should_panic(expected = "expected status 201")]
async fn created_assertion_rejects_wrong_status() {
    fetch("/ok").await.assert_created();
}

#[tokio::test]
#[should_panic(expected = "201 without Location")]
async fn created_assertion_requires_location() {
    fetch("/bare").await.assert_created();
}
