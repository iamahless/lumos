//! Strict per-route content negotiation plus JSON:API request bodies.
//!
//! Routes opt in by using [`ApiQuery`](crate::ApiQuery),
//! [`JsonApiBody`], or the document renderers. Opted-in routes require
//! `Accept` to allow [`JSON_API_MIME`] (406 otherwise, media-type
//! parameters rejected), require write bodies to declare it as
//! `Content-Type` (415 otherwise), and always answer with the JSON:API
//! content type plus `Vary: Accept`, including errors via [`JsonApiError`].
//! Routes that never touch this module negotiate nothing.
//!
//! # Examples
//!
//! ```rust
//! use lumos_core::axum::http::{HeaderMap, HeaderValue};
//! use lumos_jsonapi::negotiate::check_accept;
//!
//! let mut headers = HeaderMap::new();
//! headers.insert("accept", HeaderValue::from_static("application/vnd.api+json"));
//! assert!(check_accept(&headers).is_ok());
//! ```

use lumos_core::axum::body::Body;
use lumos_core::axum::extract::{FromRequest, Request};
use lumos_core::axum::http::header::{ACCEPT, CONTENT_TYPE};
use lumos_core::axum::http::{HeaderMap, HeaderValue};
use lumos_core::axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use lumos_core::{AppError, Result};

use crate::resource::Resource;

/// JSON:API media type. Parameters are never allowed on it (a `Content-Type`
/// or `Accept` entry carrying any is rejected outright).
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::JSON_API_MIME;
///
/// assert_eq!(JSON_API_MIME, "application/vnd.api+json");
/// ```
pub const JSON_API_MIME: &str = "application/vnd.api+json";

/// Maximum JSON:API request body: 2 MiB, mirroring axum's JSON default.
/// Larger bodies are 400s (the error taxonomy has no 413).
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::MAX_BODY_BYTES;
///
/// assert_eq!(MAX_BODY_BYTES, 2 * 1024 * 1024);
/// ```
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Requires `Accept` to allow JSON:API.
///
/// Absent `Accept` passes (the client takes what it gets); otherwise at
/// least one entry must match `application/vnd.api+json`, `application/*`,
/// or `*/*`, with parameters limited to `q`. Anything else is a 406
/// naming the problem.
///
/// # Examples
///
/// ```rust
/// use lumos_core::axum::http::{HeaderMap, HeaderValue};
/// use lumos_jsonapi::negotiate::check_accept;
///
/// let mut headers = HeaderMap::new();
/// assert!(check_accept(&headers).is_ok()); // absent: fine
///
/// headers.insert("accept", HeaderValue::from_static("text/html"));
/// assert!(check_accept(&headers).is_err()); // 406
/// ```
pub fn check_accept(headers: &HeaderMap) -> Result<()> {
    let Some(raw) = headers.get(ACCEPT) else {
        return Ok(());
    };
    let raw = raw
        .to_str()
        .map_err(|_| AppError::not_acceptable("Accept header is not valid ASCII".to_string()))?;
    for entry in raw.split(',') {
        let Ok(mime) = entry.trim().parse::<mime::Mime>() else {
            continue;
        };
        let essence = mime.essence_str();
        let acceptable = matches!(
            essence,
            "application/vnd.api+json" | "application/*" | "*/*"
        );
        let params_clean = mime.params().all(|(name, _)| name.as_str() == "q");
        if acceptable && params_clean {
            return Ok(());
        }
        if essence == JSON_API_MIME && !params_clean {
            return Err(AppError::not_acceptable(format!(
                "Accept entry {entry:?} carries media-type parameters, which JSON:API forbids"
            )));
        }
    }
    Err(AppError::not_acceptable(format!(
        "Accept must allow {JSON_API_MIME}"
    )))
}

/// Requires `Content-Type` to be exactly [`JSON_API_MIME`] (no parameters).
///
/// Missing, unparseable, or parameterized values are 415s. Write routes
/// using [`JsonApiBody`] always require it; readers never call this.
///
/// # Examples
///
/// ```rust
/// use lumos_core::axum::http::{HeaderMap, HeaderValue};
/// use lumos_jsonapi::negotiate::check_content_type;
///
/// let mut headers = HeaderMap::new();
/// headers.insert("content-type", HeaderValue::from_static("application/vnd.api+json"));
/// assert!(check_content_type(&headers).is_ok());
///
/// headers.insert("content-type", HeaderValue::from_static("application/json"));
/// assert!(check_content_type(&headers).is_err()); // 415
/// ```
pub fn check_content_type(headers: &HeaderMap) -> Result<()> {
    let Some(raw) = headers.get(CONTENT_TYPE) else {
        return Err(AppError::unsupported_media_type(format!(
            "Content-Type must be {JSON_API_MIME}"
        )));
    };
    let content_type = raw
        .to_str()
        .ok()
        .and_then(|value| value.parse::<mime::Mime>().ok());
    match content_type {
        Some(mime) if mime.essence_str() == JSON_API_MIME && mime.params().count() == 0 => Ok(()),
        _ => Err(AppError::unsupported_media_type(format!(
            "Content-Type must be {JSON_API_MIME} without parameters"
        ))),
    }
}

/// API error: an [`AppError`] rendered as a JSON:API error document.
///
/// Same `{errors: [...]}` shape as core (status/code/title/detail/source
/// already match the spec), but served as `application/vnd.api+json` with
/// `Vary: Accept`. Handlers return [`ApiResult`] so `?` converts `AppError`
/// (and anything that converts into it, e.g. `rusticate::Error` under
/// `orm`) directly.
///
/// # Examples
///
/// ```rust
/// use lumos_core::axum::response::IntoResponse;
/// use lumos_jsonapi::{ApiResult, JsonApiError};
/// use lumos_core::AppError;
///
/// let response = JsonApiError(AppError::not_found("user 1")).into_response();
/// assert_eq!(response.status(), lumos_core::StatusCode::NOT_FOUND);
/// assert_eq!(response.headers()["content-type"], "application/vnd.api+json");
/// assert_eq!(response.headers()["vary"], "Accept");
/// ```
#[derive(Debug)]
pub struct JsonApiError(pub AppError);

impl JsonApiError {
    /// Unwraps to the inner [`AppError`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::JsonApiError;
    /// use lumos_core::AppError;
    ///
    /// let error = JsonApiError(AppError::Forbidden);
    /// assert!(matches!(error.into_inner(), AppError::Forbidden));
    /// ```
    pub fn into_inner(self) -> AppError {
        self.0
    }
}

impl From<AppError> for JsonApiError {
    fn from(error: AppError) -> Self {
        Self(error)
    }
}

/// Handler result for API routes: `T` on success, a JSON:API error document
/// on failure. [`AppError`] converts via `?`, so existing fallible code
/// needs no mapping.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::ApiResult;
///
/// let ok: ApiResult<u32> = Ok(1);
/// assert_eq!(ok.unwrap(), 1);
/// ```
pub type ApiResult<T> = std::result::Result<T, JsonApiError>;

/// Renders any response value with the JSON:API content type and `Vary`.
pub(crate) fn api_response(body: serde_json::Value, status: lumos_core::StatusCode) -> Response {
    let mut response = lumos_core::json(status, &body);
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_API_MIME));
    headers.insert("vary", HeaderValue::from_static("Accept"));
    response
}

impl IntoResponse for JsonApiError {
    fn into_response(self) -> Response {
        let document = self.0.to_document();
        let status = self.0.status_code();
        let body =
            serde_json::to_value(&document).unwrap_or_else(|_| serde_json::json!({"errors": []}));
        api_response(body, status)
    }
}

/// JSON:API request body: `{data: {type, attributes}}` for writes.
///
/// Extraction requires `Accept` ([`check_accept`]) and an exact JSON:API
/// `Content-Type` ([`check_content_type`]), parses the envelope, checks
/// `data.type` against `T::TYPE` (409 on mismatch, the spec's conflict),
/// and deserializes `data.attributes` into `T`. Missing `data`/`type` /
/// `attributes`, non-object shapes, bulk arrays, invalid JSON, and
/// over-limit bodies are 400s.
///
/// `T` needs both [`Resource`] (the expected type) and `Deserialize`
/// (the attribute shape); DTOs derive both.
///
/// # Examples
///
/// ```rust
/// use lumos_core::axum::extract::FromRequest;
/// use lumos_jsonapi::{AttributeMap, JsonApiBody, NamedRelationship, Resource};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct NewUser {
///     name: String,
/// }
///
/// impl Resource for NewUser {
///     const TYPE: &'static str = "users";
///     fn resource_id(&self) -> String {
///         String::new()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), lumos_jsonapi::JsonApiError> {
/// let request = lumos_core::axum::http::Request::builder()
///     .method("POST")
///     .header("content-type", "application/vnd.api+json")
///     .header("accept", "application/vnd.api+json")
///     .body(lumos_core::axum::body::Body::from(
///         r#"{"data":{"type":"users","attributes":{"name":"Ada"}}}"#,
///     ))
///     .unwrap();
/// let body = JsonApiBody::<NewUser>::from_request(request, &()).await?;
/// assert_eq!(body.resource.name, "Ada");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct JsonApiBody<T> {
    /// Client-provided id (`data.id`), if any. Server-generated on POST;
    /// PATCH handlers compare it against the URL id (mismatch → 409).
    pub id: Option<String>,
    /// Deserialized `data.attributes`.
    pub resource: T,
}

impl<T, S> FromRequest<S> for JsonApiBody<T>
where
    T: Resource + DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = JsonApiError;

    async fn from_request(
        request: Request,
        _state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        check_accept(request.headers())?;
        check_content_type(request.headers())?;
        let (_parts, body) = request.into_parts();
        let bytes = read_body(body).await?;
        parse_body::<T>(&bytes).map_err(JsonApiError)
    }
}

/// Collects the body with the [`MAX_BODY_BYTES`] cap.
async fn read_body(body: Body) -> Result<Vec<u8>> {
    let bytes = lumos_core::axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|_| {
            AppError::bad_request(format!("request body exceeds {MAX_BODY_BYTES} bytes"))
        })?;
    Ok(bytes.to_vec())
}

/// Parses the `{data: {type, id?, attributes}}` envelope into `T`.
fn parse_body<T: Resource + DeserializeOwned>(bytes: &[u8]) -> Result<JsonApiBody<T>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| AppError::bad_request(format!("malformed JSON body: {error}")))?;
    let data = value
        .get("data")
        .ok_or_else(|| AppError::bad_request("missing data member".to_string()))?;
    if data.is_array() {
        return Err(AppError::bad_request(
            "bulk writes are not supported".to_string(),
        ));
    }
    let object = data
        .as_object()
        .ok_or_else(|| AppError::bad_request("data must be an object".to_string()))?;
    let resource_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::bad_request("data.type is required".to_string()))?;
    if resource_type != T::TYPE {
        return Err(AppError::conflict(format!(
            "type mismatch: expected {:?}, got {resource_type:?}",
            T::TYPE
        )));
    }
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let attributes = object
        .get("attributes")
        .ok_or_else(|| AppError::bad_request("data.attributes is required".to_string()))?;
    let resource: T = serde_json::from_value(attributes.clone())
        .map_err(|error| AppError::bad_request(format!("invalid attributes: {error}")))?;
    Ok(JsonApiBody { id, resource })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a header map from pairs.
    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                lumos_core::axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn accept_matrix() {
        // Absent passes.
        assert!(check_accept(&headers(&[])).is_ok());
        // Exact, multi-value, wildcards, and q-params pass.
        for accept in [
            "application/vnd.api+json",
            "text/html, application/vnd.api+json",
            "application/vnd.api+json;q=0.9",
            "application/*",
            "*/*",
        ] {
            assert!(
                check_accept(&headers(&[("accept", accept)])).is_ok(),
                "{accept}"
            );
        }
        // Missing the type, and media-type params, are 406s.
        for accept in [
            "text/html",
            "application/json",
            "application/vnd.api+json; charset=utf-8",
            "application/vnd.api+json;profile=x;q=0.5",
        ] {
            let error = check_accept(&headers(&[("accept", accept)])).unwrap_err();
            assert_eq!(
                error.status_code(),
                lumos_core::StatusCode::NOT_ACCEPTABLE,
                "{accept}"
            );
        }
    }

    #[test]
    fn content_type_matrix() {
        assert!(
            check_content_type(&headers(&[("content-type", "application/vnd.api+json")])).is_ok()
        );
        for content_type in [
            "application/json",
            "application/vnd.api+json; charset=utf-8",
            "text/html",
        ] {
            let error =
                check_content_type(&headers(&[("content-type", content_type)])).unwrap_err();
            assert_eq!(
                error.status_code(),
                lumos_core::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "{content_type}"
            );
        }
        let missing = check_content_type(&headers(&[])).unwrap_err();
        assert_eq!(
            missing.status_code(),
            lumos_core::StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[test]
    fn error_renders_vnd_document() {
        let response = JsonApiError(AppError::not_found("user 1")).into_response();
        assert_eq!(response.status(), lumos_core::StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["content-type"], JSON_API_MIME);
        assert_eq!(response.headers()["vary"], "Accept");
    }

    #[test]
    fn body_parses_envelope_and_checks_type() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct NewUser {
            name: String,
        }
        impl Resource for NewUser {
            const TYPE: &'static str = "users";
            fn resource_id(&self) -> String {
                String::new()
            }
            fn attributes(&self) -> Result<crate::AttributeMap> {
                Ok(crate::AttributeMap::new())
            }
            fn relationships(&self) -> Vec<crate::NamedRelationship<'_>> {
                Vec::new()
            }
        }

        let body = parse_body::<NewUser>(
            br#"{"data":{"type":"users","id":"9","attributes":{"name":"Ada"}}}"#,
        )
        .unwrap();
        assert_eq!(body.id.as_deref(), Some("9"));
        assert_eq!(body.resource.name, "Ada");

        let mismatch =
            parse_body::<NewUser>(br#"{"data":{"type":"groups","attributes":{"name":"Ada"}}}"#)
                .unwrap_err();
        assert_eq!(mismatch.status_code(), lumos_core::StatusCode::CONFLICT);

        for raw in [
            r#"{"data":[{"type":"users"}]}"#,
            r#"{"data":{"attributes":{}}}"#,
            r#"{"data":{"type":"users"}}"#,
            r#"{"nope":true}"#,
            r#"{"data":{"type":"users","attributes":{"name":1}}}"#,
            r#"not json"#,
        ] {
            assert!(parse_body::<NewUser>(raw.as_bytes()).is_err(), "{raw}");
        }
    }
}
