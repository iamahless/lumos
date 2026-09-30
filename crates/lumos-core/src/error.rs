//! Standard error taxonomy and machine-readable error documents.
//!
//! Every Lumos error renders as a JSON:API-shaped error document, whether or
//! not the `jsonapi` feature is enabled, so machine clients always see the
//! same shape:
//!
//! ```json
//! {
//!   "errors": [{
//!     "status": "422",
//!     "code": "validation_failed",
//!     "title": "Validation Failed",
//!     "detail": "The email field is required.",
//!     "source": { "pointer": "/data/attributes/email" }
//!   }]
//! }
//! ```
//!
//! Status-code contracts (enforced here, relied on everywhere):
//!
//! - Validation failures → `422`, never `400`.
//! - Malformed request bodies → `400`, never `500`.
//! - `401` means unauthenticated, `403` means forbidden — never confused.
//! - `500` responses never carry internal details; the context is logged via
//!   `tracing` and visible in `Display`/`Debug` for operators only.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Fallible result with [`AppError`] as the error type.
///
/// This is the return type of controller actions, provider hooks, and most
/// framework APIs.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{AppError, Result};
///
/// fn find_user(id: i64) -> Result<String> {
///     if id == 1 {
///         Ok("Ada".to_string())
///     } else {
///         Err(AppError::not_found("user"))
///     }
/// }
///
/// assert!(find_user(1).is_ok());
/// assert!(find_user(2).is_err());
/// ```
pub type Result<T> = std::result::Result<T, AppError>;

/// Machine-readable application error.
///
/// Each variant maps to exactly one HTTP status code (see
/// [`status_code`](AppError::status_code)). The enum is [`non_exhaustive`],
/// so matching downstream requires a wildcard arm — new variants may appear
/// in minor releases.
///
/// `AppError` converts into a response via [`IntoResponse`], which renders
/// the standard [`ErrorDocument`].
///
/// # Examples
///
/// ```rust
/// use lumos_core::AppError;
///
/// let error = AppError::not_found("user 42");
/// assert_eq!(error.status_code(), lumos_core::StatusCode::NOT_FOUND);
/// assert_eq!(error.code(), "not_found");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// `404 Not Found`. The named resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// `400 Bad Request`. The request itself is malformed (bad JSON body,
    /// invalid query parameter value, missing required parameter).
    #[error("bad request: {0}")]
    BadRequest(String),

    /// `422 Unprocessable Entity`. The request was well-formed but one or
    /// more fields failed validation. Renders one error object per field.
    #[error("validation failed")]
    Validation {
        /// Per-field failures.
        errors: Vec<ValidationError>,
    },

    /// `401 Unauthorized`. The request lacks valid authentication credentials.
    #[error("unauthenticated")]
    Unauthorized,

    /// `403 Forbidden`. The caller is authenticated but not allowed to
    /// perform this action.
    #[error("forbidden")]
    Forbidden,

    /// `409 Conflict`. The request conflicts with current server state
    /// (duplicate unique key, stale version).
    #[error("conflict: {0}")]
    Conflict(String),

    /// `406 Not Acceptable`. No response representation matches the
    /// request's `Accept` header.
    #[error("not acceptable: {0}")]
    NotAcceptable(String),

    /// `415 Unsupported Media Type`. The request body's `Content-Type` is
    /// not supported for this endpoint.
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),

    /// `500 Internal Server Error`.
    ///
    /// The inner message is operator context: it appears in
    /// `Display`/`Debug` and is logged when the error becomes a response,
    /// but clients always receive a generic detail string instead.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Builds a `404 Not Found` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::not_found("user 42");
    /// assert_eq!(error.detail(), "user 42");
    /// ```
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Builds a `400 Bad Request` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::bad_request("`page` must be positive");
    /// assert_eq!(error.status_code(), lumos_core::StatusCode::BAD_REQUEST);
    /// ```
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Builds a `422 Unprocessable Entity` error from per-field failures.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{AppError, ValidationError};
    ///
    /// let error = AppError::validation(vec![
    ///     ValidationError::new("email", "The email field is required."),
    /// ]);
    /// assert_eq!(error.error_objects().len(), 1);
    /// ```
    pub fn validation(errors: Vec<ValidationError>) -> Self {
        Self::Validation { errors }
    }

    /// Builds a `409 Conflict` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::conflict("email is already taken");
    /// assert_eq!(error.code(), "conflict");
    /// ```
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// Builds a `406 Not Acceptable` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::not_acceptable("expected Accept: application/json");
    /// assert_eq!(error.status_code(), lumos_core::StatusCode::NOT_ACCEPTABLE);
    /// ```
    pub fn not_acceptable(message: impl Into<String>) -> Self {
        Self::NotAcceptable(message.into())
    }

    /// Builds a `415 Unsupported Media Type` error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::unsupported_media_type("expected Content-Type: application/json");
    /// assert_eq!(error.status_code(), lumos_core::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    /// ```
    pub fn unsupported_media_type(message: impl Into<String>) -> Self {
        Self::UnsupportedMediaType(message.into())
    }

    /// Builds a `500 Internal Server Error` with operator-only context.
    ///
    /// The message is logged and visible via `Display`, but HTTP clients
    /// receive a generic detail string.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let error = AppError::internal("pool exhausted: 30s timeout");
    /// assert_eq!(error.detail(), "An unexpected error occurred.");
    /// ```
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Returns the HTTP status code for this error.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// assert_eq!(AppError::Unauthorized.status_code(), lumos_core::StatusCode::UNAUTHORIZED);
    /// assert_eq!(AppError::Forbidden.status_code(), lumos_core::StatusCode::FORBIDDEN);
    /// ```
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::NotAcceptable(_) => StatusCode::NOT_ACCEPTABLE,
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns the stable machine-readable code, e.g. `"validation_failed"`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// assert_eq!(AppError::Unauthorized.code(), "unauthorized");
    /// ```
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::BadRequest(_) => "bad_request",
            Self::Validation { .. } => "validation_failed",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Conflict(_) => "conflict",
            Self::NotAcceptable(_) => "not_acceptable",
            Self::UnsupportedMediaType(_) => "unsupported_media_type",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Returns the human-readable title, e.g. `"Validation Failed"`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// assert_eq!(AppError::Forbidden.title(), "Forbidden");
    /// ```
    pub fn title(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "Not Found",
            Self::BadRequest(_) => "Bad Request",
            Self::Validation { .. } => "Validation Failed",
            Self::Unauthorized => "Unauthorized",
            Self::Forbidden => "Forbidden",
            Self::Conflict(_) => "Conflict",
            Self::NotAcceptable(_) => "Not Acceptable",
            Self::UnsupportedMediaType(_) => "Unsupported Media Type",
            Self::Internal(_) => "Internal Server Error",
        }
    }

    /// Returns the client-safe detail string.
    ///
    /// For [`AppError::Internal`] this is always generic; the real context
    /// is logged instead of rendered.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// assert_eq!(AppError::Unauthorized.detail(), "Authentication is required.");
    /// ```
    pub fn detail(&self) -> String {
        match self {
            Self::NotFound(message)
            | Self::BadRequest(message)
            | Self::Conflict(message)
            | Self::NotAcceptable(message)
            | Self::UnsupportedMediaType(message) => message.clone(),
            Self::Validation { .. } => "One or more fields failed validation.".to_string(),
            Self::Unauthorized => "Authentication is required.".to_string(),
            Self::Forbidden => "You are not allowed to perform this action.".to_string(),
            Self::Internal(_) => "An unexpected error occurred.".to_string(),
        }
    }

    /// Returns one error object per failure: a single object for every
    /// variant except [`AppError::Validation`], which yields one object per
    /// field with a `source.pointer` of `/data/attributes/{field}`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{AppError, ValidationError};
    ///
    /// let error = AppError::validation(vec![
    ///     ValidationError::new("email", "The email field is required."),
    /// ]);
    /// let objects = error.error_objects();
    /// assert_eq!(objects[0].source.as_ref().unwrap().pointer.as_deref(), Some("/data/attributes/email"));
    /// ```
    pub fn error_objects(&self) -> Vec<ErrorObject> {
        match self {
            Self::Validation { errors } => errors
                .iter()
                .map(|failure| ErrorObject {
                    status: self.status_code().as_u16().to_string(),
                    code: self.code().to_string(),
                    title: self.title().to_string(),
                    detail: failure.message.clone(),
                    source: Some(ErrorSource {
                        pointer: Some(format!("/data/attributes/{}", failure.field)),
                        parameter: None,
                    }),
                })
                .collect(),
            _ => vec![ErrorObject {
                status: self.status_code().as_u16().to_string(),
                code: self.code().to_string(),
                title: self.title().to_string(),
                detail: self.detail(),
                source: None,
            }],
        }
    }

    /// Renders this error as a full [`ErrorDocument`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::AppError;
    ///
    /// let document = AppError::not_found("user").to_document();
    /// assert_eq!(document.errors.len(), 1);
    /// ```
    pub fn to_document(&self) -> ErrorDocument {
        ErrorDocument {
            errors: self.error_objects(),
        }
    }
}

impl IntoResponse for AppError {
    /// Renders the standard [`ErrorDocument`] with this error's status code.
    ///
    /// Internal errors are logged via `tracing` at conversion time; clients
    /// receive the generic detail string.
    fn into_response(self) -> Response {
        if matches!(self, Self::Internal(_)) {
            tracing::error!(error = %self, "internal error");
        }
        let status = self.status_code();
        let mut response = axum::Json(self.to_document()).into_response();
        *response.status_mut() = status;
        response
    }
}

/// A single field-level validation failure.
///
/// # Examples
///
/// ```rust
/// use lumos_core::ValidationError;
///
/// let failure = ValidationError::new("email", "The email field is required.");
/// assert_eq!(failure.field, "email");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Field name, e.g. `"email"`.
    pub field: String,
    /// Human-readable failure message.
    pub message: String,
}

impl ValidationError {
    /// Builds a field-level validation failure.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::ValidationError;
    ///
    /// let failure = ValidationError::new("name", "The name field is required.");
    /// assert_eq!(failure.message, "The name field is required.");
    /// ```
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

/// Top-level error document: `{ "errors": [...] }`.
///
/// A document carries `errors` and never `data`: success and failure
/// payloads are mutually exclusive by construction.
///
/// # Examples
///
/// ```rust
/// use lumos_core::AppError;
///
/// let document = AppError::Forbidden.to_document();
/// let json = serde_json::to_value(&document).unwrap();
/// assert_eq!(json["errors"][0]["code"], "forbidden");
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorDocument {
    /// One entry per failure.
    pub errors: Vec<ErrorObject>,
}

/// A single entry in an [`ErrorDocument`].
///
/// # Examples
///
/// ```rust
/// use lumos_core::AppError;
///
/// let object = &AppError::Conflict("taken".to_string()).error_objects()[0];
/// assert_eq!(object.status, "409");
/// assert_eq!(object.title, "Conflict");
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorObject {
    /// HTTP status code as a string, e.g. `"422"`.
    pub status: String,
    /// Stable machine-readable code, e.g. `"validation_failed"`.
    pub code: String,
    /// Human-readable title, e.g. `"Validation Failed"`.
    pub title: String,
    /// Client-safe detail message.
    pub detail: String,
    /// Where the error occurred, when attributable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ErrorSource>,
}

/// Points at the request location that caused an error.
///
/// # Examples
///
/// ```rust
/// use lumos_core::ErrorSource;
///
/// let source = ErrorSource { pointer: Some("/data/attributes/email".to_string()), parameter: None };
/// assert!(source.pointer.is_some());
/// ```
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorSource {
    /// JSON pointer into the request body, e.g. `"/data/attributes/email"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// Query parameter name, for errors in the query string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_a_distinct_status() {
        let errors = vec![
            AppError::not_found("x"),
            AppError::bad_request("x"),
            AppError::validation(vec![]),
            AppError::Unauthorized,
            AppError::Forbidden,
            AppError::conflict("x"),
            AppError::not_acceptable("x"),
            AppError::unsupported_media_type("x"),
            AppError::internal("x"),
        ];
        let statuses: Vec<StatusCode> = errors.iter().map(AppError::status_code).collect();
        let unique: std::collections::HashSet<StatusCode> = statuses.iter().copied().collect();
        assert_eq!(
            statuses.len(),
            unique.len(),
            "status codes must be distinct"
        );
    }

    #[test]
    fn document_serializes_to_the_standard_shape() {
        let document =
            AppError::validation(vec![ValidationError::new("email", "required")]).to_document();
        let json = serde_json::to_value(&document).unwrap();
        assert_eq!(json["errors"][0]["status"], "422");
        assert_eq!(json["errors"][0]["code"], "validation_failed");
        assert!(json.get("data").is_none(), "errors and data never mix");
    }

    #[test]
    fn internal_detail_is_generic_but_display_keeps_context() {
        let error = AppError::internal("secret context");
        assert_eq!(error.detail(), "An unexpected error occurred.");
        assert!(error.to_string().contains("secret context"));
    }
}
