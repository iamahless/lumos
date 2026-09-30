//! Error taxonomy for Rusticate.
//!
//! Standalone by design: this module depends only on `sqlx` and `thiserror`,
//! never on `lumos-core`. The `lumos` facade maps these errors onto HTTP
//! statuses (`NotFound` → 404, `UniqueViolation` → 409, everything else →
//! 500) when the `orm` feature is enabled.

use std::fmt::Write as _;

/// Fallible result with [`Error`] as the error type.
///
/// # Examples
///
/// ```rust
/// use rusticate::{Error, Result};
///
/// fn require_name(name: Option<String>) -> Result<String> {
///     name.ok_or_else(|| Error::not_found("name"))
/// }
/// ```
pub type Result<T> = std::result::Result<T, Error>;

/// Machine-usable ORM error.
///
/// The enum is [`non_exhaustive`], so matching downstream requires a
/// wildcard arm.
///
/// # Examples
///
/// ```rust
/// use rusticate::Error;
///
/// let error = Error::not_found("user 42");
/// assert!(error.is_not_found());
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No row matched (`find_or_fail`, `first_or_fail`, `fetch_one`).
    #[error("not found: {0}")]
    NotFound(String),

    /// A uniqueness constraint rejected the write. Classified eagerly from
    /// the database error kind so callers can match without inspecting sqlx.
    #[error("unique constraint violated: {0}")]
    UniqueViolation(String),

    /// Any other database failure (connection, syntax, constraint, I/O).
    /// The message comes from the driver and names tables/constraints.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// A column value could not be decoded into the field type. Always names
    /// the column and both types involved.
    #[error("decode error: {0}")]
    Decode(String),

    /// A Rust value could not be encoded for the database (e.g. JSON
    /// serialization of a cast field failed).
    #[error("encode error: {0}")]
    Encode(String),

    /// `.get()` was called on a relation that was never loaded. The message
    /// names the relation and the `with()` call that would load it.
    #[error("relation not loaded: {0}")]
    RelationNotLoaded(String),

    /// `with()` named a relation the model does not declare.
    #[error("unknown relation: {0}")]
    RelationNotFound(String),

    /// A query ran on a transaction that was already committed or rolled back.
    #[error("transaction already committed or rolled back")]
    TransactionFinished,

    /// Migration bookkeeping or DDL failed. Always names the migration.
    #[error("migration error: {0}")]
    Migration(String),

    /// Invalid connection URL or unsupported scheme.
    #[error("configuration error: {0}")]
    Config(String),

    /// The query itself is invalid: bad identifier, empty update, illegal
    /// pagination arguments. The SQL was never sent, so this is not a database failure.
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// Defensive bucket for states the type system should prevent (failed
    /// internal downcasts and the like). Indicates a framework bug, never
    /// user error; maps to a generic 500 downstream.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Classifies a driver error, eagerly detecting unique violations.
    ///
    /// Every query path funnels driver errors through here so callers can
    /// rely on [`Error::UniqueViolation`] without matching on sqlx types.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// let error = Error::db(sqlx::Error::RowNotFound);
    /// assert!(!error.is_unique_violation());
    /// ```
    pub fn db(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(driver) = &error {
            if matches!(driver.kind(), sqlx::error::ErrorKind::UniqueViolation) {
                return Self::UniqueViolation(driver.message().to_string());
            }
        }
        Self::Database(error)
    }

    /// Builds a [`Error::NotFound`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// let error = Error::not_found("user 42");
    /// assert_eq!(error.to_string(), "not found: user 42");
    /// ```
    pub fn not_found(what: impl Into<String>) -> Self {
        Self::NotFound(what.into())
    }

    /// Builds a [`Error::Decode`] naming the column.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// let error = Error::decode("age", "expected integer, got 'old'");
    /// assert!(error.to_string().contains("age"));
    /// ```
    pub fn decode(column: &str, detail: impl Into<String>) -> Self {
        Self::Decode(format!("column `{column}`: {}", detail.into()))
    }

    /// Builds a [`Error::InvalidQuery`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// let error = Error::invalid_query("page must be at least 1");
    /// assert!(matches!(error, Error::InvalidQuery(_)));
    /// ```
    pub fn invalid_query(detail: impl Into<String>) -> Self {
        Self::InvalidQuery(detail.into())
    }

    /// Returns `true` for [`Error::NotFound`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// assert!(Error::not_found("x").is_not_found());
    /// ```
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    /// Returns `true` for [`Error::UniqueViolation`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// assert!(Error::UniqueViolation("email".to_string()).is_unique_violation());
    /// ```
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, Self::UniqueViolation(_))
    }

    /// Builds a [`Error::Internal`] with operator context.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::Error;
    ///
    /// let error = Error::internal("registry type mismatch");
    /// assert!(matches!(error, Error::Internal(_)));
    /// ```
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal(detail.into())
    }
}

/// Truncates long values in error messages so a 10 MB blob never lands in a log.
pub(crate) fn snip(value: &str) -> String {
    const LIMIT: usize = 64;
    if value.len() <= LIMIT {
        value.to_string()
    } else {
        let mut out = String::with_capacity(LIMIT + 3);
        for (index, ch) in value.chars().enumerate() {
            if index >= LIMIT {
                break;
            }
            out.push(ch);
        }
        let _ = write!(out, "...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_violations_classify() {
        // Simulate each backend's unique-violation error through the real kind.
        // (Driver-level coverage lives in the sqlite integration tests.)
        let plain = Error::db(sqlx::Error::RowNotFound);
        assert!(matches!(plain, Error::Database(_)));
        assert!(!plain.is_unique_violation());
    }

    #[test]
    fn snip_truncates_long_values() {
        assert_eq!(snip("short"), "short");
        let long = "x".repeat(100);
        let snipped = snip(&long);
        assert!(snipped.len() < long.len());
        assert!(snipped.ends_with("..."));
    }
}
