//! Captured responses: status, headers, and JSON assertions.
//!
//! [`TestResponse`] is built by [`TestClient`](crate::TestClient), never by
//! hand. Every `assert_*` returns `&Self`, so expectations chain; every
//! failure message carries the status plus a body excerpt.

use lumos_core::{Response, StatusCode};

/// Maximum body bytes quoted in failure messages.
const EXCERPT_LEN: usize = 500;

/// Dummy for the unreachable arm of [`TestResponse::require_json`].
static NULL_BODY: serde_json::Value = serde_json::Value::Null;

/// A captured HTTP response: status, headers, raw body, parsed JSON.
///
/// `json()` is `None` when the body is empty or not JSON (a 204 has no
/// body to parse); every JSON assertion fails loudly in that case.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_testing::{TestClient, TestResponse};
/// use lumos_core::{get, Router, StatusCode};
///
/// async fn hello() -> &'static str {
///     "hi"
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// let response: TestResponse = TestClient::new(Router::new().route("/", get(hello)))
///     .get("/")
///     .await;
/// response.assert_status(StatusCode::OK);
/// assert_eq!(response.text(), "hi");
/// # }
/// ```
pub struct TestResponse {
    status: StatusCode,
    headers: lumos_core::axum::http::HeaderMap,
    body: Vec<u8>,
    json: Option<serde_json::Value>,
}

impl TestResponse {
    /// Captures a response: status, headers, full body, lenient JSON.
    ///
    /// Empty and non-JSON bodies capture as `json() == None` rather than
    /// erroring — asserting JSON-ness is the assertions' job.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_core::axum::body::Body;
    /// use lumos_core::axum::http::Request;
    /// use lumos_testing::TestResponse;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let router = Router::new().route("/", get(|| async { "hi" }));
    /// let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    /// let raw = tower::ServiceExt::oneshot(router, request).await.unwrap();
    /// let response = TestResponse::capture(raw).await;
    /// assert_eq!(response.text(), "hi");
    /// # }
    /// ```
    pub async fn capture(response: Response) -> Self {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = lumos_core::axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_default();
        let body = bytes.to_vec();
        let json = serde_json::from_slice(&body).ok();
        Self {
            status,
            headers,
            body,
            json,
        }
    }

    /// The status code.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router, StatusCode};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let response = TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await;
    /// assert_eq!(response.status(), StatusCode::OK);
    /// # }
    /// ```
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// One header value, or `None` when absent (or non-UTF-8).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let response = TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await;
    /// assert!(response.header("content-type").is_some());
    /// # }
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    /// The raw body bytes.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let response = TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await;
    /// assert_eq!(response.body(), b"hi");
    /// # }
    /// ```
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as lossy UTF-8.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let response = TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await;
    /// assert_eq!(response.text(), "hi");
    /// # }
    /// ```
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The parsed JSON body, or `None` when empty or invalid.
    ///
    /// Missing keys on the returned value render as `Null` (never panic),
    /// so `response.json()["data"]["id"]` is safe to probe.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let response = TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await;
    /// assert!(response.json().is_none());
    /// # }
    /// ```
    pub fn json(&self) -> Option<&serde_json::Value> {
        self.json.as_ref()
    }

    /// Asserts the exact status.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router, StatusCode};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await
    ///     .assert_status(StatusCode::OK);
    /// # }
    /// ```
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status,
            expected,
            "expected status {expected}, got {}{}",
            self.status,
            self.excerpt()
        );
        self
    }

    /// Asserts `200 OK`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await
    ///     .assert_ok();
    /// # }
    /// ```
    pub fn assert_ok(&self) -> &Self {
        self.assert_status(StatusCode::OK)
    }

    /// Asserts `201 Created` plus a `Location` header.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{post, Router};
    ///
    /// async fn store() -> lumos_core::Response {
    ///     lumos_core::created("/widgets/1", &serde_json::json!({ "ok": true }))
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", post(store)))
    ///     .post_json("/", &serde_json::json!({}))
    ///     .await
    ///     .assert_created();
    /// # }
    /// ```
    pub fn assert_created(&self) -> &Self {
        self.assert_status(StatusCode::CREATED);
        assert!(
            self.header("location").is_some(),
            "201 without Location{}",
            self.excerpt()
        );
        self
    }

    /// Asserts `204 No Content`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{delete, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", delete(|| async { lumos_core::no_content() })))
    ///     .delete("/")
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub fn assert_no_content(&self) -> &Self {
        self.assert_status(StatusCode::NO_CONTENT)
    }

    /// Asserts `404 Not Found`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::Router;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new()).get("/missing").await.assert_not_found();
    /// # }
    /// ```
    pub fn assert_not_found(&self) -> &Self {
        self.assert_status(StatusCode::NOT_FOUND)
    }

    /// Asserts `401 Unauthorized`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// async fn guarded() -> Result<String, lumos_core::AppError> {
    ///     Err(lumos_core::AppError::Unauthorized)
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(guarded)))
    ///     .get("/")
    ///     .await
    ///     .assert_unauthorized();
    /// # }
    /// ```
    pub fn assert_unauthorized(&self) -> &Self {
        self.assert_status(StatusCode::UNAUTHORIZED)
    }

    /// Asserts `403 Forbidden`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// async fn guarded() -> Result<String, lumos_core::AppError> {
    ///     Err(lumos_core::AppError::Forbidden)
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(guarded)))
    ///     .get("/")
    ///     .await
    ///     .assert_forbidden();
    /// # }
    /// ```
    pub fn assert_forbidden(&self) -> &Self {
        self.assert_status(StatusCode::FORBIDDEN)
    }

    /// Asserts `422 Unprocessable Entity`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// async fn guarded() -> Result<String, lumos_core::AppError> {
    ///     Err(lumos_core::AppError::validation(vec![]))
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(guarded)))
    ///     .get("/")
    ///     .await
    ///     .assert_unprocessable();
    /// # }
    /// ```
    pub fn assert_unprocessable(&self) -> &Self {
        self.assert_status(StatusCode::UNPROCESSABLE_ENTITY)
    }

    /// Asserts one header's exact value.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(|| async { "hi" })))
    ///     .get("/")
    ///     .await
    ///     .assert_header("content-type", "text/plain; charset=utf-8");
    /// # }
    /// ```
    pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
        assert_eq!(
            self.header(name),
            Some(expected),
            "expected header {name}: {expected:?}{}",
            self.excerpt()
        );
        self
    }

    /// Asserts a JSON:API resource body: `data.type` and `data.id`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// async fn show() -> lumos_core::Response {
    ///     lumos_core::ok(&serde_json::json!({
    ///         "data": { "type": "widgets", "id": "1" }
    ///     }))
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(show)))
    ///     .get("/")
    ///     .await
    ///     .assert_ok()
    ///     .assert_data("widgets", "1");
    /// # }
    /// ```
    pub fn assert_data(&self, resource_type: &str, id: &str) -> &Self {
        let json = self.require_json();
        assert_eq!(
            json["data"]["type"].as_str(),
            Some(resource_type),
            "expected data.type {resource_type:?}{}",
            self.excerpt()
        );
        assert_eq!(
            json["data"]["id"].as_str(),
            Some(id),
            "expected data.id {id:?}{}",
            self.excerpt()
        );
        self
    }

    /// Asserts one error object's JSON pointer appears in `errors[]`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router, ValidationError};
    ///
    /// async fn invalid() -> Result<String, lumos_core::AppError> {
    ///     Err(lumos_core::AppError::validation(vec![ValidationError::new(
    ///         "email",
    ///         "is invalid",
    ///     )]))
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", get(invalid)))
    ///     .get("/")
    ///     .await
    ///     .assert_unprocessable()
    ///     .assert_error_pointer("/data/attributes/email");
    /// # }
    /// ```
    pub fn assert_error_pointer(&self, pointer: &str) -> &Self {
        let json = self.require_json();
        let found = json["errors"].as_array().is_some_and(|errors| {
            errors.iter().any(|error| {
                error
                    .get("source")
                    .and_then(|source| source.get("pointer"))
                    .and_then(serde_json::Value::as_str)
                    == Some(pointer)
            })
        });
        assert!(
            found,
            "expected error pointer {pointer:?}{}",
            self.excerpt()
        );
        self
    }

    /// Returns the JSON body, failing loudly when the body is not JSON.
    fn require_json(&self) -> &serde_json::Value {
        assert!(
            self.json.is_some(),
            "expected a JSON body{}",
            self.excerpt()
        );
        // The assert guarantees `Some`; the static stands in for the
        // unreachable arm so no unwrap appears in shipped code.
        self.json.as_ref().unwrap_or(&NULL_BODY)
    }

    /// Failure suffix: status plus a body excerpt.
    fn excerpt(&self) -> String {
        let text = String::from_utf8_lossy(&self.body);
        let mut short: String = text.chars().take(EXCERPT_LEN).collect();
        if text.len() > EXCERPT_LEN {
            short.push('…');
        }
        format!("\n  status: {}\n  body: {short}", self.status)
    }
}
