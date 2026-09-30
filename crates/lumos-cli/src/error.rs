//! CLI errors: a message for stderr plus a process exit code.
//!
//! Every failure mode maps here: unknown commands and flags, invalid
//! values, refused overwrites, missing `cargo`, failed migrations. An
//! empty message prints nothing (the child process already explained
//! itself); the code still propagates.

use std::fmt;

/// CLI failure: stderr message plus exit code.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::CliError;
///
/// let error = CliError::new("unknown command `frobnicate`");
/// assert_eq!(error.code(), 1);
/// assert!(error.to_string().contains("frobnicate"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    message: String,
    code: i32,
}

impl CliError {
    /// Builds an error exiting with code 1.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::CliError;
    ///
    /// let error = CliError::new("bad flag");
    /// assert_eq!(error.code(), 1);
    /// ```
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }

    /// Builds an error with an explicit exit code (child failures propagate
    /// theirs; use an empty message when the child already printed).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::CliError;
    ///
    /// let error = CliError::with_code("", 3);
    /// assert_eq!(error.code(), 3);
    /// assert!(error.to_string().is_empty());
    /// ```
    pub fn with_code(message: impl Into<String>, code: i32) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }

    /// The process exit code.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::CliError;
    ///
    /// assert_eq!(CliError::new("x").code(), 1);
    /// ```
    pub fn code(&self) -> i32 {
        self.code
    }

    /// The stderr message (possibly empty — see [`CliError::with_code`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::CliError;
    ///
    /// assert_eq!(CliError::new("boom").message(), "boom");
    /// ```
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}
