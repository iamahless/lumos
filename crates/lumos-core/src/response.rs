//! Response helpers with enforced HTTP semantics (RFC 9110).
//!
//! These constructors make the status-code contracts unrepresentable to
//! violate: [`created`] requires a `Location` header value, [`no_content`]
//! carries no body, and serialization failures degrade to a standard
//! [`AppError`](crate::AppError) document instead of panicking.

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response as AxumResponse};
use serde::Serialize;

/// HTTP response. An alias of axum's `Response`, re-exported so controller
/// actions name one type.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{ok, Response};
///
/// fn show() -> Response {
///     ok(&serde_json::json!({ "hello": "world" }))
/// }
///
/// assert_eq!(show().status(), lumos_core::StatusCode::OK);
/// ```
pub type Response = AxumResponse;

/// Renders `value` as JSON with an explicit status code.
///
/// If serialization fails, the response is a standard `500` error document
/// (and the failure is logged); this constructor never panics.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{json, StatusCode};
///
/// let response = json(StatusCode::ACCEPTED, &serde_json::json!({ "queued": true }));
/// assert_eq!(response.status(), StatusCode::ACCEPTED);
/// ```
pub fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => {
            let mut response = Response::new(Body::from(bytes));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
        Err(error) => {
            crate::AppError::internal(format!("failed to serialize response body: {error}"))
                .into_response()
        }
    }
}

/// Renders `value` as JSON with `200 OK`.
///
/// # Examples
///
/// ```rust
/// use lumos_core::ok;
///
/// let response = ok(&vec!["a", "b"]);
/// assert_eq!(response.status(), lumos_core::StatusCode::OK);
/// ```
pub fn ok<T: Serialize>(value: &T) -> Response {
    json(StatusCode::OK, value)
}

/// Renders `value` as JSON with `201 Created`.
///
/// The `Location` header is a required parameter: per RFC 9110 it MUST be
/// set on every `201`, and this signature makes omitting it a compile error.
///
/// # Examples
///
/// ```rust
/// use lumos_core::created;
///
/// let response = created("/users/1", &serde_json::json!({ "id": 1 }));
/// assert_eq!(response.status(), lumos_core::StatusCode::CREATED);
/// assert_eq!(response.headers()["location"], "/users/1");
/// ```
pub fn created<T: Serialize>(location: &str, value: &T) -> Response {
    let mut response = json(StatusCode::CREATED, value);
    if let Ok(header_value) = HeaderValue::from_str(location) {
        response
            .headers_mut()
            .insert(header::LOCATION, header_value);
    } else {
        // An invalid Location is a programmer error: degrade loudly to a
        // standard 500 document rather than emitting a 201 without one.
        return crate::AppError::internal(format!("invalid Location header value: {location:?}"))
            .into_response();
    }
    response
}

/// Returns `204 No Content` with an empty body, for successful deletes and
/// updates that return nothing.
///
/// # Examples
///
/// ```rust
/// use lumos_core::no_content;
///
/// let response = no_content();
/// assert_eq!(response.status(), lumos_core::StatusCode::NO_CONTENT);
/// ```
pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// Returns `302 Found` with a `Location` header (Laravel-compatible
/// default for form flows).
///
/// # Examples
///
/// ```rust
/// use lumos_core::redirect;
///
/// let response = redirect("/login");
/// assert_eq!(response.status(), lumos_core::StatusCode::FOUND);
/// assert_eq!(response.headers()["location"], "/login");
/// ```
pub fn redirect(url: &str) -> Response {
    redirect_with(StatusCode::FOUND, url)
}

/// Returns `303 See Other` with a `Location` header.
///
/// Prefer this over [`redirect`] after `POST`/`PUT`/`DELETE`: it tells the
/// client to follow the location with `GET`, per RFC 9110.
///
/// # Examples
///
/// ```rust
/// use lumos_core::see_other;
///
/// let response = see_other("/users/1");
/// assert_eq!(response.status(), lumos_core::StatusCode::SEE_OTHER);
/// ```
pub fn see_other(url: &str) -> Response {
    redirect_with(StatusCode::SEE_OTHER, url)
}

/// Shared redirect implementation: invalid URLs degrade to a standard 500
/// document instead of emitting a redirect without a `Location`.
fn redirect_with(status: StatusCode, url: &str) -> Response {
    match HeaderValue::from_str(url) {
        Ok(location) => {
            let mut response = StatusCode::OK.into_response();
            *response.status_mut() = status;
            response.headers_mut().insert(header::LOCATION, location);
            response
        }
        Err(_) => {
            crate::AppError::internal(format!("invalid redirect URL: {url:?}")).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unserializable_values_become_standard_500s() {
        struct NeverSerializable;
        impl Serialize for NeverSerializable {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("boom"))
            }
        }

        let response = ok(&NeverSerializable);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn invalid_locations_degrade_instead_of_emitting_bad_redirects() {
        let response = created("http://exa mple.com/\n", &1);
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let response = redirect("not a \n url");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn json_sets_content_type() {
        let response = ok(&1);
        assert_eq!(response.headers()["content-type"], "application/json");
    }
}
