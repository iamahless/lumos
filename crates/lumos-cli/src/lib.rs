//! `lumos` developer CLI: project scaffolding, generators, dev server,
//! migrations, seeding, and route listing.
//!
//! The crate is a library plus a thin binary. The `lumos` binary runs
//! project-local commands ([`run`]): `new`, `serve`, `make:*`, `help`,
//! `version`. App-linked commands (`migrate`, `db:seed`, `route:list`)
//! need app code, so apps call [`run_app`] with an [`AppContext`] from
//! their own CLI binary (the `new` template wires this); the `lumos`
//! binary answers those with a direction error instead of pretending.
//!
//! # Examples
//!
//! ```rust
//! use lumos_cli::run;
//!
//! // Help exits zero.
//! assert_eq!(run(&[]), 0);
//! assert_eq!(run(&["--version".to_string()]), 0);
//! // Unknown commands fail loudly.
//! assert_eq!(run(&["frobnicate".to_string()]), 1);
//! ```

#![warn(missing_docs)]

pub mod args;
pub mod error;
pub mod generate;
pub mod runtime;

pub use args::{
    app_command_hint, command_help, global_help, is_app_command, parse, Command, MakeKind,
};
pub use error::CliError;
pub use generate::{
    migration_timestamp, render_controller, render_make, render_middleware, render_migration,
    render_model, render_new, render_resource, render_seeder, to_camel, to_snake, to_table,
    validate_crate_name, validate_type_name, write_tree, Generated, GeneratedFile,
};
pub use lumos_core::RouteRegistry;
pub use runtime::{
    app_help, bin_command_hint, migrate, migration_status, rollback_migrations, route_list,
    run_app, run_serve, seed, serve_argv, serve_env, AppContext, MigrateReport, RollbackReport,
    SeedReport, ServeOptions, StatusReport,
};

use std::path::PathBuf;

/// Runs the `lumos` binary: parses `argv`, executes project-local
/// commands, prints results, and returns the exit code. App-linked
/// commands get a direction error pointing at the app binary.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::run;
///
/// assert_eq!(run(&["help".to_string(), "serve".to_string()]), 0);
/// ```
pub fn run(argv: &[String]) -> i32 {
    let command = match parse(argv) {
        Ok(command) => command,
        Err(error) => {
            eprint_error(&error);
            return error.code();
        }
    };
    if is_app_command(&command) {
        eprintln!("{}", app_command_hint(&command));
        return 1;
    }
    match execute(command) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            0
        }
        Err(error) => {
            eprint_error(&error);
            error.code()
        }
    }
}

/// Executes one project-local command, returning its stdout (possibly
/// empty. `serve` streams through the child instead.
fn execute(command: Command) -> Result<String, CliError> {
    match command {
        Command::New { name, path, force } => {
            let generated = generate::render_new(&name, env!("CARGO_PKG_VERSION"))?;
            let root = path.unwrap_or_else(|| PathBuf::from(&name));
            if !force && root.exists() {
                return Err(CliError::new(format!(
                    "`{}` already exists: re-run with --force to overwrite.",
                    root.display()
                )));
            }
            let written = generate::write_tree(&root, &generated.files, force)?;
            let mut output = format!("Created {} ({} files):", root.display(), written.len());
            for file in &generated.files {
                output.push_str(&format!("\n  {}", file.path.display()));
            }
            for note in &generated.notes {
                output.push_str(&format!("\n  note: {note}"));
            }
            Ok(output)
        }
        Command::Serve {
            port,
            host,
            release,
            passthrough,
        } => {
            let options = ServeOptions {
                port,
                host: host.clone(),
                release,
                passthrough,
            };
            println!("Serving at http://{host}:{port} (Ctrl-C to stop)…");
            run_serve(&options).map(|()| String::new())
        }
        Command::Make { kind, name, force } => {
            let stamp = generate::migration_timestamp();
            let generated = generate::render_make(kind, &name, &stamp)?;
            let root = std::env::current_dir().map_err(|error| {
                CliError::new(format!("cannot resolve working directory: {error}"))
            })?;
            let written = generate::write_tree(&root, &generated.files, force)?;
            let mut output = String::from("Created:");
            for path in &written {
                output.push_str(&format!("\n  {}", path.display()));
            }
            for note in &generated.notes {
                output.push_str(&format!("\n  note: {note}"));
            }
            Ok(output)
        }
        Command::Help { command } => match command.as_deref().and_then(command_help) {
            Some(text) => Ok(text.to_string()),
            None => Ok(global_help().to_string()),
        },
        Command::Version => Ok(format!("lumos {}", env!("CARGO_PKG_VERSION"))),
        app_command => Err(CliError::new(app_command_hint(&app_command))),
    }
}

fn eprint_error(error: &CliError) {
    if !error.message().is_empty() {
        eprintln!("{error}");
    }
}
