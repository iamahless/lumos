//! Explicit route registry: the `route:list` source of truth.
//!
//! Axum routers cannot be introspected, so Lumos records routes
//! explicitly instead of pretending otherwise. Controllers expose
//! generated [`route_entries`](https://docs.rs/lumos/latest/lumos/)
//! (method + relative path + action per convention method that exists);
//! the app mounts them into a [`RouteRegistry`] next to the real mounts,
//! plus manual [`RouteRegistry::route`] calls for hand-written routes.
//! `route:list` renders whatever was registered. Raw axum routes do not
//! touch are invisible by design, never guessed.
//!
//! # Examples
//!
//! ```rust
//! use lumos_core::{RouteEntry, RouteRegistry};
//!
//! let mut registry = RouteRegistry::new();
//! registry.route("GET", "/health", "health");
//! registry.resource(
//!     "/users",
//!     vec![RouteEntry { method: "GET".to_string(), path: "/".to_string(), action: "index".to_string() }],
//! );
//! assert_eq!(registry.entries().len(), 2);
//! assert_eq!(registry.entries()[1].path, "/users");
//! ```

/// One registered route: method, absolute path, and handler name.
///
/// # Examples
///
/// ```rust
/// use lumos_core::RouteEntry;
///
/// let entry = RouteEntry {
///     method: "GET".to_string(),
///     path: "/users/{id}".to_string(),
///     action: "UserController::show".to_string(),
/// };
/// assert_eq!(entry.method, "GET");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// HTTP method in uppercase, e.g. `"GET"`.
    pub method: String,
    /// Absolute path, e.g. `"/users/{id}"`.
    pub path: String,
    /// Handler name, e.g. `"UserController::show"`.
    pub action: String,
}

/// Ordered route list, populated explicitly at app setup.
///
/// Entries render in registration order. The registry lives beside the
/// router (often in `Application` setup or a `routes()` helper) and is
/// passed to `route:list` from the app's CLI binary.
///
/// # Examples
///
/// ```rust
/// use lumos_core::RouteRegistry;
///
/// let mut registry = RouteRegistry::new();
/// assert!(registry.is_empty());
/// registry.route("GET", "/health", "health");
/// assert_eq!(registry.len(), 1);
/// ```
#[derive(Debug, Clone, Default)]
pub struct RouteRegistry {
    entries: Vec<RouteEntry>,
}

impl RouteRegistry {
    /// Builds an empty registry.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::RouteRegistry;
    ///
    /// let registry = RouteRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one hand-written route.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::RouteRegistry;
    ///
    /// let mut registry = RouteRegistry::new();
    /// registry.route("POST", "/login", "login");
    /// assert_eq!(registry.entries()[0].path, "/login");
    /// ```
    pub fn route(
        &mut self,
        method: impl Into<String>,
        path: impl Into<String>,
        action: impl Into<String>,
    ) {
        self.entries.push(RouteEntry {
            method: method.into(),
            path: path.into(),
            action: action.into(),
        });
    }

    /// Registers a controller's entries under `prefix`, joining paths.
    ///
    /// Join rules: `"/users"` + `"/"` → `"/users"`; `"/users"` + `"/{id}"`
    /// → `"/users/{id}"`; trailing slashes on the prefix are trimmed
    /// (`"/users/"` behaves as `"/users"`); an empty prefix keeps relative
    /// paths as-is. Mirrors `Router::nest` placement for well-formed
    /// prefixes, so the registry call sits next to its mount.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{RouteEntry, RouteRegistry};
    ///
    /// let mut registry = RouteRegistry::new();
    /// registry.resource(
    ///     "/users",
    ///     vec![
    ///         RouteEntry { method: "GET".to_string(), path: "/".to_string(), action: "index".to_string() },
    ///         RouteEntry { method: "GET".to_string(), path: "/{id}".to_string(), action: "show".to_string() },
    ///     ],
    /// );
    /// let paths: Vec<&str> =
    ///     registry.entries().iter().map(|entry| entry.path.as_str()).collect();
    /// assert_eq!(paths, vec!["/users", "/users/{id}"]);
    /// ```
    pub fn resource(&mut self, prefix: &str, entries: Vec<RouteEntry>) {
        let base = prefix.trim_end_matches('/');
        for mut entry in entries {
            entry.path = join_prefix(base, &entry.path);
            self.entries.push(entry);
        }
    }

    /// Borrows the entries in registration order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::RouteRegistry;
    ///
    /// let mut registry = RouteRegistry::new();
    /// registry.route("GET", "/a", "a");
    /// registry.route("GET", "/b", "b");
    /// assert_eq!(registry.entries().len(), 2);
    /// ```
    pub fn entries(&self) -> &[RouteEntry] {
        &self.entries
    }

    /// Counts registered routes.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::RouteRegistry;
    ///
    /// let registry = RouteRegistry::new();
    /// assert_eq!(registry.len(), 0);
    /// ```
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether no routes are registered.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::RouteRegistry;
    ///
    /// let registry = RouteRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Joins a trimmed prefix with a relative controller path.
fn join_prefix(base: &str, relative: &str) -> String {
    if relative == "/" {
        if base.is_empty() {
            return "/".to_string();
        }
        return base.to_string();
    }
    let joined = format!("{base}{relative}");
    if joined.is_empty() {
        return "/".to_string();
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> RouteEntry {
        RouteEntry {
            method: "GET".to_string(),
            path: path.to_string(),
            action: "a".to_string(),
        }
    }

    #[test]
    fn prefix_join_covers_edges() {
        let mut registry = RouteRegistry::new();
        registry.resource("/users", vec![entry("/"), entry("/{id}")]);
        registry.resource("/admin/", vec![entry("/")]);
        registry.resource("", vec![entry("/"), entry("/x")]);
        let paths: Vec<&str> = registry
            .entries()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, vec!["/users", "/users/{id}", "/admin", "/", "/x"]);
    }
}
