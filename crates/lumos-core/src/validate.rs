//! Request validation: declarative rules with `validator`, failures as 422.
//!
//! Derive [`Validate`](validator::Validate) on a request struct, extract it
//! with [`Validated`] (JSON bodies) or [`ValidatedQuery`] (query strings),
//! and invalid input becomes [`AppError::Validation`] — status 422 with one
//! `/data/attributes/{field}` entry per failure — while malformed input
//! still fails earlier as 400. The free [`validate`] function covers
//! non-extractor flows (services, tests, manual parsing).
//!
//! Rule failures sort by field name, so 422 output is deterministic across
//! runs (validator's error map itself is unordered). Nested structs flatten
//! to dotted paths (`address.street`); sequences index with brackets
//! (`items[0].name`).
//!
//! # Examples
//!
//! ```rust,no_run
//! use lumos_core::Validated;
//! use serde::Deserialize;
//! use validator::Validate;
//!
//! #[derive(Deserialize, Validate)]
//! struct NewUser {
//!     #[validate(length(min = 1, max = 80))]
//!     name: String,
//!     #[validate(email)]
//!     email: String,
//! }
//!
//! async fn store(Validated(input): Validated<NewUser>) -> String {
//!     format!("hello {}", input.name)
//! }
//! ```

use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;
use validator::{Validate, ValidationErrors, ValidationErrorsKind};

use crate::{AppError, Json, Result, ValidationError};

impl From<ValidationErrors> for AppError {
    fn from(errors: ValidationErrors) -> Self {
        Self::validation(field_errors(&errors))
    }
}

/// Validated JSON body: parses exactly like [`Json`], then runs
/// [`Validate`]. Malformed bodies fail as 400 (unchanged); rule violations
/// fail as 422 with per-field entries.
///
/// Destructures to the validated value: `Validated(input): Validated<T>`.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_core::Validated;
/// use serde::Deserialize;
/// use validator::Validate;
///
/// #[derive(Deserialize, Validate)]
/// struct NewUser {
///     #[validate(length(min = 1))]
///     name: String,
/// }
///
/// async fn store(Validated(input): Validated<NewUser>) -> String {
///     input.name
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Validated<T>(pub T);

impl<T, S> FromRequest<S> for Validated<T>
where
    T: Validate + DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        validate(&value)?;
        Ok(Self(value))
    }
}

/// Validated query string: parses exactly like axum's `Query`, then runs
/// [`Validate`]. Unparseable queries fail as 400; rule violations as 422.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_core::ValidatedQuery;
/// use serde::Deserialize;
/// use validator::Validate;
///
/// #[derive(Deserialize, Validate)]
/// struct Paging {
///     #[validate(range(min = 1, max = 100))]
///     per_page: u32,
/// }
///
/// async fn index(ValidatedQuery(paging): ValidatedQuery<Paging>) -> String {
///     format!("{} per page", paging.per_page)
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidatedQuery<T>
where
    T: Validate + DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self> {
        let Query(value) = Query::<T>::from_request_parts(parts, _state)
            .await
            .map_err(|rejection| AppError::bad_request(rejection.to_string()))?;
        validate(&value)?;
        Ok(Self(value))
    }
}

/// Validates `value`, mapping failures onto [`AppError::Validation`] (422).
///
/// Use it wherever validation happens outside an extractor — services,
/// manual parsing, tests.
///
/// # Examples
///
/// ```rust
/// use lumos_core::validate;
/// use serde::Deserialize;
/// use validator::Validate;
///
/// #[derive(Deserialize, Validate)]
/// struct NewUser {
///     #[validate(length(min = 1))]
///     name: String,
/// }
///
/// let input: NewUser = serde_json::from_value(serde_json::json!({ "name": "" })).unwrap();
/// let error = validate(&input).unwrap_err();
/// assert_eq!(error.status_code(), lumos_core::StatusCode::UNPROCESSABLE_ENTITY);
/// ```
pub fn validate<T: Validate>(value: &T) -> Result<()> {
    value.validate().map_err(AppError::from)
}

/// Flattens [`ValidationErrors`] into per-field [`ValidationError`]s, sorted
/// by field path. Nested structs join with dots, sequences with `[index]`.
///
/// Custom `message = "..."` rules surface verbatim; built-in codes render as
/// short sentences (`length` honors its `min`/`max` params, and so on);
/// unknown codes fall back to `is invalid`.
///
/// # Examples
///
/// ```rust
/// use lumos_core::validate::field_errors;
/// use serde::Deserialize;
/// use validator::Validate;
///
/// #[derive(Deserialize, Validate)]
/// struct NewUser {
///     #[validate(email)]
///     email: String,
/// }
///
/// let input: NewUser =
///     serde_json::from_value(serde_json::json!({ "email": "nope" })).unwrap();
/// let errors = input.validate().unwrap_err();
/// let flat = field_errors(&errors);
/// assert_eq!(flat.len(), 1);
/// assert_eq!(flat[0].field, "email");
/// ```
pub fn field_errors(errors: &ValidationErrors) -> Vec<ValidationError> {
    let mut flat = Vec::new();
    flatten_errors(errors, String::new(), &mut flat);
    flat.sort_by(|left, right| {
        left.field
            .cmp(&right.field)
            .then(left.message.cmp(&right.message))
    });
    flat
}

/// Recursively flattens one error map under `prefix`.
fn flatten_errors(errors: &ValidationErrors, prefix: String, flat: &mut Vec<ValidationError>) {
    for (field, kind) in errors.0.iter() {
        let path = if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        };
        match kind {
            ValidationErrorsKind::Field(failures) => {
                for failure in failures.iter() {
                    flat.push(ValidationError {
                        field: path.clone(),
                        message: failure_message(failure),
                    });
                }
            }
            ValidationErrorsKind::Struct(nested) => flatten_errors(nested, path, flat),
            ValidationErrorsKind::List(indexed) => {
                for (index, nested) in indexed.iter() {
                    flatten_errors(nested, format!("{path}[{index}]"), flat);
                }
            }
        }
    }
}

/// Renders one rule failure: custom message verbatim, known codes as
/// sentences, unknown codes as `is invalid`.
fn failure_message(failure: &validator::ValidationError) -> String {
    if let Some(message) = failure.message.as_ref() {
        return message.to_string();
    }
    let param = |name: &str| failure.params.get(name).and_then(|value| value.as_u64());
    match failure.code.as_ref() {
        "length" => match (param("min"), param("max")) {
            (Some(min), Some(max)) => format!("length must be between {min} and {max}"),
            (Some(min), None) => format!("length must be at least {min}"),
            (None, Some(max)) => format!("length must be at most {max}"),
            (None, None) => "has an invalid length".to_string(),
        },
        "range" => match (param("min"), param("max")) {
            (Some(min), Some(max)) => format!("must be between {min} and {max}"),
            (Some(min), None) => format!("must be at least {min}"),
            (None, Some(max)) => format!("must be at most {max}"),
            (None, None) => "is out of range".to_string(),
        },
        "email" => "must be a valid email address".to_string(),
        "url" => "must be a valid URL".to_string(),
        "contains" => "has an invalid format".to_string(),
        "regex" => "has an invalid format".to_string(),
        "required" => "is required".to_string(),
        _ => "is invalid".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use validator::Validate;

    use super::*;

    #[derive(Debug, Deserialize, Validate)]
    struct NewUser {
        #[validate(length(min = 1, max = 80))]
        name: String,
        #[validate(email)]
        email: String,
        #[validate(range(min = 13, max = 130))]
        age: u32,
    }

    #[derive(Debug, Deserialize, Validate)]
    struct Team {
        #[validate(length(min = 1))]
        name: String,
        #[validate(nested)]
        owner: NewUser,
        #[validate(nested)]
        members: Vec<NewUser>,
    }

    #[test]
    fn valid_input_passes() {
        let input: NewUser = serde_json::from_value(serde_json::json!({
            "name": "Ada",
            "email": "ada@example.com",
            "age": 36,
        }))
        .unwrap();
        assert!(validate(&input).is_ok());
    }

    #[test]
    fn failures_flatten_sorted_with_messages() {
        let input: NewUser = serde_json::from_value(serde_json::json!({
            "name": "",
            "email": "nope",
            "age": 200,
        }))
        .unwrap();
        let errors = input.validate().unwrap_err();
        let flat = field_errors(&errors);
        let fields: Vec<&str> = flat.iter().map(|error| error.field.as_str()).collect();
        assert_eq!(fields, vec!["age", "email", "name"]);
        assert!(flat[0].message.contains("130"), "{}", flat[0].message);
        assert!(flat[1].message.contains("email"), "{}", flat[1].message);
        assert!(
            flat[2].message.contains("between 1 and 80"),
            "{}",
            flat[2].message
        );
    }

    #[test]
    fn nested_paths_use_dots_and_brackets() {
        let bad: NewUser = serde_json::from_value(serde_json::json!({
            "name": "",
            "email": "nope",
            "age": 1,
        }))
        .unwrap();
        assert!(bad.validate().is_err());
        let team = Team {
            name: "t".to_string(),
            owner: serde_json::from_value(serde_json::json!({
                "name": "O",
                "email": "bad",
                "age": 30,
            }))
            .unwrap(),
            members: vec![bad],
        };
        let flat = field_errors(&team.validate().unwrap_err());
        let fields: Vec<&str> = flat.iter().map(|error| error.field.as_str()).collect();
        assert!(fields.contains(&"owner.email"), "{fields:?}");
        assert!(fields.contains(&"members[0].email"), "{fields:?}");
    }

    #[test]
    fn custom_messages_surface_verbatim_and_unknown_codes_fall_back() {
        #[derive(Debug, Validate)]
        struct Custom {
            #[validate(length(min = 2, message = "too short, friend"))]
            name: String,
            #[validate(custom(function = "forbid_bob"))]
            nickname: String,
        }

        fn forbid_bob(name: &str) -> std::result::Result<(), validator::ValidationError> {
            if name == "bob" {
                let mut error = validator::ValidationError::new("forbidden_name");
                error.message = None;
                return Err(error);
            }
            Ok(())
        }

        let input = Custom {
            name: "x".to_string(),
            nickname: "bob".to_string(),
        };
        let flat = field_errors(&input.validate().unwrap_err());
        assert_eq!(flat[0].field, "name");
        assert_eq!(flat[0].message, "too short, friend");
        assert_eq!(flat[1].field, "nickname");
        assert_eq!(flat[1].message, "is invalid");
    }

    #[test]
    fn validate_maps_to_422() {
        let input: NewUser = serde_json::from_value(serde_json::json!({
            "name": "Ada",
            "email": "nope",
            "age": 36,
        }))
        .unwrap();
        let error = validate(&input).unwrap_err();
        assert_eq!(
            error.status_code(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(error.code(), "validation_failed");
    }
}
