//! Sessions: plain-JSON login/logout beside the JSON:API resources.
//!
//! These routes are hand-written (not convention actions): `login` and
//! `logout` are inherent methods mounted explicitly in `build()`, which
//! is also how any non-RESTful route joins a Lumos app.

use std::sync::Arc;
use std::time::Duration;

use lumos::rusticate::{Model, DB};
use lumos::{controller, AppError, CurrentUser, Response, Result, SessionStore, Validated};
use serde::Serialize;

use crate::models::User;
use crate::resources::LoginInput;

/// Session lifetime: one week.
const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Sessions controller: database and store arrive via `from_container`.
#[controller]
pub struct SessionsController {
    db: DB,
    store: Arc<dyn SessionStore>,
}

impl SessionsController {
    /// Logs in with email + password: wrong credentials are 401, and the
    /// session lands in a cookie on success.
    pub async fn login(&self, Validated(input): Validated<LoginInput>) -> Result<Response> {
        let user = User::query(&self.db)
            .where_eq("email", input.email)
            .first()
            .await?
            .filter(|user| lumos::verify_password(&input.password, &user.password_hash))
            .ok_or(AppError::Unauthorized)?;
        let session = lumos::login(
            self.store.as_ref(),
            user.id.to_string(),
            vec!["write".to_string()],
            SESSION_TTL,
        )
        .await?;
        let mut response = lumos::ok(&SessionView {
            user_id: user.id,
            name: user.name.clone(),
        });
        insert_cookie(
            &mut response,
            lumos::session_cookie(&session.token, SESSION_TTL),
        )?;
        Ok(response)
    }

    /// Logs out the current session, clearing the cookie.
    pub async fn logout(&self, user: CurrentUser) -> Result<Response> {
        lumos::logout(self.store.as_ref(), &user.0.token).await?;
        let mut response = lumos::no_content();
        insert_cookie(&mut response, lumos::clear_session_cookie())?;
        Ok(response)
    }
}

/// Login response body: who the cookie now identifies.
#[derive(Serialize)]
struct SessionView {
    user_id: i64,
    name: String,
}

/// Sets `Set-Cookie`, mapping the (unreachable-in-practice) invalid
/// header value onto a 500 instead of panicking.
fn insert_cookie(response: &mut Response, cookie: String) -> Result<()> {
    let value = lumos::axum::http::HeaderValue::from_str(&cookie)
        .map_err(|_| AppError::internal("cannot render session cookie"))?;
    response
        .headers_mut()
        .insert(lumos::axum::http::header::SET_COOKIE, value);
    Ok(())
}
