//! Server-rendered views: Askama templates as responders.
//!
//! Derive [`Template`] on a struct (inline `source` or a file under
//! `templates/`), return [`View`] from a handler, and the response is `200`
//! with `Content-Type: text/html`. Rendering is compile-time codegen — a
//! template that fails to compile fails the build, never a request. Runtime
//! render failures (a fallible filter, a broken `Write`) become 500s with
//! the cause logged and a generic detail sent to clients.
//!
//! # Examples
//!
//! ```rust
//! use axum::response::IntoResponse;
//! use lumos_core::{Template, View};
//!
//! #[derive(Template)]
//! #[template(source = "<h1>Hello, {{ name }}!</h1>", ext = "html")]
//! struct Hello<'a> {
//!     name: &'a str,
//! }
//!
//! let response = View(Hello { name: "Ada" }).into_response();
//! assert_eq!(response.status(), lumos_core::StatusCode::OK);
//! ```

pub use askama::Template;
use axum::response::{Html, IntoResponse, Response};

use crate::AppError;

/// Responder that renders any Askama template as `text/html`.
///
/// Render failures map to [`AppError::Internal`] (500): the error is logged
/// for operators while clients get the generic detail string.
///
/// `View` takes templates by value or by reference (`&T` implements
/// [`Template`] whenever `T` does), so borrowed data needs no clone.
///
/// # Examples
///
/// ```rust
/// use axum::response::IntoResponse;
/// use lumos_core::{Template, View};
///
/// #[derive(Template)]
/// #[template(source = "<p>{{ text }}</p>", ext = "html")]
/// struct Page<'a> {
///     text: &'a str,
/// }
///
/// let response = View(&Page { text: "hi" }).into_response();
/// assert_eq!(response.status(), lumos_core::StatusCode::OK);
/// assert_eq!(
///     response.headers()["content-type"],
///     "text/html; charset=utf-8"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View<T>(pub T);

impl<T: Template> IntoResponse for View<T> {
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => Html(html).into_response(),
            Err(error) => {
                tracing::error!(error = %error, "template render failed");
                AppError::internal(format!("template render failed: {error}")).into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Template)]
    #[template(source = "<h1>Hello, {{ name }}!</h1>", ext = "html")]
    struct Hello<'a> {
        name: &'a str,
    }

    #[derive(Template)]
    #[template(path = "views_test_page.html")]
    struct Page<'a> {
        title: &'a str,
    }

    #[tokio::test]
    async fn renders_html_with_content_type() {
        let response = View(Hello { name: "Ada" }).into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, "<h1>Hello, Ada!</h1>");
    }

    #[tokio::test]
    async fn inheritance_extends_base_blocks() {
        let response = View(Page { title: "Child" }).into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("<title>Child</title>"), "{body}");
        assert!(body.contains("from base"), "{body}");
    }

    /// A template whose render always fails, without depending on any
    /// particular Askama failure mode: manual `Template` impl, `render`
    /// overridden to `Err`. This pins *our* 500 mapping, not askama's.
    struct Broken;

    impl std::fmt::Display for Broken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "broken")
        }
    }

    impl askama::FastWritable for Broken {
        fn write_into(
            &self,
            dest: &mut dyn std::fmt::Write,
            _values: &dyn askama::Values,
        ) -> askama::Result<()> {
            dest.write_fmt(format_args!("{self}"))
                .map_err(|_| askama::Error::Fmt)
        }
    }

    impl Template for Broken {
        const SIZE_HINT: usize = 0;

        fn render_into_with_values(
            &self,
            writer: &mut dyn std::fmt::Write,
            values: &dyn askama::Values,
        ) -> askama::Result<()> {
            <Self as askama::FastWritable>::write_into(self, writer, values)
        }

        fn render(&self) -> askama::Result<String> {
            Err(askama::Error::Fmt)
        }
    }

    #[tokio::test]
    async fn render_failure_maps_to_500() {
        let response = View(Broken).into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
