//! The Phase 4 path: derived resources over a real router, strict
//! negotiation, and both renderers. `negotiate_*` runs under either flag
//! (negotiation is shared); `full_*`/`derive_*` need `jsonapi`;
//! `lite_*` needs `jsonapi-lite`.

#![cfg(any(feature = "jsonapi", feature = "jsonapi-lite"))]

use lumos::{
    get, post, ApiQuery, JsonApiBody, JsonApiResource, Resource, Router, StatusCode, ToMany, ToOne,
};
use tower::ServiceExt;

const VND: &str = "application/vnd.api+json";

#[derive(JsonApiResource)]
#[resource(type = "users")]
struct ApiUser {
    #[resource(id)]
    id: i64,
    name: String,
    #[resource(rename = "emailAddress")]
    email: String,
    // Read only by the jsonapi-gated derive test; exists to prove `hidden`.
    #[allow(dead_code)]
    #[resource(hidden)]
    password_hash: String,
}

#[derive(JsonApiResource)]
#[resource(type = "articles")]
struct ApiArticle {
    #[resource(id)]
    id: i64,
    title: String,
    #[resource(relation)]
    author: ToOne<ApiUser>,
    #[resource(relation, rename = "editors")]
    reviewed_by: ToMany<ApiUser>,
    #[resource(relation)]
    co_author: Option<ToOne<ApiUser>>,
}

fn ada() -> ApiUser {
    ApiUser {
        id: 7,
        name: "Ada".to_string(),
        email: "ada@example.com".to_string(),
        password_hash: "secret".to_string(),
    }
}

fn article() -> ApiArticle {
    ApiArticle {
        id: 1,
        title: "Hello".to_string(),
        author: ToOne::loaded(ada()),
        reviewed_by: ToMany::new(vec![ToOne::loaded(ada())]),
        co_author: None,
    }
}

fn vnd(body: &str) -> lumos::axum::body::Body {
    lumos::axum::body::Body::from(body.to_string())
}

async fn show(query: ApiQuery) -> lumos::ApiResult<lumos::Response> {
    lumos::single(&article(), &query)
}

async fn index(query: ApiQuery) -> lumos::ApiResult<lumos::Response> {
    let items = vec![article()];
    lumos::collection_memory(&items, &query)
}

async fn missing() -> lumos::ApiResult<lumos::Response> {
    Err(lumos::AppError::not_found("article 99").into())
}

#[derive(serde::Deserialize)]
struct NewArticle {
    title: String,
}

impl Resource for NewArticle {
    const TYPE: &'static str = "articles";
    fn resource_id(&self) -> String {
        String::new()
    }
    fn attributes(&self) -> lumos::Result<lumos::AttributeMap> {
        Ok(lumos::AttributeMap::new())
    }
    fn relationships(&self) -> Vec<lumos::NamedRelationship<'_>> {
        Vec::new()
    }
}

async fn store(body: JsonApiBody<NewArticle>) -> lumos::ApiResult<lumos::Response> {
    let created = ApiArticle {
        id: 9,
        title: body.resource.title.clone(),
        author: ToOne::loaded(ada()),
        reviewed_by: ToMany::default(),
        co_author: None,
    };
    let query = ApiQuery::parse(&[], "/articles").unwrap();
    lumos::lumos_jsonapi::created(&created, "/articles/9", &query)
}

fn app() -> Router {
    Router::new()
        .route("/articles", post(store).get(index))
        .route("/articles/1", get(show))
        .route("/missing", get(missing))
}

async fn body_json(response: lumos::Response) -> serde_json::Value {
    let bytes = lumos::axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[cfg(feature = "jsonapi")]
#[tokio::test]
async fn derive_shapes_attributes_linkage_and_options() {
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/articles/1?include=author,editors")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], VND);
    assert_eq!(response.headers()["vary"], "Accept");
    let json = body_json(response).await;
    // Attributes: title present, id structural, hidden excluded, rename applied.
    assert_eq!(json["data"]["attributes"]["title"], "Hello");
    assert!(json["data"]["attributes"].get("id").is_none());
    // Linkage: renamed relation, null option.
    assert_eq!(json["data"]["relationships"]["author"]["data"]["id"], "7");
    assert_eq!(
        json["data"]["relationships"]["editors"]["data"][0]["id"],
        "7"
    );
    assert!(json["data"]["relationships"]["co_author"]["data"].is_null());
    // Included: author 7 once despite two paths.
    assert_eq!(json["included"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["included"][0]["attributes"]["emailAddress"],
        "ada@example.com"
    );
    assert!(json["included"][0]["attributes"]
        .get("password_hash")
        .is_none());
    // Hidden excludes from output; the field itself still exists.
    assert_eq!(ada().password_hash, "secret");
}

#[cfg(feature = "jsonapi")]
#[tokio::test]
async fn full_collection_paginates_links_and_fieldsets() {
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/articles?sort=-title&page[size]=1&fields[articles]=title")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert!(json["data"][0].get("relationships").is_none());
    assert!(json["links"]["last"]
        .as_str()
        .unwrap()
        .contains("page[number]=1"));
}

#[tokio::test]
async fn negotiate_accept_matrix() {
    // Absent Accept passes.
    let response = app()
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/articles/1")
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Wrong type and parameterized entries are 406s with vnd errors.
    for accept in ["text/html", "application/vnd.api+json; charset=utf-8"] {
        let response = app()
            .clone()
            .oneshot(
                lumos::axum::http::Request::builder()
                    .uri("/articles/1")
                    .header("accept", accept)
                    .body(vnd(""))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE, "{accept}");
        assert_eq!(response.headers()["content-type"], VND);
        let json = body_json(response).await;
        assert_eq!(json["errors"][0]["code"], "not_acceptable");
    }
}

#[tokio::test]
async fn negotiate_body_requires_vnd_and_matching_type() {
    // Missing Content-Type is a 415.
    let response = app()
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/articles")
                .header("accept", VND)
                .body(vnd(
                    r#"{"data":{"type":"articles","attributes":{"title":"x"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Wrong data.type is a 409.
    let response = app()
        .clone()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/articles")
                .header("accept", VND)
                .header("content-type", VND)
                .body(vnd(
                    r#"{"data":{"type":"users","attributes":{"title":"x"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Valid write is a 201 with Location and a vnd document.
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .method("POST")
                .uri("/articles")
                .header("accept", VND)
                .header("content-type", VND)
                .body(vnd(
                    r#"{"data":{"type":"articles","attributes":{"title":"New"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["location"], "/articles/9");
    assert_eq!(response.headers()["content-type"], VND);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], "9");
}

#[tokio::test]
async fn negotiate_errors_render_vnd_documents() {
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/missing")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["content-type"], VND);
    assert_eq!(response.headers()["vary"], "Accept");
    let json = body_json(response).await;
    assert_eq!(json["errors"][0]["code"], "not_found");
    assert!(json.get("data").is_none(), "errors and data never mix");
}

#[cfg(feature = "jsonapi-lite")]
#[tokio::test]
async fn lite_flat_render_without_included() {
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/articles/1")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["attributes"]["title"], "Hello");
    assert_eq!(json["data"]["relationships"]["author"]["data"]["id"], "7");
    assert!(json.get("included").is_none());
}

#[cfg(feature = "jsonapi-lite")]
#[tokio::test]
async fn lite_compound_parameters_are_400s() {
    for uri in [
        "/articles/1?include=author",
        "/articles/1?fields[articles]=title",
    ] {
        let response = app()
            .clone()
            .oneshot(
                lumos::axum::http::Request::builder()
                    .uri(uri)
                    .header("accept", VND)
                    .body(vnd(""))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        let json = body_json(response).await;
        assert!(
            json["errors"][0]["detail"]
                .as_str()
                .unwrap()
                .contains("jsonapi-lite"),
            "{json}"
        );
    }
}

#[cfg(feature = "jsonapi-lite")]
#[tokio::test]
async fn lite_sort_and_page_still_work() {
    let response = app()
        .oneshot(
            lumos::axum::http::Request::builder()
                .uri("/articles?sort=-title&page[size]=1")
                .header("accept", VND)
                .body(vnd(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert!(json["links"]["self"]
        .as_str()
        .unwrap()
        .contains("sort=-title"));
}
