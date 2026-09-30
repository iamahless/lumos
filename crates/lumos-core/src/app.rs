//! Application kernel: owns config, container, providers, and routes.
//!
//! [`Application`] is the composition root. Providers are registered
//! explicitly, routers are mounted explicitly, and [`Application::serve`]
//! boots everything exactly once before listening.

use crate::{Config, Container, Result, Router, ServiceProvider};

/// Composition root: config + container + providers + router.
///
/// A typical `main` builds the app, registers providers (which bind services
/// and mount controllers), then serves:
///
/// ```rust,no_run
/// use lumos_core::{Application, Config};
///
/// # #[tokio::main]
/// # async fn main() -> lumos_core::Result<()> {
/// let mut app = Application::load()?;
/// // app.register(AppProvider)?;
/// app.serve("127.0.0.1:3000").await
/// # }
/// ```
pub struct Application {
    container: Container,
    config: Config,
    router: Router,
    providers: Vec<Box<dyn ServiceProvider>>,
    booted: bool,
}

impl Application {
    /// Builds an application from already-loaded config.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{Application, Config};
    ///
    /// let config = Config::load_from("tests/fixtures/config").unwrap();
    /// let app = Application::new(config);
    /// assert_eq!(app.config().get::<String>("app.name").unwrap(), "blog");
    /// ```
    pub fn new(config: Config) -> Self {
        Self {
            container: Container::new(),
            config,
            router: Router::new(),
            providers: Vec::new(),
            booted: false,
        }
    }

    /// Builds an application with [`Config::load`] (`.env` + `config/*.toml`).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_core::Application;
    ///
    /// let app = Application::load()?;
    /// # Ok::<(), lumos_core::AppError>(())
    /// ```
    pub fn load() -> Result<Self> {
        Ok(Self::new(Config::load()?))
    }

    /// Returns the service container.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{Application, Config};
    ///
    /// let app = Application::new(Config::default());
    /// assert!(!app.container().has::<String>());
    /// ```
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Returns the service container mutably, for binding services.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{Application, Config};
    ///
    /// let mut app = Application::new(Config::default());
    /// app.container_mut().singleton_value(1u32);
    /// assert!(app.container().has::<u32>());
    /// ```
    pub fn container_mut(&mut self) -> &mut Container {
        &mut self.container
    }

    /// Returns the loaded configuration.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{Application, Config};
    ///
    /// let app = Application::new(Config::default());
    /// assert!(app.config().get::<String>("app.name").is_err());
    /// ```
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Registers a provider: its [`register`](ServiceProvider::register)
    /// runs immediately (in call order); its [`boot`](ServiceProvider::boot)
    /// is deferred until [`boot`](Application::boot).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{Application, Config, Result, ServiceProvider};
    ///
    /// struct AppProvider;
    /// impl ServiceProvider for AppProvider {
    ///     fn register(&self, app: &mut Application) -> Result<()> {
    ///         app.container_mut().singleton_value(1u8);
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let mut app = Application::new(Config::default());
    /// app.register(AppProvider).unwrap();
    /// assert!(app.container().has::<u8>());
    /// ```
    pub fn register<P>(&mut self, provider: P) -> Result<()>
    where
        P: ServiceProvider + 'static,
    {
        provider.register(self)?;
        self.providers.push(Box::new(provider));
        Ok(())
    }

    /// Merges a router into the application (route mounting).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{get, Application, Config};
    ///
    /// let mut app = Application::new(Config::default());
    /// app.mount(lumos_core::Router::new().route("/health", get(|| async { "ok" })));
    /// ```
    pub fn mount(&mut self, router: Router) -> &mut Self {
        let current = std::mem::replace(&mut self.router, Router::new());
        self.router = current.merge(router);
        self
    }

    /// Runs every provider's [`boot`](ServiceProvider::boot) hook, once.
    ///
    /// All providers have registered by now, so resolving any binding is
    /// safe. Calling `boot` again is a no-op.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> lumos_core::Result<()> {
    /// use lumos_core::{Application, Config};
    ///
    /// let mut app = Application::new(Config::default());
    /// app.boot().await?;
    /// app.boot().await?; // idempotent
    /// # Ok(())
    /// # }
    /// ```
    pub async fn boot(&mut self) -> Result<()> {
        if self.booted {
            return Ok(());
        }
        for provider in &self.providers {
            provider.boot(self).await?;
        }
        self.booted = true;
        Ok(())
    }

    /// Boots (if needed) and serves HTTP until Ctrl-C.
    ///
    /// The shutdown path is graceful: in-flight requests complete before
    /// the server stops. Binding or I/O failures are [`AppError::Internal`]
    /// with operator context.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[tokio::main]
    /// # async fn main() -> lumos_core::Result<()> {
    /// use lumos_core::{Application, Config};
    ///
    /// Application::new(Config::default()).serve("127.0.0.1:3000").await
    /// # }
    /// ```
    pub async fn serve(mut self, address: &str) -> Result<()> {
        self.boot().await?;
        crate::router::serve(self.router, address).await
    }

    /// Consumes the application and returns its router.
    ///
    /// Escape hatch for tests (in-process requests without binding a port),
    /// custom servers, and embedding Lumos routers in larger axum apps.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_core::{get, Application, Config};
    ///
    /// let mut app = Application::new(Config::default());
    /// app.mount(lumos_core::Router::new().route("/", get(|| async { "hi" })));
    /// let _router = app.into_router();
    /// ```
    pub fn into_router(self) -> Router {
        self.router
    }
}

impl std::fmt::Debug for Application {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Application")
            .field("container", &self.container)
            .field("providers", &self.providers.len())
            .field("booted", &self.booted)
            .finish()
    }
}
