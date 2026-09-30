//! Service providers: the two-phase boot lifecycle.
//!
//! Providers wire an application in two phases:
//!
//! 1. [`ServiceProvider::register`] (synchronous): bind services into the
//!    [`Container`](crate::Container) and mount routers. Runs immediately
//!    when the provider is registered, in registration order.
//! 2. [`ServiceProvider::boot`] (async): runs after *all* providers have
//!    registered, so every binding exists before anyone uses one.
//!
//! Registration is explicit (`app.register(MyProvider)`); there is no
//! runtime discovery. (The `lumos` CLI generates the registration list from
//! `config/app.toml` in Phase 5.)

pub use async_trait::async_trait;

use crate::{Application, Result};

/// Registers bindings, then boots once every provider has registered.
///
/// Providers must be `Send + Sync + 'static` because the application holds
/// them for the server's lifetime.
///
/// # Examples
///
/// ```rust
/// use lumos_core::{async_trait, Application, Result, ServiceProvider};
///
/// struct AppProvider;
///
/// #[async_trait]
/// impl ServiceProvider for AppProvider {
///     fn register(&self, app: &mut Application) -> Result<()> {
///         app.container_mut().singleton_value("ready".to_string());
///         Ok(())
///     }
///
///     async fn boot(&self, app: &Application) -> Result<()> {
///         let value: String = app.container().resolve_value()?;
///         assert_eq!(value, "ready");
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait ServiceProvider: Send + Sync {
    /// Binds services into the container and mounts routers.
    ///
    /// Called synchronously from [`Application::register`], in registration
    /// order. Only bind here. Do not resolve bindings owned by another
    /// provider (it may not have registered yet); do that in [`boot`](ServiceProvider::boot).
    fn register(&self, app: &mut Application) -> Result<()>;

    /// Runs after every provider has registered.
    ///
    /// All bindings exist at this point, so resolving anything is safe. The
    /// default implementation does nothing.
    async fn boot(&self, _app: &Application) -> Result<()> {
        Ok(())
    }
}
