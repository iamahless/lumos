//! Configuration: `.env` + `config/*.toml` + typed structs.
//!
//! [`Config::load`] reads `.env` into the environment (via `dotenvy`; a
//! missing file is fine) and parses every `config/*.toml` file. Values are
//! read either as whole typed structs ([`Config::file`]) or via dotted keys
//! whose first segment names the file ([`Config::get`], e.g.
//! `"app.server.port"`).
//!
//! All failures are [`AppError::Internal`] with operator context: config is
//! read at boot, where the full message reaches the operator.

use std::collections::HashMap;
use std::path::Path;

use serde::de::DeserializeOwned;

use crate::{AppError, Result};

/// Loaded configuration: one parsed TOML document per file.
#[derive(Debug, Clone, Default)]
pub struct Config {
    files: HashMap<String, toml::Value>,
}

impl Config {
    /// Loads `.env` (optional) plus every `*.toml` file in `./config`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_core::Config;
    ///
    /// // Reads `.env` (if present) and `config/*.toml`.
    /// let config = Config::load()?;
    /// # Ok::<(), lumos_core::AppError>(())
    /// ```
    pub fn load() -> Result<Self> {
        // `.env` is optional; the real environment always wins.
        let _ = dotenvy::dotenv();
        Self::load_from("config")
    }

    /// Loads every `*.toml` file from `directory` (no `.env` handling).
    ///
    /// Missing directories and invalid TOML are errors naming the path.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Config;
    ///
    /// let error = Config::load_from("/definitely/not/a/config/dir").unwrap_err();
    /// assert_eq!(error.code(), "internal_error");
    /// ```
    pub fn load_from(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref();
        let entries = std::fs::read_dir(directory).map_err(|error| {
            AppError::internal(format!(
                "cannot read config directory {}: {error}",
                directory.display()
            ))
        })?;

        let mut files = HashMap::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                AppError::internal(format!("cannot list config directory: {error}"))
            })?;
            let path = entry.path();
            let is_toml = path
                .extension()
                .is_some_and(|extension| extension == "toml");
            if !is_toml {
                continue;
            }
            let name = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = std::fs::read_to_string(&path).map_err(|error| {
                AppError::internal(format!("cannot read {}: {error}", path.display()))
            })?;
            let value: toml::Value = toml::from_str(&text).map_err(|error| {
                AppError::internal(format!("invalid TOML in {}: {error}", path.display()))
            })?;
            files.insert(name, value);
        }
        Ok(Self { files })
    }

    /// Deserializes a whole config file into a typed struct.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Config;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct AppConfig {
    ///     name: String,
    /// }
    ///
    /// let config = Config::load_from("tests/fixtures/config").unwrap();
    /// let app: AppConfig = config.file("app").unwrap();
    /// assert_eq!(app.name, "blog");
    /// ```
    pub fn file<T: DeserializeOwned>(&self, name: &str) -> Result<T> {
        let value = self
            .files
            .get(name)
            .ok_or_else(|| AppError::internal(format!("missing config file '{name}.toml'")))?;
        T::deserialize(value.clone()).map_err(|error| {
            AppError::internal(format!("invalid config file '{name}.toml': {error}"))
        })
    }

    /// Reads a dotted key whose first segment names the file.
    ///
    /// For example, `"app.server.port"` reads key `server.port` from
    /// `app.toml`. Unknown files, missing keys, and type mismatches are
    /// errors naming the full key.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Config;
    ///
    /// let config = Config::load_from("tests/fixtures/config").unwrap();
    /// let port: u16 = config.get("app.server.port").unwrap();
    /// assert_eq!(port, 3000);
    /// assert!(config.get::<u16>("app.server.missing").is_err());
    /// ```
    pub fn get<T: DeserializeOwned>(&self, dotted: &str) -> Result<T> {
        let (file, rest) = dotted.split_once('.').ok_or_else(|| {
            AppError::internal(format!(
                "invalid config key '{dotted}': expected '<file>.<key>'"
            ))
        })?;
        let mut value = self
            .files
            .get(file)
            .ok_or_else(|| AppError::internal(format!("missing config file for key '{dotted}'")))?
            .clone();
        for segment in rest.split('.') {
            value = value
                .get(segment)
                .cloned()
                .ok_or_else(|| AppError::internal(format!("missing config key '{dotted}'")))?;
        }
        T::deserialize(value).map_err(|error| {
            AppError::internal(format!("invalid value for config key '{dotted}': {error}"))
        })
    }

    /// Returns `true` when a dotted key resolves to any value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Config;
    ///
    /// let config = Config::load_from("tests/fixtures/config").unwrap();
    /// assert!(config.has("app.name"));
    /// assert!(!config.has("app.nope"));
    /// ```
    pub fn has(&self, dotted: &str) -> bool {
        // Every TOML value deserializes into `toml::Value`, so success here
        // means exactly "the key exists".
        self.get::<toml::Value>(dotted).is_ok()
    }

    /// Reads and parses an environment variable (`.env` is loaded by
    /// [`Config::load`], so `.env` entries are visible here too).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Config;
    ///
    /// std::env::set_var("LUMOS_DOCTEST_PORT", "3000");
    /// let port: u16 = Config::env("LUMOS_DOCTEST_PORT").unwrap();
    /// std::env::remove_var("LUMOS_DOCTEST_PORT");
    /// assert_eq!(port, 3000);
    /// ```
    pub fn env<T>(key: &str) -> Result<T>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        let raw = std::env::var(key)
            .map_err(|_| AppError::internal(format!("missing environment variable '{key}'")))?;
        raw.parse::<T>()
            .map_err(|error| AppError::internal(format!("invalid value for '{key}': {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Config {
        Config::load_from("tests/fixtures/config").unwrap()
    }

    #[test]
    fn skips_non_toml_files() {
        // The fixture dir contains a README to prove non-toml files are ignored.
        let config = fixture();
        assert!(config.has("app.name"));
    }

    #[test]
    fn malformed_keys_are_rejected() {
        let config = fixture();
        assert!(config.get::<String>("no-file-segment").is_err());
        assert!(config.get::<String>("missing-file.key").is_err());
        assert!(config.get::<u16>("app.name").is_err(), "type mismatch");
    }

    #[test]
    fn env_reports_missing_and_invalid() {
        assert!(Config::env::<String>("LUMOS_DEFINITELY_UNSET_VAR_XYZ").is_err());
        std::env::set_var("LUMOS_DOCTEST_BAD_INT", "not-a-number");
        assert!(Config::env::<u16>("LUMOS_DOCTEST_BAD_INT").is_err());
        std::env::remove_var("LUMOS_DOCTEST_BAD_INT");
    }
}
