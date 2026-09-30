//! In-process HTTP: build requests, run them against a router, capture.
//!
//! [`TestClient`] wraps a router with one-shot requests plus a cookie jar
//! (login once, then every call carries the session). [`TestRequest`]
//! builds a single call; every `send` returns a [`TestResponse`].

use std::cell::RefCell;
use std::collections::HashMap;

use lumos_core::axum::body::Body;
use lumos_core::axum::http::{Method, Request};
use lumos_core::Router;
use tower::ServiceExt;

use crate::response::TestResponse;

/// The JSON:API media type, for `Accept` / `Content-Type` headers.
pub const VND_API_JSON: &str = "application/vnd.api+json";

/// Plain JSON media type, for `Accept` / `Content-Type` headers.
pub const APPLICATION_JSON: &str = "application/json";

/// Runs requests against a router, remembering cookies between calls.
///
/// The jar is always on: `Set-Cookie` pairs persist and ride along as
/// `Cookie` until [`clear_cookies`](TestClient::clear_cookies). Methods
/// take `&self`, so one client serves a whole test.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_testing::TestClient;
/// use lumos_core::{get, Router};
///
/// # #[tokio::main]
/// # async fn main() {
/// let client = TestClient::new(Router::new().route("/", get(|| async { "hi" })));
/// client.get("/").await.assert_ok();
/// # }
/// ```
pub struct TestClient {
    router: Router,
    jar: RefCell<HashMap<String, String>>,
}

impl TestClient {
    /// Wraps `router` (cloned per request) with an empty cookie jar.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::Router;
    ///
    /// let client = TestClient::new(Router::new());
    /// ```
    pub fn new(router: Router) -> Self {
        Self {
            router,
            jar: RefCell::new(HashMap::new()),
        }
    }

    /// Starts a custom request (method + uri, then headers/body).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let client = TestClient::new(Router::new().route("/", get(|| async { "hi" })));
    /// client
    ///     .request("GET", "/")
    ///     .header("x-trace", "1")
    ///     .send(&client)
    ///     .await
    ///     .assert_ok();
    /// # }
    /// ```
    pub fn request(&self, method: &str, uri: &str) -> TestRequest {
        TestRequest {
            method: method.to_string(),
            uri: uri.to_string(),
            headers: Vec::new(),
            body: RequestBody::Empty,
        }
    }

    /// `GET uri` with no body.
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
    pub async fn get(&self, uri: &str) -> TestResponse {
        self.request("GET", uri).send(self).await
    }

    /// `DELETE uri` with no body.
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
    pub async fn delete(&self, uri: &str) -> TestResponse {
        self.request("DELETE", uri).send(self).await
    }

    /// `POST uri` with a plain-JSON body.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{post, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", post(|| async { lumos_core::no_content() })))
    ///     .post_json("/", &serde_json::json!({ "a": 1 }))
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub async fn post_json(
        &self,
        uri: &str,
        value: &impl lumos_core::serde::Serialize,
    ) -> TestResponse {
        self.request("POST", uri).json(value).send(self).await
    }

    /// `POST uri` with a JSON:API body (`Accept` + `Content-Type` set).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{post, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", post(|| async { lumos_core::no_content() })))
    ///     .post_vnd("/", &serde_json::json!({ "data": { "type": "w" } }))
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub async fn post_vnd(
        &self,
        uri: &str,
        value: &impl lumos_core::serde::Serialize,
    ) -> TestResponse {
        self.request("POST", uri).vnd(value).send(self).await
    }

    /// `PATCH uri` with a JSON:API body (`Accept` + `Content-Type` set).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{patch, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// TestClient::new(Router::new().route("/", patch(|| async { lumos_core::no_content() })))
    ///     .patch_vnd("/", &serde_json::json!({ "data": { "type": "w" } }))
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub async fn patch_vnd(
        &self,
        uri: &str,
        value: &impl lumos_core::serde::Serialize,
    ) -> TestResponse {
        self.request("PATCH", uri).vnd(value).send(self).await
    }

    /// Reads one jarred cookie (e.g. after login).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::Router;
    ///
    /// let client = TestClient::new(Router::new());
    /// assert!(client.cookie("session").is_none());
    /// ```
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.jar.borrow().get(name).cloned()
    }

    /// Presets one jar cookie (skips the login round-trip when a test
    /// mints sessions directly).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::Router;
    ///
    /// let client = TestClient::new(Router::new());
    /// client.set_cookie("session", "token-1");
    /// assert_eq!(client.cookie("session").as_deref(), Some("token-1"));
    /// ```
    pub fn set_cookie(&self, name: &str, value: &str) {
        self.jar
            .borrow_mut()
            .insert(name.to_string(), value.to_string());
    }

    /// Empties the jar (the next request goes anonymous).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::Router;
    ///
    /// let client = TestClient::new(Router::new());
    /// client.set_cookie("session", "token-1");
    /// client.clear_cookies();
    /// assert!(client.cookie("session").is_none());
    /// ```
    pub fn clear_cookies(&self) {
        self.jar.borrow_mut().clear();
    }
}

/// One request under construction; [`send`](TestRequest::send) runs it.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_testing::TestClient;
/// use lumos_core::{get, Router};
///
/// # #[tokio::main]
/// # async fn main() {
/// let client = TestClient::new(Router::new().route("/", get(|| async { "hi" })));
/// client.request("GET", "/").send(&client).await.assert_ok();
/// # }
/// ```
pub struct TestRequest {
    method: String,
    uri: String,
    headers: Vec<(String, String)>,
    body: RequestBody,
}

/// One mutually exclusive request-body representation.
enum RequestBody {
    Empty,
    Encoded {
        bytes: Vec<u8>,
        content_type: &'static str,
        accept: Option<&'static str>,
    },
    EncodeError(String),
}

impl TestRequest {
    /// Adds one header (repeat calls append; duplicates all send).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let client = TestClient::new(Router::new().route("/", get(|| async { "hi" })));
    /// client
    ///     .request("GET", "/")
    ///     .header("x-trace", "1")
    ///     .send(&client)
    ///     .await
    ///     .assert_ok();
    /// # }
    /// ```
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Sets a plain-JSON body (plus `Content-Type`).
    ///
    /// Serialization failure fails the send loudly, naming the cause.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{post, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let client = TestClient::new(Router::new().route("/", post(|| async { lumos_core::no_content() })));
    /// client
    ///     .request("POST", "/")
    ///     .json(&serde_json::json!({ "a": 1 }))
    ///     .send(&client)
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub fn json(mut self, value: &impl lumos_core::serde::Serialize) -> Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                self.body = RequestBody::Encoded {
                    bytes,
                    content_type: APPLICATION_JSON,
                    accept: None,
                };
            }
            Err(error) => {
                self.body =
                    RequestBody::EncodeError(format!("cannot serialize JSON body: {error}"));
            }
        }
        self
    }

    /// Sets a JSON:API body (`Accept` + `Content-Type` + document).
    ///
    /// Serialization failure fails the send loudly, naming the cause.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{post, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let client = TestClient::new(Router::new().route("/", post(|| async { lumos_core::no_content() })));
    /// client
    ///     .request("POST", "/")
    ///     .vnd(&serde_json::json!({ "data": { "type": "w" } }))
    ///     .send(&client)
    ///     .await
    ///     .assert_no_content();
    /// # }
    /// ```
    pub fn vnd(mut self, value: &impl lumos_core::serde::Serialize) -> Self {
        match serde_json::to_vec(value) {
            Ok(bytes) => {
                self.body = RequestBody::Encoded {
                    bytes,
                    content_type: VND_API_JSON,
                    accept: Some(VND_API_JSON),
                };
            }
            Err(error) => {
                self.body =
                    RequestBody::EncodeError(format!("cannot serialize JSON:API body: {error}"));
            }
        }
        self
    }

    /// Runs the request against `client`, jarring any `Set-Cookie`s.
    ///
    /// A bad method literal or an unserializable body fails loudly —
    /// both are test bugs, never server behavior.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_testing::TestClient;
    /// use lumos_core::{get, Router};
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let client = TestClient::new(Router::new().route("/", get(|| async { "hi" })));
    /// client.request("GET", "/").send(&client).await.assert_ok();
    /// # }
    /// ```
    pub async fn send(self, client: &TestClient) -> TestResponse {
        let (body, content_type, accept) = match self.body {
            RequestBody::Empty => (Vec::new(), None, None),
            RequestBody::Encoded {
                bytes,
                content_type,
                accept,
            } => (bytes, Some(content_type), accept),
            RequestBody::EncodeError(error) => panic!("{error}"),
        };
        let method = self.method.parse::<Method>();
        assert!(
            method.is_ok(),
            "invalid test request method: {:?}",
            self.method
        );
        let mut builder = Request::builder()
            .method(method.unwrap_or(Method::GET))
            .uri(self.uri.clone());
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(accept) = accept {
            builder = builder.header("accept", accept);
        }
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let jarred = render_cookies(&client.jar.borrow());
        if !jarred.is_empty() {
            builder = builder.header("cookie", jarred);
        }
        let built = builder.body(Body::from(body));
        assert!(built.is_ok(), "invalid test request URI: {:?}", self.uri);
        // The assert guarantees `Ok`; the fallback only satisfies the type.
        let request = built.unwrap_or_else(|_| Request::new(Body::empty()));
        let response = match client.router.clone().oneshot(request).await {
            Ok(response) => response,
            // `Router` never errors; the arm only satisfies the type.
            Err(error) => match error {},
        };
        jar_cookies(client, response.headers());
        TestResponse::capture(response).await
    }
}

/// Renders the jar as one `Cookie` header value.
fn render_cookies(jar: &HashMap<String, String>) -> String {
    let mut pairs: Vec<(&String, &String)> = jar.iter().collect();
    pairs.sort_by(|left, right| left.0.cmp(right.0));
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Stores every `Set-Cookie` pair (`name=value` before the first `;`).
fn jar_cookies(client: &TestClient, headers: &lumos_core::axum::http::HeaderMap) {
    for raw in headers.get_all("set-cookie") {
        let Ok(text) = raw.to_str() else {
            continue;
        };
        let Some(pair) = text.split(';').next() else {
            continue;
        };
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        client.set_cookie(name.trim(), value.trim());
    }
}
