//! Session authentication: argon2 passwords, token sessions, guards.
//!
//! Passwords hash with argon2id ([`hash_password`]) and check with
//! [`verify_password`]; both are synchronous and CPU-intensive, so handlers
//! should call them inside `tokio::task::spawn_blocking`. Sessions are opaque
//! 128-bit tokens kept in a [`SessionStore`] — [`MemorySessionStore`] ships
//! in-core, custom stores implement the three-method trait. [`login`] mints a
//! session, [`logout`] revokes it; the [`CurrentUser`] extractor guards
//! routes (`401` when the token is missing, unknown, or expired) and
//! [`CurrentUser::require`] adds scope checks (`403` for the wrong scope).
//!
//! Credentials arrive as `Authorization: Bearer <token>` first, then the
//! [`SESSION_COOKIE`] cookie. [`session_cookie`] / [`clear_session_cookie`]
//! build the `Set-Cookie` values; handlers attach them to responses.
//!
//! # Examples
//!
//! ```rust,no_run
//! use lumos_core::{
//!     login, logout, session_cookie, CurrentUser, MemorySessionStore,
//! };
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! async fn sign_in(store: Arc<MemorySessionStore>) -> String {
//!     let session = login(
//!         store.as_ref(),
//!         "user-1",
//!         vec!["read".to_string()],
//!         Duration::from_secs(3600),
//!     )
//!     .await
//!     .unwrap();
//!     session_cookie(&session.token, Duration::from_secs(3600))
//! }
//!
//! async fn sign_out(user: CurrentUser, store: Arc<MemorySessionStore>) {
//!     logout(store.as_ref(), &user.0.token).await.unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::{AppError, Result};

/// Cookie name for session tokens.
///
/// # Examples
///
/// ```rust
/// use lumos_core::SESSION_COOKIE;
///
/// assert_eq!(SESSION_COOKIE, "lumos_session");
/// ```
pub const SESSION_COOKIE: &str = "lumos_session";

/// Hashes a password with argon2id default parameters.
///
/// Returns the PHC-encoded hash (`$argon2id$...`), which embeds the
/// algorithm, parameters, and salt — store it verbatim.
///
/// This is CPU-intensive by design; call it from
/// `tokio::task::spawn_blocking` inside handlers. It fails only when the
/// OS RNG fails.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{hash_password, verify_password};
///
/// let hash = hash_password("correct horse").unwrap();
/// assert!(hash.starts_with("$argon2id$"));
/// assert!(verify_password("correct horse", &hash));
/// ```
pub fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::internal(format!("password hashing failed: {error}")))
}

/// Checks a password against a PHC hash from [`hash_password`].
///
/// Returns `false` for wrong passwords and malformed hashes alike, so
/// callers cannot distinguish the two. Like [`hash_password`], prefer
/// `tokio::task::spawn_blocking` inside handlers.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{hash_password, verify_password};
///
/// let hash = hash_password("correct horse").unwrap();
/// assert!(verify_password("correct horse", &hash));
/// assert!(!verify_password("wrong horse", &hash));
/// assert!(!verify_password("correct horse", "not-a-hash"));
/// ```
pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// An authenticated session: opaque token, owner, scopes, and expiry.
///
/// Sessions are bearer credentials — whoever holds [`Session::token`]
/// passes the [`CurrentUser`] guard until [`Session::expired`].
/// [`logout`] revokes them early.
///
/// # Examples
///
/// ```rust
/// use lumos_core::Session;
/// use std::time::{Duration, SystemTime};
///
/// let session = Session {
///     token: "abc123".to_string(),
///     user_id: "user-1".to_string(),
///     scopes: vec!["read".to_string()],
///     expires_at: SystemTime::now() + Duration::from_secs(60),
/// };
/// assert!(!session.expired());
/// assert!(session.allows("read"));
/// assert!(!session.allows("admin"));
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// Opaque bearer token (32 hex chars, 128-bit entropy).
    pub token: String,
    /// Owner identifier, e.g. a user id.
    pub user_id: String,
    /// Granted scopes, e.g. `["read", "admin"]`.
    pub scopes: Vec<String>,
    /// Instant the session stops authenticating.
    pub expires_at: SystemTime,
}

impl Session {
    /// Reports whether the session no longer authenticates.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Session;
    /// use std::time::{Duration, SystemTime};
    ///
    /// let fresh = Session {
    ///     token: String::new(),
    ///     user_id: String::new(),
    ///     scopes: Vec::new(),
    ///     expires_at: SystemTime::now() + Duration::from_secs(60),
    /// };
    /// assert!(!fresh.expired());
    /// ```
    pub fn expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }

    /// Reports whether the session grants `scope`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::Session;
    /// use std::time::{Duration, SystemTime};
    ///
    /// let session = Session {
    ///     token: String::new(),
    ///     user_id: String::new(),
    ///     scopes: vec!["admin".to_string()],
    ///     expires_at: SystemTime::now() + Duration::from_secs(60),
    /// };
    /// assert!(session.allows("admin"));
    /// assert!(!session.allows("read"));
    /// ```
    pub fn allows(&self, scope: &str) -> bool {
        self.scopes.iter().any(|granted| granted == scope)
    }
}

/// Persistent session storage: the seam custom drivers implement.
///
/// All methods are `&self` so stores share across handlers behind `Arc`.
/// Failures surface as [`AppError`]s (a downed Redis is a 500, not a 401).
///
/// # Examples
///
/// ```rust
/// use lumos_core::{MemorySessionStore, SessionStore};
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let store = MemorySessionStore::new();
/// assert!(store.load("missing").await?.is_none());
/// # Ok(())
/// # }
/// ```
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persists a session (insert or overwrite by token).
    async fn save(&self, session: Session) -> Result<()>;

    /// Loads the session for `token`, or `None` when unknown.
    async fn load(&self, token: &str) -> Result<Option<Session>>;

    /// Revokes the session for `token`; unknown tokens are a no-op.
    async fn delete(&self, token: &str) -> Result<()>;
}

/// In-memory [`SessionStore`], guarded by an `RwLock`.
///
/// Sessions vanish on restart and never replicate — fine for tests,
/// single-process apps, and as the reference driver implementation.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{login, MemorySessionStore, SessionStore};
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let store = MemorySessionStore::new();
/// let session = login(&store, "user-1", Vec::new(), Duration::from_secs(60)).await?;
/// assert!(store.load(&session.token).await?.is_some());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct MemorySessionStore {
    sessions: RwLock<HashMap<String, Session>>,
}

impl MemorySessionStore {
    /// Builds an empty store.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{MemorySessionStore, SessionStore};
    ///
    /// # #[tokio::main]
    /// # async fn main() -> lumos_core::Result<()> {
    /// let store = MemorySessionStore::new();
    /// assert!(store.load("anything").await?.is_none());
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Self {
        Self::default()
    }
}

/// Maps a poisoned lock onto a 500. Poisoning means another thread
/// panicked mid-access; the map itself may still be usable, but failing
/// closed is the honest response for an auth path.
fn poisoned<T>(_: T) -> AppError {
    AppError::internal("session store lock poisoned")
}

#[async_trait]
impl SessionStore for MemorySessionStore {
    async fn save(&self, session: Session) -> Result<()> {
        self.sessions
            .write()
            .map_err(poisoned)?
            .insert(session.token.clone(), session);
        Ok(())
    }

    async fn load(&self, token: &str) -> Result<Option<Session>> {
        Ok(self.sessions.read().map_err(poisoned)?.get(token).cloned())
    }

    async fn delete(&self, token: &str) -> Result<()> {
        self.sessions.write().map_err(poisoned)?.remove(token);
        Ok(())
    }
}

/// Mints a session for `user_id` with `scopes`, valid for `ttl`, and saves
/// it to `store`. Handlers return the token (JSON body) and/or set it via
/// [`session_cookie`].
///
/// # Examples
///
/// ```rust
/// use lumos_core::{login, MemorySessionStore};
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let store = MemorySessionStore::new();
/// let session = login(
///     &store,
///     "user-1",
///     vec!["read".to_string()],
///     Duration::from_secs(3600),
/// )
/// .await?;
/// assert_eq!(session.token.len(), 32);
/// # Ok(())
/// # }
/// ```
pub async fn login(
    store: &dyn SessionStore,
    user_id: impl Into<String>,
    scopes: Vec<String>,
    ttl: Duration,
) -> Result<Session> {
    let expires_at = SystemTime::now()
        .checked_add(ttl)
        .ok_or_else(|| AppError::internal("session ttl overflows system time"))?;
    let session = Session {
        token: new_token()?,
        user_id: user_id.into(),
        scopes,
        expires_at,
    };
    store.save(session.clone()).await?;
    Ok(session)
}

/// Revokes the session for `token`; unknown tokens are a no-op. Pair with
/// [`clear_session_cookie`] so browsers drop the cookie too.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{login, logout, MemorySessionStore, SessionStore};
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let store = MemorySessionStore::new();
/// let session = login(&store, "user-1", Vec::new(), Duration::from_secs(60)).await?;
/// logout(&store, &session.token).await?;
/// assert!(store.load(&session.token).await?.is_none());
/// # Ok(())
/// # }
/// ```
pub async fn logout(store: &dyn SessionStore, token: &str) -> Result<()> {
    store.delete(token).await
}

/// Mints a 128-bit session token as 32 hex chars. Draws from the OS CSPRNG
/// through the same helper argon2 uses for salts — random bytes are random
/// bytes; the `salt` name is password-hash's, not a constraint on use.
fn new_token() -> Result<String> {
    let bytes = argon2::password_hash::try_generate_salt()
        .map_err(|error| AppError::internal(format!("session entropy failed: {error}")))?;
    Ok(hex_encode(&bytes))
}

/// Lowercase hex without a dependency: 16 bytes in, 32 chars out.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Guard extractor: the request's authenticated session.
///
/// Reads `Authorization: Bearer <token>`, falling back to the
/// [`SESSION_COOKIE`] cookie. Rejects with `401` when credentials are
/// missing, unknown, or expired (expired sessions are deleted on sight).
/// Pair with [`CurrentUser::require`] for scope checks (`403`).
///
/// The store comes from an `Extension<Arc<dyn SessionStore>>` layer, so the
/// guard works with any router state. A missing layer is a 500: the app is
/// misconfigured, not the request.
///
/// # Examples
///
/// ```rust
/// use axum::extract::FromRequestParts;
/// use lumos_core::{login, CurrentUser, MemorySessionStore, SessionStore};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
/// let session = login(store.as_ref(), "user-1", Vec::new(), Duration::from_secs(60)).await?;
/// let request = axum::http::Request::builder()
///     .header("authorization", format!("Bearer {}", session.token))
///     .extension(store)
///     .body(())
///     .unwrap();
/// let (mut parts, _) = request.into_parts();
/// let user = CurrentUser::from_request_parts(&mut parts, &()).await?;
/// assert_eq!(user.0.user_id, "user-1");
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Session);

impl CurrentUser {
    /// Requires `scope`, rejecting with `403 Forbidden` when the session
    /// lacks it. The 401/403 split stays sharp: extraction proves *who*,
    /// `require` proves *allowed*.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{AppError, CurrentUser, Session};
    /// use std::time::{Duration, SystemTime};
    ///
    /// let user = CurrentUser(Session {
    ///     token: String::new(),
    ///     user_id: "user-1".to_string(),
    ///     scopes: vec!["read".to_string()],
    ///     expires_at: SystemTime::now() + Duration::from_secs(60),
    /// });
    /// assert!(user.require("read").is_ok());
    /// assert!(matches!(user.require("admin"), Err(AppError::Forbidden)));
    /// ```
    pub fn require(&self, scope: &str) -> Result<()> {
        if self.0.allows(scope) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self> {
        let token = bearer_token(&parts.headers)
            .or_else(|| cookie_token(&parts.headers))
            .ok_or(AppError::Unauthorized)?;
        let store = parts
            .extensions
            .get::<Arc<dyn SessionStore>>()
            .cloned()
            .ok_or_else(|| {
                AppError::internal(
                    "auth misconfigured: add an Extension<Arc<dyn SessionStore>> layer",
                )
            })?;
        match store.load(token).await? {
            Some(session) if !session.expired() => Ok(Self(session)),
            Some(session) => {
                store.delete(&session.token).await?;
                Err(AppError::Unauthorized)
            }
            None => Err(AppError::Unauthorized),
        }
    }
}

/// Extracts a Bearer token (scheme matched case-insensitively per RFC 6750).
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ").or_else(|| {
        if value.len() > 7 && value[..7].eq_ignore_ascii_case("bearer ") {
            Some(&value[7..])
        } else {
            None
        }
    })?;
    Some(token.trim())
}

/// Extracts the [`SESSION_COOKIE`] value from any `Cookie` header.
fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    for value in headers.get_all(COOKIE) {
        let cookies = value.to_str().ok()?;
        for pair in cookies.split(';') {
            let pair = pair.trim();
            if let Some(rest) = pair.strip_prefix(SESSION_COOKIE) {
                if let Some(token) = rest.strip_prefix('=') {
                    return Some(token.trim());
                }
            }
        }
    }
    None
}

/// Builds a `Set-Cookie` value persisting `token` for `max_age`.
///
/// `HttpOnly` + `SameSite=Lax` + `Path=/`; send over HTTPS in production
/// (add `Secure` at the reverse proxy or append it here).
///
/// # Examples
///
/// ```rust
/// use lumos_core::session_cookie;
/// use std::time::Duration;
///
/// let header = session_cookie("abc123", Duration::from_secs(3600));
/// assert!(header.starts_with("lumos_session=abc123"));
/// assert!(header.contains("HttpOnly"));
/// assert!(header.contains("Max-Age=3600"));
/// ```
pub fn session_cookie(token: &str, max_age: Duration) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        max_age.as_secs()
    )
}

/// Builds a `Set-Cookie` value clearing the session cookie.
///
/// # Examples
///
/// ```rust
/// use lumos_core::clear_session_cookie;
///
/// let header = clear_session_cookie();
/// assert!(header.contains("Max-Age=0"));
/// ```
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon2_round_trip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong horse", &hash));
        assert!(!verify_password("correct horse", "not-a-hash"));
    }

    #[test]
    fn tokens_are_32_hex_chars() {
        let first = new_token().unwrap();
        let second = new_token().unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()), "{first}");
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn login_stores_and_logout_revokes() {
        let store = MemorySessionStore::new();
        let session = login(
            &store,
            "user-1",
            vec!["read".to_string()],
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        let loaded = store.load(&session.token).await.unwrap().unwrap();
        assert_eq!(loaded.user_id, "user-1");
        assert!(loaded.allows("read"));

        logout(&store, &session.token).await.unwrap();
        assert!(store.load(&session.token).await.unwrap().is_none());

        // Unknown tokens are a no-op, never an error.
        logout(&store, "missing").await.unwrap();
    }

    /// Builds request parts carrying `store` plus the given headers.
    fn parts_with(store: Arc<dyn SessionStore>, headers: &[(&str, String)]) -> Parts {
        let mut builder = axum::http::Request::builder().extension(store);
        for (name, value) in headers {
            builder = builder.header(*name, value.as_str());
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[tokio::test]
    async fn guard_accepts_bearer_and_cookie() {
        let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
        let session = login(
            store.as_ref(),
            "user-1",
            vec!["read".to_string()],
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        let mut bearer = parts_with(
            Arc::clone(&store),
            &[("authorization", format!("Bearer {}", session.token))],
        );
        let user = CurrentUser::from_request_parts(&mut bearer, &())
            .await
            .unwrap();
        assert_eq!(user.0.user_id, "user-1");
        assert!(user.require("read").is_ok());

        let mut cookie = parts_with(
            Arc::clone(&store),
            &[(
                "cookie",
                format!("theme=dark; {SESSION_COOKIE}={}", session.token),
            )],
        );
        let user = CurrentUser::from_request_parts(&mut cookie, &())
            .await
            .unwrap();
        assert_eq!(user.0.user_id, "user-1");
    }

    #[tokio::test]
    async fn guard_rejects_with_401_and_require_with_403() {
        let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
        let session = login(
            store.as_ref(),
            "user-1",
            Vec::new(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        // Missing credentials → 401.
        let mut empty = parts_with(Arc::clone(&store), &[]);
        let error = CurrentUser::from_request_parts(&mut empty, &())
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Unauthorized));
        assert_eq!(error.code(), "unauthorized");

        // Unknown token → 401.
        let mut unknown = parts_with(
            Arc::clone(&store),
            &[("authorization", "Bearer nope".to_string())],
        );
        let error = CurrentUser::from_request_parts(&mut unknown, &())
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Unauthorized));

        // Authenticated but missing scope → 403, not 401.
        let mut bearer = parts_with(
            Arc::clone(&store),
            &[("authorization", format!("Bearer {}", session.token))],
        );
        let user = CurrentUser::from_request_parts(&mut bearer, &())
            .await
            .unwrap();
        let error = user.require("admin").unwrap_err();
        assert!(matches!(error, AppError::Forbidden));
        assert_eq!(error.status_code(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn expired_sessions_fail_closed_and_are_deleted() {
        let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
        // Zero TTL: already expired by the time the guard runs.
        let session = login(store.as_ref(), "user-1", Vec::new(), Duration::ZERO)
            .await
            .unwrap();
        assert!(session.expired());

        let mut bearer = parts_with(
            Arc::clone(&store),
            &[("authorization", format!("Bearer {}", session.token))],
        );
        let error = CurrentUser::from_request_parts(&mut bearer, &())
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Unauthorized));
        assert!(store.load(&session.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn missing_store_layer_is_a_500_not_a_401() {
        let request = axum::http::Request::builder()
            .header("authorization", "Bearer abc")
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();
        let error = CurrentUser::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Internal(_)));
    }

    #[test]
    fn cookie_helpers_shape() {
        let set = session_cookie("tok", Duration::from_secs(60));
        assert!(set.contains("lumos_session=tok"), "{set}");
        assert!(set.contains("Max-Age=60"), "{set}");
        assert!(set.contains("HttpOnly"), "{set}");
        let clear = clear_session_cookie();
        assert!(clear.contains("Max-Age=0"), "{clear}");
    }
}
