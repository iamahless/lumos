//! Command execution: serve, migrate, seed, route:list, app dispatch.
//!
//! [`ServeOptions`] shapes the `cargo run` invocation ([`serve_argv`] is
//! pure and tested; [`run_serve`] spawns it with inherited stdio).
//! [`migrate`], [`rollback_migrations`], [`migration_status`], and [`seed`]
//! wrap the rusticate APIs with printable reports. [`AppContext`] bundles
//! what app-linked commands need (database, migrations, seeders,
//! registry); [`run_app`] parses and dispatches for the app's CLI binary.

use std::fmt;
use std::future::Future;
use std::process::Command as ProcCommand;

use lumos_core::RouteRegistry;
use rusticate::{Migration, MigrationStatus, Migrator, Seeder, DB};

use crate::args::{command_help, parse, Command};
use crate::error::CliError;

/// Dev-server options (see `serve --help`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::ServeOptions;
///
/// let options = ServeOptions { port: 8080, host: "127.0.0.1".to_string(), release: false, passthrough: vec![] };
/// assert_eq!(options.port, 8080);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeOptions {
    /// Port; sets `PORT`.
    pub port: u16,
    /// Host; sets `HOST`.
    pub host: String,
    /// Pass `--release` to cargo.
    pub release: bool,
    /// Arguments after `--`, forwarded to the app.
    pub passthrough: Vec<String>,
}

/// Builds the `cargo run …` argument vector (without the program name).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{serve_argv, ServeOptions};
///
/// let options = ServeOptions { port: 8080, host: "0.0.0.0".to_string(), release: true, passthrough: vec!["--foo".to_string()] };
/// assert_eq!(serve_argv(&options), vec!["run", "--release", "--", "--foo"]);
/// ```
pub fn serve_argv(options: &ServeOptions) -> Vec<String> {
    let mut argv = vec!["run".to_string()];
    if options.release {
        argv.push("--release".to_string());
    }
    if !options.passthrough.is_empty() {
        argv.push("--".to_string());
        argv.extend(options.passthrough.iter().cloned());
    }
    argv
}

/// Builds the `PORT`/`HOST` environment (plus `RUST_LOG` defaulted to
/// `info` when unset).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{serve_env, ServeOptions};
///
/// let options = ServeOptions { port: 8080, host: "0.0.0.0".to_string(), release: false, passthrough: Vec::new() };
/// let env = serve_env(&options);
/// assert!(env.contains(&("PORT".to_string(), "8080".to_string())));
/// ```
pub fn serve_env(options: &ServeOptions) -> Vec<(String, String)> {
    let mut env = vec![
        ("PORT".to_string(), options.port.to_string()),
        ("HOST".to_string(), options.host.clone()),
    ];
    if std::env::var_os("RUST_LOG").is_none() {
        env.push(("RUST_LOG".to_string(), "info".to_string()));
    }
    env
}

/// Runs the dev server: `cargo run` in the current directory with
/// [`serve_env`] applied and inherited stdio (Ctrl-C stops the child).
/// A missing `cargo` and spawn failures are errors naming the cause; a
/// failing child propagates its exit code silently (it already printed).
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_cli::{run_serve, ServeOptions};
///
/// let options = ServeOptions { port: 3000, host: "127.0.0.1".to_string(), release: false, passthrough: Vec::new() };
/// run_serve(&options).unwrap();
/// ```
pub fn run_serve(options: &ServeOptions) -> Result<(), CliError> {
    let mut command = ProcCommand::new("cargo");
    command.args(serve_argv(options));
    for (name, value) in serve_env(options) {
        command.env(name, value);
    }
    let status = command.status().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => {
            CliError::new("`cargo` not found in PATH: serve runs `cargo run` here.")
        }
        _ => CliError::new(format!("failed to start server: {error}")),
    })?;
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(CliError::with_code("", code)),
        None => Err(CliError::new("server stopped by signal.")),
    }
}

/// Report of a `migrate` run: applied names in order.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::MigrateReport;
///
/// let report = MigrateReport { applied: vec![] };
/// assert_eq!(report.to_string(), "Nothing to migrate.");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateReport {
    /// Applied migration names (empty when nothing was pending).
    pub applied: Vec<String>,
}

impl fmt::Display for MigrateReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.applied.is_empty() {
            formatter.write_str("Nothing to migrate.")
        } else {
            writeln!(formatter, "Migrated {}:", self.applied.len())?;
            for name in &self.applied {
                writeln!(formatter, "  {name}")?;
            }
            Ok(())
        }
    }
}

/// Runs pending migrations in list order, recording one batch.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::migrate;
/// use rusticate::{async_trait, Migration, Result, Schema, DB};
///
/// struct CreateUsers;
/// #[async_trait]
/// impl Migration for CreateUsers {
///     async fn up(&self, schema: &mut Schema) -> Result<()> {
///         schema.create("users", |t| { t.id(); }).await
///     }
///     async fn down(&self, schema: &mut Schema) -> Result<()> {
///         schema.drop("users").await
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let report = migrate(&db, &[&CreateUsers]).await?;
/// assert_eq!(report.applied.len(), 1);
/// # Ok(())
/// # }
/// ```
pub async fn migrate(db: &DB, migrations: &[&dyn Migration]) -> rusticate::Result<MigrateReport> {
    let applied = Migrator::new(db).run(migrations).await?;
    Ok(MigrateReport { applied })
}

/// Report of a `migrate:rollback` run: reverted names in reverse order.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::RollbackReport;
///
/// let report = RollbackReport { reverted: vec![] };
/// assert_eq!(report.to_string(), "Nothing to roll back.");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackReport {
    /// Reverted migration names (empty when no batches applied).
    pub reverted: Vec<String>,
}

impl fmt::Display for RollbackReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.reverted.is_empty() {
            formatter.write_str("Nothing to roll back.")
        } else {
            writeln!(formatter, "Rolled back {}:", self.reverted.len())?;
            for name in &self.reverted {
                writeln!(formatter, "  {name}")?;
            }
            Ok(())
        }
    }
}

/// Reverts the last `batches` batches in reverse order.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{migrate, rollback_migrations};
/// use rusticate::{async_trait, Migration, Result, Schema, DB};
///
/// struct CreateUsers;
/// #[async_trait]
/// impl Migration for CreateUsers {
///     async fn up(&self, schema: &mut Schema) -> Result<()> {
///         schema.create("users", |t| { t.id(); }).await
///     }
///     async fn down(&self, schema: &mut Schema) -> Result<()> {
///         schema.drop("users").await
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// migrate(&db, &[&CreateUsers]).await?;
/// let report = rollback_migrations(&db, &[&CreateUsers], 1).await?;
/// assert_eq!(report.reverted.len(), 1);
/// # Ok(())
/// # }
/// ```
pub async fn rollback_migrations(
    db: &DB,
    migrations: &[&dyn Migration],
    batches: usize,
) -> rusticate::Result<RollbackReport> {
    let reverted = Migrator::new(db).rollback(migrations, batches).await?;
    Ok(RollbackReport { reverted })
}

/// Report of a `migrate:status` run: one row per registered migration.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::StatusReport;
///
/// let report = StatusReport { rows: vec![] };
/// assert_eq!(report.to_string(), "No migrations registered.");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// Rows in registry order.
    pub rows: Vec<MigrationStatus>,
}

impl fmt::Display for StatusReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rows.is_empty() {
            return formatter.write_str("No migrations registered.");
        }
        let name_width = self
            .rows
            .iter()
            .map(|row| row.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        writeln!(formatter, "{:<name_width$}  BATCH  RAN", "NAME")?;
        for row in &self.rows {
            let batch = row
                .batch
                .map(|batch| batch.to_string())
                .unwrap_or_else(|| "-".to_string());
            let ran = if row.ran { "yes" } else { "no" };
            writeln!(formatter, "{:<name_width$}  {batch:<5}  {ran}", row.name)?;
        }
        Ok(())
    }
}

/// Lists registered migrations with batch numbers and ran state.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{migrate, migration_status};
/// use rusticate::{async_trait, Migration, Result, Schema, DB};
///
/// struct CreateUsers;
/// #[async_trait]
/// impl Migration for CreateUsers {
///     async fn up(&self, schema: &mut Schema) -> Result<()> {
///         schema.create("users", |t| { t.id(); }).await
///     }
///     async fn down(&self, schema: &mut Schema) -> Result<()> {
///         schema.drop("users").await
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let before = migration_status(&db, &[&CreateUsers]).await?;
/// assert!(!before.rows[0].ran);
/// migrate(&db, &[&CreateUsers]).await?;
/// let after = migration_status(&db, &[&CreateUsers]).await?;
/// assert!(after.rows[0].ran);
/// # Ok(())
/// # }
/// ```
pub async fn migration_status(
    db: &DB,
    migrations: &[&dyn Migration],
) -> rusticate::Result<StatusReport> {
    let rows = Migrator::new(db).status(migrations).await?;
    Ok(StatusReport { rows })
}

/// Report of a `db:seed` run: seeder names in run order.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::SeedReport;
///
/// let report = SeedReport { ran: vec!["AdminSeeder".to_string()] };
/// assert!(report.to_string().contains("AdminSeeder"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedReport {
    /// Seeder names that ran.
    pub ran: Vec<String>,
}

impl fmt::Display for SeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ran.is_empty() {
            formatter.write_str("No seeders registered.")
        } else {
            writeln!(formatter, "Seeded {}:", self.ran.len())?;
            for name in &self.ran {
                writeln!(formatter, "  {name}")?;
            }
            Ok(())
        }
    }
}

/// Runs seeders in list order, or only `filter` when given (matched by
/// [`Seeder::name`]; unknown names fail naming the valid set).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::seed;
/// use rusticate::{async_trait, Result, Seeder, DB};
///
/// struct AdminSeeder;
/// #[async_trait]
/// impl Seeder for AdminSeeder {
///     async fn run(&self, _db: &DB) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> rusticate::Result<()> {
/// let db = DB::memory().await?;
/// let report = seed(&db, &[&AdminSeeder], None).await?;
/// assert_eq!(report.ran.len(), 1);
/// # Ok(())
/// # }
/// ```
pub async fn seed(
    db: &DB,
    seeders: &[&dyn Seeder],
    filter: Option<&str>,
) -> rusticate::Result<SeedReport> {
    let selected: Vec<&dyn Seeder> = match filter {
        None => seeders.to_vec(),
        Some(wanted) => {
            let found: Vec<&dyn Seeder> = seeders
                .iter()
                .copied()
                .filter(|seeder| seeder.name() == wanted)
                .collect();
            if found.is_empty() {
                let valid: Vec<String> = seeders.iter().map(|seeder| seeder.name()).collect();
                return Err(rusticate::Error::NotFound(format!(
                    "unknown seeder `{wanted}`: expected one of {}",
                    valid.join(", ")
                )));
            }
            found
        }
    };
    let mut ran = Vec::with_capacity(selected.len());
    for seeder in selected {
        seeder.run(db).await?;
        ran.push(seeder.name());
    }
    Ok(SeedReport { ran })
}

/// Renders the `route:list` table (METHOD / PATH / ACTION, registration
/// order, columns sized to content). Empty registries say so with the fix.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::route_list;
/// use lumos_core::RouteRegistry;
///
/// let mut registry = RouteRegistry::new();
/// registry.route("GET", "/health", "health");
/// let table = route_list(&registry);
/// assert!(table.contains("GET"));
/// assert!(table.contains("/health"));
/// ```
pub fn route_list(registry: &RouteRegistry) -> String {
    if registry.is_empty() {
        return "No routes registered: call RouteRegistry::resource next to each mount.\n"
            .to_string();
    }
    let entries = registry.entries();
    let method_width = entries
        .iter()
        .map(|entry| entry.method.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let path_width = entries
        .iter()
        .map(|entry| entry.path.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let mut table = format!(
        "{:<method_width$}  {:<path_width$}  ACTION\n",
        "METHOD", "PATH"
    );
    for entry in entries {
        table.push_str(&format!(
            "{:<method_width$}  {:<path_width$}  {}\n",
            entry.method, entry.path, entry.action
        ));
    }
    table
}

/// What app-linked commands need: database, migrations, seeders, registry.
/// Built with the builder methods; only `registry` is required (database
/// commands fail with a direction error when `db` is absent).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::AppContext;
/// use lumos_core::RouteRegistry;
///
/// let context = AppContext::new().registry(RouteRegistry::new());
/// assert!(context.db.is_none());
/// ```
#[derive(Default)]
pub struct AppContext {
    /// Database for migrate/seed commands.
    pub db: Option<DB>,
    /// Migrations in run order.
    pub migrations: Vec<Box<dyn Migration>>,
    /// Seeders in run order.
    pub seeders: Vec<Box<dyn Seeder>>,
    /// Route registry for `route:list`.
    pub registry: RouteRegistry,
}

impl AppContext {
    /// Builds an empty context (registry present but unregistered).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::AppContext;
    ///
    /// let context = AppContext::new();
    /// assert!(context.registry.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches the database for migrate/seed commands.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lumos_cli::AppContext;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> rusticate::Result<()> {
    /// let db = rusticate::DB::memory().await?;
    /// let context = AppContext::new().db(db);
    /// assert!(context.db.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub fn db(mut self, db: DB) -> Self {
        self.db = Some(db);
        self
    }

    /// Registers one migration (run order = call order).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::AppContext;
    /// use rusticate::{async_trait, Migration, Result, Schema};
    ///
    /// struct CreateUsers;
    /// #[async_trait]
    /// impl Migration for CreateUsers {
    ///     async fn up(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.create("users", |t| { t.id(); }).await
    ///     }
    ///     async fn down(&self, schema: &mut Schema) -> Result<()> {
    ///         schema.drop("users").await
    ///     }
    /// }
    ///
    /// let context = AppContext::new().migration(CreateUsers);
    /// assert_eq!(context.migrations.len(), 1);
    /// ```
    pub fn migration<M: Migration + 'static>(mut self, migration: M) -> Self {
        self.migrations.push(Box::new(migration));
        self
    }

    /// Registers one seeder (run order = call order).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::AppContext;
    /// use rusticate::{async_trait, Result, Seeder, DB};
    ///
    /// struct AdminSeeder;
    /// #[async_trait]
    /// impl Seeder for AdminSeeder {
    ///     async fn run(&self, _db: &DB) -> Result<()> {
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let context = AppContext::new().seeder(AdminSeeder);
    /// assert_eq!(context.seeders.len(), 1);
    /// ```
    pub fn seeder<S: Seeder + 'static>(mut self, seeder: S) -> Self {
        self.seeders.push(Box::new(seeder));
        self
    }

    /// Attaches the route registry for `route:list`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::AppContext;
    /// use lumos_core::RouteRegistry;
    ///
    /// let mut registry = RouteRegistry::new();
    /// registry.route("GET", "/health", "health");
    /// let context = AppContext::new().registry(registry);
    /// assert_eq!(context.registry.len(), 1);
    /// ```
    pub fn registry(mut self, registry: RouteRegistry) -> Self {
        self.registry = registry;
        self
    }

    fn migration_refs(&self) -> Vec<&dyn Migration> {
        self.migrations
            .iter()
            .map(|migration| migration.as_ref())
            .collect()
    }

    fn seeder_refs(&self) -> Vec<&dyn Seeder> {
        self.seeders.iter().map(|seeder| seeder.as_ref()).collect()
    }
}

impl fmt::Debug for AppContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppContext")
            .field("db", &self.db.is_some())
            .field("migrations", &self.migrations.len())
            .field("seeders", &self.seeders.len())
            .field("registry", &self.registry.len())
            .finish()
    }
}

/// Parses and runs for the app's CLI binary; returns the exit code.
/// Project-local commands (`new`, `serve`, `make:*`) answer with a
/// direction error (run `lumos` itself); database commands without a
/// configured `db` explain how to attach one.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{run_app, AppContext};
///
/// # #[tokio::main]
/// # async fn main() {
/// let context = AppContext::new();
/// let code = run_app(&["route:list".to_string()], &context).await;
/// assert_eq!(code, 0);
/// # }
/// ```
pub async fn run_app(argv: &[String], context: &AppContext) -> i32 {
    let command = match parse(argv) {
        Ok(command) => command,
        Err(error) => {
            eprint_cli_error(&error);
            return error.code();
        }
    };
    match command {
        Command::Migrate => {
            run_db_command(context, "migrate", |db, ctx| {
                Box::pin(async move {
                    migrate(db, &ctx.migration_refs())
                        .await
                        .map(|report| report.to_string())
                })
            })
            .await
        }
        Command::MigrateRollback { batches } => {
            run_db_command(context, "migrate:rollback", |db, ctx| {
                Box::pin(async move {
                    rollback_migrations(db, &ctx.migration_refs(), batches)
                        .await
                        .map(|report| report.to_string())
                })
            })
            .await
        }
        Command::MigrateStatus => {
            run_db_command(context, "migrate:status", |db, ctx| {
                Box::pin(async move {
                    migration_status(db, &ctx.migration_refs())
                        .await
                        .map(|report| report.to_string())
                })
            })
            .await
        }
        Command::Seed { seeder } => {
            run_db_command(context, "db:seed", |db, ctx| {
                Box::pin(async move {
                    seed(db, &ctx.seeder_refs(), seeder.as_deref())
                        .await
                        .map(|report| report.to_string())
                })
            })
            .await
        }
        Command::RouteList => {
            print!("{}", route_list(&context.registry));
            0
        }
        Command::Help { command } => {
            match command.as_deref().and_then(command_help) {
                Some(text) => println!("{text}"),
                None => println!("{}", app_help()),
            }
            0
        }
        Command::Version => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            0
        }
        Command::New { .. } | Command::Serve { .. } | Command::Make { .. } => {
            eprintln!("{}", bin_command_hint());
            1
        }
    }
}

/// Runs one database command against the context's `db`, printing the
/// report or the failure. Missing `db` explains the fix (not a stack trace).
async fn run_db_command(
    context: &AppContext,
    name: &str,
    run: impl for<'a> FnOnce(
        &'a DB,
        &'a AppContext,
    )
        -> std::pin::Pin<Box<dyn Future<Output = rusticate::Result<String>> + 'a>>,
) -> i32 {
    let Some(db) = context.db.as_ref() else {
        eprintln!(
            "`{name}` needs a database: attach one with AppContext::db (see src/bin/cli.rs)."
        );
        return 1;
    };
    match run(db, context).await {
        Ok(report) => {
            print!("{report}");
            if !report.ends_with('\n') {
                println!();
            }
            0
        }
        Err(error) => {
            eprintln!("{name} failed: {error}");
            1
        }
    }
}

/// Prints a [`CliError`] unless its message is empty (child already spoke).
fn eprint_cli_error(error: &CliError) {
    if !error.message().is_empty() {
        eprintln!("{error}");
    }
}

/// App-scoped help: the commands an app binary actually runs.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::app_help;
///
/// assert!(app_help().contains("migrate"));
/// assert!(!app_help().contains("make:controller"));
/// ```
pub fn app_help() -> &'static str {
    "app CLI — migrate, seed, and route commands for this application\n\
     \n\
     Usage: cargo run --bin cli -- <command> [args]\n\
     \n\
     ·   migrate                       run pending migrations\n\
     ·   migrate:rollback [--batches N]  revert last batch (default 1)\n\
     ·   migrate:status                list migrations and batches\n\
     ·   db:seed [--seeder Name]       run seeders\n\
     ·   route:list                    list registered routes\n\
     \n\
     Project commands (new, serve, make:*) run via the `lumos` binary."
}

/// Hint naming the `lumos` binary for project-local commands issued to an
/// app binary. (Kept beside [`crate::app_command_hint`], which points the other way.)
///
/// # Examples
///
/// ```rust
/// use lumos_cli::bin_command_hint;
///
/// assert!(bin_command_hint().contains("lumos"));
/// ```
pub fn bin_command_hint() -> &'static str {
    "run project commands with the `lumos` binary, not the app CLI."
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusticate::{async_trait, Result, Schema};

    struct CreateUsers;
    #[async_trait]
    impl Migration for CreateUsers {
        async fn up(&self, schema: &mut Schema) -> Result<()> {
            schema
                .create("users", |t| {
                    t.id();
                })
                .await
        }
        async fn down(&self, schema: &mut Schema) -> Result<()> {
            schema.drop("users").await
        }
    }

    struct AdminSeeder;
    #[async_trait]
    impl Seeder for AdminSeeder {
        fn name(&self) -> String {
            "AdminSeeder".to_string()
        }
        async fn run(&self, _db: &DB) -> Result<()> {
            Ok(())
        }
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn serve_argv_shapes_cargo_invocation() {
        let plain = ServeOptions {
            port: 3000,
            host: "127.0.0.1".to_string(),
            release: false,
            passthrough: Vec::new(),
        };
        assert_eq!(serve_argv(&plain), vec!["run".to_string()]);
        let full = ServeOptions {
            port: 8080,
            host: "0.0.0.0".to_string(),
            release: true,
            passthrough: vec!["--foo".to_string()],
        };
        assert_eq!(serve_argv(&full), vec!["run", "--release", "--", "--foo"]);
        let env = serve_env(&full);
        assert!(env.contains(&("PORT".to_string(), "8080".to_string())));
        assert!(env.contains(&("HOST".to_string(), "0.0.0.0".to_string())));
    }

    #[tokio::test]
    async fn runtime_migrate_rollback_status_seed_cycle() {
        let db = DB::memory().await.unwrap();
        let migrations: &[&dyn Migration] = &[&CreateUsers];
        let seeders: &[&dyn Seeder] = &[&AdminSeeder];

        let status = migration_status(&db, migrations).await.unwrap();
        assert!(!status.rows[0].ran);
        assert!(status.to_string().contains("CreateUsers"));

        let migrated = migrate(&db, migrations).await.unwrap();
        assert_eq!(migrated.applied.len(), 1);
        let again = migrate(&db, migrations).await.unwrap();
        assert!(again.applied.is_empty());

        let seeded = seed(&db, seeders, None).await.unwrap();
        assert_eq!(seeded.ran, vec!["AdminSeeder".to_string()]);
        let filtered = seed(&db, seeders, Some("AdminSeeder")).await.unwrap();
        assert_eq!(filtered.ran.len(), 1);
        assert!(seed(&db, seeders, Some("Nope")).await.is_err());

        let rolled = rollback_migrations(&db, migrations, 1).await.unwrap();
        assert_eq!(rolled.reverted.len(), 1);
        let status = migration_status(&db, migrations).await.unwrap();
        assert!(!status.rows[0].ran);
    }

    #[tokio::test]
    async fn runtime_run_app_dispatch_and_missing_db() {
        let mut registry = RouteRegistry::new();
        registry.route("GET", "/health", "health");
        let context = AppContext::new().registry(registry);
        // route:list works without a database.
        assert_eq!(run_app(&argv(&["route:list"]), &context).await, 0);
        // Database commands without a db fail with direction, not panic.
        assert_eq!(run_app(&argv(&["migrate"]), &context).await, 1);
        assert_eq!(run_app(&argv(&["db:seed"]), &context).await, 1);
        // Project commands point back at the binary.
        assert_eq!(run_app(&argv(&["serve"]), &context).await, 1);
        // Help and version always work.
        assert_eq!(run_app(&argv(&["help"]), &context).await, 0);
        assert_eq!(run_app(&argv(&["--version"]), &context).await, 0);

        let db = DB::memory().await.unwrap();
        let full = AppContext::new()
            .db(db)
            .migration(CreateUsers)
            .seeder(AdminSeeder)
            .registry(RouteRegistry::new());
        assert_eq!(run_app(&argv(&["migrate"]), &full).await, 0);
        assert_eq!(run_app(&argv(&["migrate:status"]), &full).await, 0);
        assert_eq!(
            run_app(&argv(&["db:seed", "--seeder", "AdminSeeder"]), &full).await,
            0
        );
        assert_eq!(run_app(&argv(&["migrate:rollback"]), &full).await, 0);
    }

    #[test]
    fn runtime_route_table_aligns_and_explains_empty() {
        let table = route_list(&RouteRegistry::new());
        assert!(table.contains("No routes registered"));
        let mut registry = RouteRegistry::new();
        registry.route("GET", "/users/{id}", "UserController::show");
        registry.route("DELETE", "/u", "x");
        let table = route_list(&registry);
        assert!(table.starts_with("METHOD"));
        let delete_line = table
            .lines()
            .find(|line| line.starts_with("DELETE"))
            .unwrap();
        let get_line = table.lines().find(|line| line.starts_with("GET")).unwrap();
        assert_eq!(delete_line.find("/u"), get_line.find("/users/{id}"));
    }
}
