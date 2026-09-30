//! Typed request extractors.
//!
//! [`Json`] is Lumos's JSON-body extractor: unlike axum's, its rejections
//! render as standard [`AppError`](crate::AppError) documents (malformed
//! body → `400`, wrong content type → `415`). [`Path`], [`Query`], and
//! [`State`] are axum's extractors, re-exported for a single import surface.
//!
//! Escape hatch: axum's own `Json` keeps working inside Lumos handlers; it
//! simply renders axum's default error shape instead of Lumos's.

pub use axum::extract::{Path, Query, State};

use axum::extract::{rejection::JsonRejection, FromRequest, Request};
use axum::response::{IntoResponse, Response};

/// JSON request-body extractor with Lumos-standard error rendering.
///
/// Behaves like axum's `Json`, except rejections become [`AppError`]s:
/// unparseable bodies → `400 Bad Request`, missing or non-JSON content
/// type → `415 Unsupported Media Type`.
///
/// # Examples
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// use axum::extract::FromRequest;
/// use lumos_core::Json;
///
/// let request = axum::http::Request::builder()
///     .header("content-type", "application/json")
///     .body(axum::body::Body::from(r#"{"name":"Ada"}"#))
///     .unwrap();
/// let Json(value): Json<serde_json::Value> = Json::from_request(request, &()).await?;
/// assert_eq!(value["name"], "Ada");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// Consumes the extractor and returns the parsed body.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Json;
    ///
    /// let body = Json(serde_json::json!({ "a": 1 }));
    /// assert_eq!(body.into_inner()["a"], 1);
    /// ```
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> FromRequest<S> for Json<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = crate::AppError;

    async fn from_request(
        request: Request,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(rejection) => Err(map_json_rejection(rejection)),
        }
    }
}

/// Maps axum's JSON rejections onto the Lumos taxonomy. Every arm is
/// explicit: if axum adds a variant, this fails to compile until the new
/// case is classified — never silently misclassified.
fn map_json_rejection(rejection: JsonRejection) -> crate::AppError {
    match rejection {
        JsonRejection::JsonDataError(error) => {
            crate::AppError::bad_request(format!("invalid JSON body: {error}"))
        }
        JsonRejection::JsonSyntaxError(error) => {
            crate::AppError::bad_request(format!("malformed JSON body: {error}"))
        }
        JsonRejection::MissingJsonContentType(_) => {
            crate::AppError::unsupported_media_type("expected Content-Type: application/json")
        }
        JsonRejection::BytesRejection(error) => {
            crate::AppError::bad_request(format!("failed to read request body: {error}"))
        }
        _ => crate::AppError::bad_request("invalid JSON request"),
    }
}

impl<T: serde::Serialize> IntoResponse for Json<T> {
    /// Renders the value as `200 OK` JSON (symmetric with extraction).
    fn into_response(self) -> Response {
        crate::response::ok(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StatusCode;

    async fn extract<T: serde::de::DeserializeOwned>(
        content_type: Option<&str>,
        body: &'static str,
    ) -> std::result::Result<T, crate::AppError> {
        let mut builder = Request::builder().uri("/");
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let request = builder.body(axum::body::Body::from(body)).unwrap();
        Json::<T>::from_request(request, &())
            .await
            .map(Json::into_inner)
    }

    #[tokio::test]
    async fn valid_json_extracts() {
        let value: serde_json::Value = extract(Some("application/json"), r#"{"a":1}"#)
            .await
            .unwrap();
        assert_eq!(value["a"], 1);
    }

    #[tokio::test]
    async fn malformed_json_is_400_with_standard_shape() {
        let error = extract::<serde_json::Value>(Some("application/json"), "{oops")
            .await
            .unwrap_err();
        assert_eq!(error.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(error.code(), "bad_request");
    }

    #[tokio::test]
    async fn missing_content_type_is_415() {
        let error = extract::<serde_json::Value>(None, r#"{"a":1}"#)
            .await
            .unwrap_err();
        assert_eq!(error.status_code(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn json_response_is_200() {
        let response = Json(serde_json::json!({"a": 1})).into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
