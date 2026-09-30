//! Logging facade over `tracing`.
//!
//! [`Log::init`] installs a default subscriber (human-readable `fmt` output
//! filtered by `RUST_LOG`, defaulting to `info`). The `info`/`warn`/`error`/
//! `debug` helpers cover plain-string logging; for formatted messages and
//! spans, use `tracing`'s macros directly — that is the supported escape
//! hatch, and replacing the subscriber wholesale works the same way.

use crate::Result;

/// Logging facade.
///
/// # Examples
///
/// ```rust
/// use lumos_core::Log;
///
/// Log::init().unwrap();
/// Log::info("application booted");
/// ```
pub struct Log;

impl Log {
    /// Installs the default `tracing` subscriber (idempotent).
    ///
    /// Output is human-readable `fmt` filtered by the `RUST_LOG` environment
    /// variable, defaulting to `info`. Calling `init` when a subscriber is
    /// already installed is a documented no-op returning `Ok`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Log;
    ///
    /// Log::init().unwrap();
    /// Log::init().unwrap(); // idempotent
    /// ```
    pub fn init() -> Result<()> {
        use tracing_subscriber::EnvFilter;
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        // `try_init` only fails when a global subscriber is already set, in
        // which case keeping the existing one is the documented behavior.
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        Ok(())
    }

    /// Logs `message` at `INFO` level.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Log;
    ///
    /// Log::init().unwrap();
    /// Log::info("listening on 127.0.0.1:3000");
    /// ```
    pub fn info(message: &str) {
        tracing::info!("{message}");
    }

    /// Logs `message` at `WARN` level.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Log;
    ///
    /// Log::init().unwrap();
    /// Log::warn("deprecated endpoint hit");
    /// ```
    pub fn warn(message: &str) {
        tracing::warn!("{message}");
    }

    /// Logs `message` at `ERROR` level.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Log;
    ///
    /// Log::init().unwrap();
    /// Log::error("payment webhook failed");
    /// ```
    pub fn error(message: &str) {
        tracing::error!("{message}");
    }

    /// Logs `message` at `DEBUG` level.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Log;
    ///
    /// Log::init().unwrap();
    /// Log::debug("cache miss for user 7");
    /// ```
    pub fn debug(message: &str) {
        tracing::debug!("{message}");
    }
}
