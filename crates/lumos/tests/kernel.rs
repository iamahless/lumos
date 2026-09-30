//! Phase 1 kernel integration tests, through the public `lumos` facade.
//!
//! Covers MVC dispatch (`#[controller]` + `routes!`), error documents,
//! extractors, container, config, providers, middleware, and response
//! helpers — all via in-process requests (no sockets).

use std::sync::{Arc, Mutex};

use lumos::axum::body::Body;
use lumos::axum::http::{Request, StatusCode};
use lumos::axum::response::IntoResponse;
use lumos::{
    async_trait, controller, created, from_fn, group, no_content, ok, routes, AppError,
    Application, Config, Container, Json, Log, MiddlewareRegistry, Next, Path, Query, Response,
    Result, Router, ServiceProvider, ValidationError,
};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

// --- Helpers ---------------------------------------------------------------

async fn body_bytes(response: Response) -> Vec<u8> {
    lumos::axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

async fn body_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

fn request(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(uri);
    match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&value).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn raw_request(
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: &'static str,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    builder.body(Body::from(body)).unwrap()
}

// --- Fixture service + controller ------------------------------------------

#[derive(Debug, Clone)]
struct UserService {
    tag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct User {
    id: i64,
    name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fixture shape for the Query extractor; values intentionally unused.
struct Pagination {
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct NewUser {
    name: String,
}

#[controller]
#[derive(Debug)]
struct UserController {
    users: UserService,
}

#[controller]
impl UserController {
    pub async fn index(&self, Query(pagination): Query<Pagination>) -> Result<Response> {
        let _ = pagination;
        Ok(ok(&vec![User {
            id: 1,
            name: format!("{}-ada", self.users.tag),
        }]))
    }

    pub async fn store(&self, Json(input): Json<NewUser>) -> Result<Response> {
        Ok(created(
            "/users/2",
            &User {
                id: 2,
                name: input.name,
            },
        ))
    }

    pub async fn show(&self, Path(id): Path<i64>) -> Result<Response> {
        if id == 1 {
            Ok(ok(&User {
                id,
                name: "ada".to_string(),
            }))
        } else {
            Err(AppError::not_found(format!("user {id}")))
        }
    }

    pub async fn update(
        &self,
        Path(id): Path<i64>,
        Json(input): Json<NewUser>,
    ) -> Result<Response> {
        Ok(ok(&User {
            id,
            name: input.name,
        }))
    }

    /// Sync actions are supported too: the codegen adapts to the method.
    pub fn destroy(&self, Path(id): Path<i64>) -> Result<Response> {
        let _ = id;
        Ok(no_content())
    }

    /// Non-convention methods get no route (proven by the 404 test below)
    /// but remain ordinary callable methods.
    pub async fn custom(&self) -> usize {
        self.helper() + 1
    }

    fn helper(&self) -> usize {
        self.users.tag.len()
    }
}

fn users_router() -> Router {
    let mut container = Container::new();
    container.singleton_value(UserService {
        tag: "test".to_string(),
    });
    let controller = Arc::new(UserController::from_container(&container).unwrap());
    routes! {
        resource("/users", UserController, controller),
        route("/health", lumos::get(|| async { "ok" })),
    }
}

// --- MVC dispatch ----------------------------------------------------------

#[tokio::test]
async fn resource_index_lists_users() {
    let response = users_router()
        .oneshot(request("GET", "/users", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body[0]["name"], "test-ada");

    // Query extractors survive empty and populated query strings alike.
    let response = users_router()
        .oneshot(request("GET", "/users?page=2&per_page=5", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn resource_store_returns_201_with_location() {
    let response = users_router()
        .oneshot(request(
            "POST",
            "/users",
            Some(serde_json::json!({ "name": "grace" })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["location"], "/users/2");
    assert_eq!(body_json(response).await["name"], "grace");
}

#[tokio::test]
async fn resource_show_serves_and_404s_with_standard_shape() {
    let response = users_router()
        .oneshot(request("GET", "/users/1", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["id"], 1);

    let response = users_router()
        .oneshot(request("GET", "/users/999", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["errors"][0]["code"], "not_found");
    assert_eq!(body["errors"][0]["status"], "404");
    assert!(body.get("data").is_none());
}

#[tokio::test]
async fn resource_update_answers_put_and_patch() {
    for method in ["PUT", "PATCH"] {
        let response = users_router()
            .oneshot(request(
                method,
                "/users/1",
                Some(serde_json::json!({ "name": "ada" })),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "for {method}");
        assert_eq!(body_json(response).await["name"], "ada");
    }
}

#[tokio::test]
async fn resource_destroy_returns_204_without_body() {
    let response = users_router()
        .oneshot(request("DELETE", "/users/1", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(body_bytes(response).await.is_empty());
}

#[tokio::test]
async fn plain_routes_coexist_with_resources_and_helpers_stay_callable() {
    let response = users_router()
        .oneshot(request("GET", "/health", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // No route was generated for the non-convention method.
    let response = users_router()
        .oneshot(request("GET", "/users/1/custom", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // ...but it is still an ordinary method.
    let mut container = Container::new();
    container.singleton_value(UserService {
        tag: "test".to_string(),
    });
    let controller = UserController::from_container(&container).unwrap();
    assert_eq!(controller.custom().await, 5);
}

#[tokio::test]
async fn from_container_fails_loudly_on_missing_bindings() {
    let container = Container::new();
    let error = UserController::from_container(&container).unwrap_err();
    assert_eq!(error.code(), "internal_error");
    assert!(error.to_string().contains("UserService"));
}

// --- Error documents --------------------------------------------------------

#[tokio::test]
async fn every_status_renders_the_standard_shape() {
    let cases: Vec<(AppError, StatusCode, &str)> = vec![
        (
            AppError::not_found("user"),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            AppError::bad_request("bad"),
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            AppError::Unauthorized,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        ),
        (AppError::Forbidden, StatusCode::FORBIDDEN, "forbidden"),
        (
            AppError::conflict("taken"),
            StatusCode::CONFLICT,
            "conflict",
        ),
        (
            AppError::not_acceptable("nope"),
            StatusCode::NOT_ACCEPTABLE,
            "not_acceptable",
        ),
        (
            AppError::unsupported_media_type("nope"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
        (
            AppError::internal("secret"),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
    ];
    for (error, status, code) in cases {
        let response = error.into_response();
        assert_eq!(response.status(), status, "for {code}");
        let body = body_json(response).await;
        assert_eq!(body["errors"][0]["status"], status.as_u16().to_string());
        assert_eq!(body["errors"][0]["code"], code);
        assert!(body.get("data").is_none());
    }

    // 401 and 403 are never confused.
    assert_ne!(
        AppError::Unauthorized.status_code(),
        AppError::Forbidden.status_code()
    );
}

#[tokio::test]
async fn validation_errors_carry_one_pointer_per_field() {
    let error = AppError::validation(vec![
        ValidationError::new("email", "The email field is required."),
        ValidationError::new("name", "The name field is required."),
    ]);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(response).await;
    assert_eq!(body["errors"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["errors"][0]["source"]["pointer"],
        "/data/attributes/email"
    );
    assert_eq!(
        body["errors"][1]["source"]["pointer"],
        "/data/attributes/name"
    );
}

#[tokio::test]
async fn internal_errors_hide_details_from_clients() {
    let body = body_json(AppError::internal("pool exhausted").into_response()).await;
    assert_eq!(body["errors"][0]["detail"], "An unexpected error occurred.");
}

// --- JSON extractor ---------------------------------------------------------

async fn echo(Json(value): Json<serde_json::Value>) -> Response {
    ok(&value)
}

fn echo_router() -> Router {
    Router::new().route("/echo", lumos::post(echo))
}

#[tokio::test]
async fn json_extractor_round_trips_rejects_and_types() {
    let response = echo_router()
        .oneshot(request(
            "POST",
            "/echo",
            Some(serde_json::json!({ "a": 1 })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["a"], 1);

    let response = echo_router()
        .oneshot(raw_request(
            "POST",
            "/echo",
            Some("application/json"),
            "{oops",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(response).await["errors"][0]["code"],
        "bad_request"
    );

    let response = echo_router()
        .oneshot(raw_request("POST", "/echo", None, r#"{"a":1}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        body_json(response).await["errors"][0]["code"],
        "unsupported_media_type"
    );
}

// --- Container ---------------------------------------------------------------

#[tokio::test]
async fn container_distinguishes_singletons_transients_and_missing() {
    let mut container = Container::new();
    container.singleton_value("shared".to_string());
    let counter = Arc::new(Mutex::new(0u32));
    container.bind({
        let counter = Arc::clone(&counter);
        move |_| Arc::new(*counter.lock().unwrap())
    });

    let first: Arc<String> = container.resolve().unwrap();
    let second: Arc<String> = container.resolve().unwrap();
    assert!(Arc::ptr_eq(&first, &second));

    *counter.lock().unwrap() = 1;
    let one: Arc<u32> = container.resolve().unwrap();
    *counter.lock().unwrap() = 2;
    let two: Arc<u32> = container.resolve().unwrap();
    assert_ne!(one, two);

    let error = container.resolve::<Vec<u8>>().unwrap_err();
    assert!(error.to_string().contains("Vec<u8>"));
}

// --- Config ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ServerConfig {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
struct AppConfig {
    name: String,
    debug: bool,
    server: ServerConfig,
}

#[test]
fn config_loads_typed_files_and_dotted_keys() {
    let config = Config::load_from("tests/fixtures/config").unwrap();
    let app: AppConfig = config.file("app").unwrap();
    assert_eq!(app.name, "blog");
    assert!(app.debug);
    assert_eq!(app.server.host, "127.0.0.1");
    assert_eq!(app.server.port, 3000);

    assert_eq!(
        config.get::<String>("database.default.url").unwrap(),
        "sqlite::memory:"
    );
    assert_eq!(config.get::<u64>("database.default.pool").unwrap(), 5);
    assert!(config.has("app.name"));
    assert!(!config.has("app.nope"));
    assert!(config.file::<AppConfig>("missing").is_err());
    assert!(config.get::<String>("app.nope").is_err());
}

// --- Providers ---------------------------------------------------------------

#[derive(Clone, Default)]
struct OrderLog(Arc<Mutex<Vec<String>>>);

struct ProviderA;
struct ProviderB;

#[async_trait]
impl ServiceProvider for ProviderA {
    fn register(&self, app: &mut Application) -> Result<()> {
        log(app, "register_a");
        Ok(())
    }

    async fn boot(&self, app: &Application) -> Result<()> {
        log_shared(app, "boot_a");
        Ok(())
    }
}

#[async_trait]
impl ServiceProvider for ProviderB {
    fn register(&self, app: &mut Application) -> Result<()> {
        log(app, "register_b");
        Ok(())
    }

    async fn boot(&self, app: &Application) -> Result<()> {
        log_shared(app, "boot_b");
        Ok(())
    }
}

fn log(app: &Application, event: &str) {
    log_shared(app, event);
}

fn log_shared(app: &Application, event: &str) {
    app.container()
        .resolve::<OrderLog>()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .push(event.to_string());
}

#[tokio::test]
async fn providers_register_in_order_then_boot_once() {
    let mut app = Application::new(Config::default());
    app.container_mut().singleton_value(OrderLog::default());
    app.register(ProviderA).unwrap();
    app.register(ProviderB).unwrap();
    app.boot().await.unwrap();
    app.boot().await.unwrap(); // idempotent

    let log: Arc<OrderLog> = app.container().resolve().unwrap();
    assert_eq!(
        *log.0.lock().unwrap(),
        vec!["register_a", "register_b", "boot_a", "boot_b"]
    );
}

#[tokio::test]
async fn application_mounts_routers_and_releases_them() {
    let mut app = Application::new(Config::default());
    app.mount(users_router());
    let response = app
        .into_router()
        .oneshot(request("GET", "/users/1", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

// --- Middleware + groups -----------------------------------------------------

async fn tag(request: lumos::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-tag", lumos::axum::http::HeaderValue::from_static("yes"));
    response
}

#[tokio::test]
async fn group_prefixes_routes_and_applies_layers() {
    let inner = Router::new().route("/ping", lumos::get(|| async { "pong" }));
    let router = group!("/admin", inner, [from_fn(tag)]);

    let response = router
        .oneshot(request("GET", "/admin/ping", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-tag"], "yes");

    let inner = Router::new().route("/ping", lumos::get(|| async { "pong" }));
    let router = group!("/admin", inner, [from_fn(tag)]);
    let response = router.oneshot(request("GET", "/ping", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn registry_applies_by_name_and_rejects_unknown() {
    let mut registry = MiddlewareRegistry::new();
    registry.register("tag", |router| router.layer(from_fn(tag)));

    let router = Router::new().route("/ping", lumos::get(|| async { "pong" }));
    let router = registry.apply(router, &["tag"]).unwrap();
    let response = router.oneshot(request("GET", "/ping", None)).await.unwrap();
    assert_eq!(response.headers()["x-tag"], "yes");

    let error = registry.apply(Router::new(), &["nope"]).unwrap_err();
    assert!(error.to_string().contains("nope"));
}

// --- Response helpers + logging ----------------------------------------------

#[test]
fn response_helpers_enforce_their_contracts() {
    let response = created("/users/1", &serde_json::json!({ "id": 1 }));
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["location"], "/users/1");

    let response = no_content();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = lumos::redirect("/login");
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()["location"], "/login");

    let response = lumos::see_other("/users/1");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[test]
fn log_init_is_idempotent() {
    Log::init().unwrap();
    Log::init().unwrap();
    Log::info("kernel test");
}

#[tokio::test]
async fn hello_world_router_answers() {
    let router = Router::new().route("/", lumos::get(|| async { "Hello, Lumos!" }));
    let response = router.oneshot(request("GET", "/", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_bytes(response).await, b"Hello, Lumos!");
}
