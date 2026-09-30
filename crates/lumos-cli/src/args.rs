//! Hand-rolled argument parser: no dependency, full control over errors.
//!
//! Grammar: `lumos <command> [positionals] [--flag[=value]] [-- passthrough]`.
//! Flags take `--flag value` or `--flag=value`; `-h`/`--help` and `-V`/
//! `--version` work globally and per-command; `--` ends parsing (only
//! `serve` accepts passthrough). The parser checks syntax only. Name
//! validity and value ranges belong to the commands.
//!
//! # Examples
//!
//! ```rust
//! use lumos_cli::{parse, Command};
//!
//! let command = parse(&["new".to_string(), "blog".to_string()]).unwrap();
//! assert!(matches!(command, Command::New { name, .. } if name == "blog"));
//! ```

use std::path::PathBuf;

use crate::error::CliError;

/// Parsed top-level command.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{parse, Command};
///
/// assert!(matches!(parse(&[]).unwrap(), Command::Help { command: None }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Scaffold a project: `new <name> [--path <dir>] [--force]`.
    New {
        /// Project/package name (validated by the generator).
        name: String,
        /// Target directory (defaults to `./<name>`).
        path: Option<PathBuf>,
        /// Overwrite an existing directory.
        force: bool,
    },
    /// Run the dev server: `serve [--port N] [--host H] [--release] [-- args]`.
    Serve {
        /// Port (sets `PORT`); default 3000.
        port: u16,
        /// Host (sets `HOST`); default 127.0.0.1.
        host: String,
        /// Pass `--release` to cargo.
        release: bool,
        /// Arguments after `--`, forwarded to the app.
        passthrough: Vec<String>,
    },
    /// Generate a file: `make:<kind> <Name> [--force]`.
    Make {
        /// What to generate.
        kind: MakeKind,
        /// Type name (validated by the generator).
        name: String,
        /// Overwrite an existing file.
        force: bool,
    },
    /// Run pending migrations (app binary only).
    Migrate,
    /// Roll back migration batches (app binary only).
    MigrateRollback {
        /// Batches to revert; default 1.
        batches: usize,
    },
    /// Show migration status (app binary only).
    MigrateStatus,
    /// Run seeders (app binary only): `db:seed [--seeder Name]`.
    Seed {
        /// Run only this seeder (matched by name).
        seeder: Option<String>,
    },
    /// List registered routes (app binary only).
    RouteList,
    /// Show help (global, or for one command).
    Help {
        /// Command name, when `help <command>` or `<command> --help`.
        command: Option<String>,
    },
    /// Show the version.
    Version,
}

/// Generator kinds behind `make:*`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::MakeKind;
///
/// assert_eq!(MakeKind::parse("model"), Some(MakeKind::Model));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakeKind {
    /// `make:controller` → `src/controllers/<snake>.rs`.
    Controller,
    /// `make:model` → `src/models/<snake>.rs`.
    Model,
    /// `make:migration` → `database/migrations/<stamp>_<snake>.rs`.
    Migration,
    /// `make:seeder` → `database/seeders/<snake>.rs`.
    Seeder,
    /// `make:resource` → `src/resources/<snake>.rs`.
    Resource,
    /// `make:middleware` → `src/middleware/<snake>.rs`.
    Middleware,
}

impl MakeKind {
    /// Parses the `make:*` suffix.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::MakeKind;
    ///
    /// assert_eq!(MakeKind::parse("model"), Some(MakeKind::Model));
    /// assert_eq!(MakeKind::parse("widget"), None);
    /// ```
    pub fn parse(suffix: &str) -> Option<Self> {
        match suffix {
            "controller" => Some(Self::Controller),
            "model" => Some(Self::Model),
            "migration" => Some(Self::Migration),
            "seeder" => Some(Self::Seeder),
            "resource" => Some(Self::Resource),
            "middleware" => Some(Self::Middleware),
            _ => None,
        }
    }

    /// The `make:*` suffix.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_cli::MakeKind;
    ///
    /// assert_eq!(MakeKind::Migration.name(), "migration");
    /// ```
    pub fn name(&self) -> &'static str {
        match self {
            Self::Controller => "controller",
            Self::Model => "model",
            Self::Migration => "migration",
            Self::Seeder => "seeder",
            Self::Resource => "resource",
            Self::Middleware => "middleware",
        }
    }
}

/// Reports whether a command needs app code (migrations, seeders, routes).
/// The `lumos` binary answers these with a direction error; the app's CLI
/// binary runs them via the `runtime` module.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{is_app_command, Command};
///
/// assert!(is_app_command(&Command::Migrate));
/// assert!(!is_app_command(&Command::Version));
/// ```
pub fn is_app_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Migrate
            | Command::MigrateRollback { .. }
            | Command::MigrateStatus
            | Command::Seed { .. }
            | Command::RouteList
    )
}

/// Parse `argv` (without the program name) into a [`Command`].
///
/// Bare `lumos` prints global help; `--help`/`--version` (and `-h`/`-V`)
/// win anywhere they appear as the first argument or after the command.
/// Unknown commands and flags fail naming the culprit plus the valid set.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{parse, Command};
///
/// assert!(matches!(parse(&[]).unwrap(), Command::Help { command: None }));
/// let error = parse(&["frobnicate".to_string()]).unwrap_err();
/// assert!(error.to_string().contains("frobnicate"));
/// ```
pub fn parse(argv: &[String]) -> Result<Command, CliError> {
    let Some((head, rest)) = argv.split_first() else {
        return Ok(Command::Help { command: None });
    };
    match head.as_str() {
        "-h" | "--help" => return Ok(Command::Help { command: None }),
        "-V" | "--version" => return Ok(Command::Version),
        _ => {}
    }
    if head.starts_with('-') {
        return Err(CliError::new(format!(
            "expected a command, got flag `{head}`.\n\n{VALID_COMMANDS}"
        )));
    }
    let parsed = RawArgs::split(rest)?;
    if parsed.wants_help() {
        return help_for(head);
    }
    match head.as_str() {
        "new" => parse_new(&parsed),
        "serve" => parse_serve(&parsed),
        "migrate" => parsed.no_args("migrate").map(|()| Command::Migrate),
        "migrate:rollback" => parse_rollback(&parsed),
        "migrate:status" => parsed
            .no_args("migrate:status")
            .map(|()| Command::MigrateStatus),
        "db:seed" => parse_seed(&parsed),
        "route:list" => parsed.no_args("route:list").map(|()| Command::RouteList),
        "help" => parse_help_target(&parsed),
        "version" => parsed.no_args("version").map(|()| Command::Version),
        "make" => Err(CliError::new(format!("`make` needs a kind: {MAKE_KINDS}."))),
        other => match other.strip_prefix("make:") {
            Some(suffix) => match MakeKind::parse(suffix) {
                Some(kind) => parse_make(kind, &parsed),
                None => Err(CliError::new(format!(
                    "unknown generator `make:{suffix}`: expected one of {MAKE_KINDS}."
                ))),
            },
            None => Err(CliError::new(format!(
                "unknown command `{other}`.\n\n{VALID_COMMANDS}"
            ))),
        },
    }
}

const VALID_COMMANDS: &str = "Valid commands: new, serve, make:* (controller, model, migration, seeder, resource, middleware), migrate, migrate:rollback, migrate:status, db:seed, route:list, help, version.";
const MAKE_KINDS: &str = "controller, model, migration, seeder, resource, middleware";

/// Pre-split arguments: positionals, `--flag value` pairs, passthrough.
#[derive(Debug, Default, Clone)]
struct RawArgs {
    positionals: Vec<String>,
    flags: Vec<(String, Option<String>)>,
    passthrough: Vec<String>,
}

impl RawArgs {
    /// Splits raw tokens. `--name=value` and `--name value` both work;
    /// bare `--flag` records `None`; single `-x` (except `-h`) is rejected
    /// (`--force` has no short form, which keeps flags explicit).
    fn split(tokens: &[String]) -> Result<Self, CliError> {
        let mut args = Self::default();
        let mut rest = tokens.iter().peekable();
        while let Some(token) = rest.next() {
            if token == "--" {
                args.passthrough.extend(rest.cloned());
                break;
            }
            if let Some(name) = token.strip_prefix("--") {
                if name.is_empty() {
                    return Err(CliError::new(
                        "bare `--` needs a command before passthrough; put it after the flags.",
                    ));
                }
                match name.split_once('=') {
                    Some((key, value)) => {
                        args.flags
                            .push((format!("--{key}"), Some(value.to_string())));
                    }
                    None => {
                        let value = match rest.peek() {
                            Some(next) if !next.starts_with('-') => rest.next().cloned(),
                            _ => None,
                        };
                        args.flags.push((format!("--{name}"), value));
                    }
                }
                continue;
            }
            if token.starts_with('-') && token.len() > 1 {
                if token == "-h" {
                    args.flags.push(("-h".to_string(), None));
                    continue;
                }
                return Err(CliError::new(format!(
                    "unknown flag `{token}`: flags are `--like-this` (only -h is short)."
                )));
            }
            args.positionals.push(token.clone());
        }
        Ok(args)
    }

    fn wants_help(&self) -> bool {
        self.flags
            .iter()
            .any(|(name, _)| name == "--help" || name == "-h")
    }

    fn take_flag(&mut self, name: &str) -> Option<Option<String>> {
        let want = format!("--{name}");
        self.flags
            .iter()
            .position(|(flag, _)| flag == &want)
            .map(|index| self.flags.remove(index).1)
    }

    /// Rejects leftover positionals, flags, and passthrough for commands
    /// taking no arguments.
    fn no_args(&self, command: &str) -> Result<(), CliError> {
        if let Some(extra) = self.positionals.first() {
            return Err(CliError::new(format!(
                "`{command}` takes no arguments, got `{extra}`."
            )));
        }
        if let Some((flag, _)) = self.flags.first() {
            return Err(CliError::new(format!(
                "`{command}` takes no flags, got `{flag}`."
            )));
        }
        if !self.passthrough.is_empty() {
            return Err(CliError::new(format!(
                "`{command}` takes no `--` passthrough."
            )));
        }
        Ok(())
    }

    fn no_passthrough(&self, command: &str) -> Result<(), CliError> {
        if self.passthrough.is_empty() {
            Ok(())
        } else {
            Err(CliError::new(format!(
                "only `serve` accepts `--` passthrough (got it on `{command}`)."
            )))
        }
    }

    fn reject_unknown_flags(&self, command: &str) -> Result<(), CliError> {
        if let Some((flag, _)) = self.flags.first() {
            return Err(CliError::new(format!(
                "unknown flag `{flag}` for `{command}`."
            )));
        }
        Ok(())
    }
}

fn help_for(command: &str) -> Result<Command, CliError> {
    if command == "help" || command == "version" {
        return Ok(Command::Help { command: None });
    }
    if is_known(command) {
        return Ok(Command::Help {
            command: Some(command.to_string()),
        });
    }
    if command == "make" {
        return Ok(Command::Help {
            command: Some("make".to_string()),
        });
    }
    Err(CliError::new(format!(
        "unknown command `{command}`.\n\n{VALID_COMMANDS}"
    )))
}

fn is_known(command: &str) -> bool {
    matches!(
        command,
        "new"
            | "serve"
            | "migrate"
            | "migrate:rollback"
            | "migrate:status"
            | "db:seed"
            | "route:list"
    ) || command
        .strip_prefix("make:")
        .is_some_and(|suffix| MakeKind::parse(suffix).is_some())
}

fn parse_help_target(parsed: &RawArgs) -> Result<Command, CliError> {
    match parsed.positionals.as_slice() {
        [] => {
            parsed.reject_unknown_flags("help")?;
            parsed.no_passthrough("help")?;
            Ok(Command::Help { command: None })
        }
        [target] => {
            parsed.reject_unknown_flags("help")?;
            parsed.no_passthrough("help")?;
            help_for(target)
        }
        _ => Err(CliError::new("`help` takes at most one command.")),
    }
}

fn one_positional(parsed: &RawArgs, command: &str, what: &str) -> Result<String, CliError> {
    match parsed.positionals.as_slice() {
        [value] => Ok(value.clone()),
        [] => Err(CliError::new(format!("`{command}` needs a {what}."))),
        _ => Err(CliError::new(format!(
            "`{command}` takes one {what}, got {}.",
            parsed.positionals.len()
        ))),
    }
}

fn take_bool(parsed: &mut RawArgs, command: &str, name: &str) -> Result<bool, CliError> {
    match parsed.take_flag(name) {
        None => Ok(false),
        Some(None) => Ok(true),
        Some(Some(value)) => Err(CliError::new(format!(
            "`--{name}` on `{command}` takes no value, got `{value}`."
        ))),
    }
}

fn take_value(parsed: &mut RawArgs, command: &str, name: &str) -> Result<Option<String>, CliError> {
    match parsed.take_flag(name) {
        None => Ok(None),
        Some(None) => Err(CliError::new(format!(
            "`--{name}` on `{command}` needs a value."
        ))),
        Some(value) => Ok(value),
    }
}

fn parse_new(parsed: &RawArgs) -> Result<Command, CliError> {
    let mut parsed = parsed.clone();
    let name = one_positional(&parsed, "new", "project name")?;
    let path = take_value(&mut parsed, "new", "path")?.map(PathBuf::from);
    let force = take_bool(&mut parsed, "new", "force")?;
    parsed.no_passthrough("new")?;
    parsed.reject_unknown_flags("new")?;
    Ok(Command::New { name, path, force })
}

fn parse_serve(parsed: &RawArgs) -> Result<Command, CliError> {
    let mut parsed = parsed.clone();
    let port = match take_value(&mut parsed, "serve", "port")? {
        None => 3000,
        Some(raw) => raw
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= 1)
            .ok_or_else(|| CliError::new(format!("invalid --port `{raw}`: expected 1–65535.")))?,
    };
    let host = take_value(&mut parsed, "serve", "host")?.unwrap_or_else(|| "127.0.0.1".to_string());
    let release = take_bool(&mut parsed, "serve", "release")?;
    parsed.reject_unknown_flags("serve")?;
    Ok(Command::Serve {
        port,
        host,
        release,
        passthrough: parsed.passthrough.clone(),
    })
}

fn parse_make(kind: MakeKind, parsed: &RawArgs) -> Result<Command, CliError> {
    let mut parsed = parsed.clone();
    let command = format!("make:{}", kind.name());
    let name = one_positional(&parsed, &command, "name")?;
    let force = take_bool(&mut parsed, &command, "force")?;
    parsed.no_passthrough(&command)?;
    parsed.reject_unknown_flags(&command)?;
    Ok(Command::Make { kind, name, force })
}

fn parse_rollback(parsed: &RawArgs) -> Result<Command, CliError> {
    let mut parsed = parsed.clone();
    let batches = match take_value(&mut parsed, "migrate:rollback", "batches")? {
        None => 1,
        Some(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|batches| *batches >= 1)
            .ok_or_else(|| {
                CliError::new(format!(
                    "invalid --batches `{raw}`: expected an integer ≥ 1."
                ))
            })?,
    };
    parsed.no_passthrough("migrate:rollback")?;
    if !parsed.positionals.is_empty() {
        return Err(CliError::new("`migrate:rollback` takes no arguments."));
    }
    parsed.reject_unknown_flags("migrate:rollback")?;
    Ok(Command::MigrateRollback { batches })
}

fn parse_seed(parsed: &RawArgs) -> Result<Command, CliError> {
    let mut parsed = parsed.clone();
    let seeder = take_value(&mut parsed, "db:seed", "seeder")?;
    parsed.no_passthrough("db:seed")?;
    if !parsed.positionals.is_empty() {
        return Err(CliError::new("`db:seed` takes no arguments."));
    }
    parsed.reject_unknown_flags("db:seed")?;
    Ok(Command::Seed { seeder })
}

/// Global help text.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::global_help;
///
/// assert!(global_help().contains("new"));
/// assert!(global_help().contains("route:list"));
/// ```
pub fn global_help() -> &'static str {
    "lumos — artisan-like developer CLI\n\
     \n\
     Usage: lumos <command> [args] [--flag[=value]]\n\
     \n\
     Project commands (run anywhere):\n\
     ·   new <name> [--path <dir>] [--force]      scaffold a project\n\
     ·   serve [--port N] [--host H] [--release] [-- args]\n\
     ·                                             run the dev server via cargo\n\
     ·   make:<kind> <Name> [--force]             generate a file\n\
     ·       kinds: controller, model, migration, seeder, resource, middleware\n\
     \n\
     App commands (run from your app binary; see `new` template):\n\
     ·   migrate                                 run pending migrations\n\
     ·   migrate:rollback [--batches N]          revert last batch (default 1)\n\
     ·   migrate:status                          list migrations and batches\n\
     ·   db:seed [--seeder Name]                 run seeders\n\
     ·   route:list                              list registered routes\n\
     \n\
     ·   help [command] ·  version               this text ·  lumos version\n\
     \n\
     Flags: --flag value or --flag=value; -h everywhere shows help."
}

/// Per-command help, or [`None`] for unknown names.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::command_help;
///
/// assert!(command_help("serve").unwrap().contains("--port"));
/// assert!(command_help("frobnicate").is_none());
/// ```
pub fn command_help(command: &str) -> Option<&'static str> {
    match command {
        "new" => Some(
            "lumos new <name> [--path <dir>] [--force]\n\
             \n\
             Scaffolds an API project: lib + server + CLI bins, one controller,\n\
             config, README. Writes into ./<name> (or --path); refuses to\n\
             overwrite without --force.",
        ),
        "serve" => Some(
            "lumos serve [--port N] [--host H] [--release] [-- args]\n\
             \n\
             Runs `cargo run` in the project with PORT/HOST set from the flags\n\
             (defaults 3000 / 127.0.0.1) and RUST_LOG defaulted when unset.\n\
             Arguments after `--` reach the app verbatim.",
        ),
        "make" => Some(
            "lumos make:<kind> <Name> [--force]\n\
             \n\
             Generates one file (prints the `mod` line to add; never edits):\n\
             controller → src/controllers, model → src/models,\n\
             migration → database/migrations (timestamped), seeder →\n\
             database/seeders, resource → src/resources, middleware →\n\
             src/middleware. Refuses to overwrite without --force.",
        ),
        "migrate" => Some(
            "lumos migrate  — app binary only (see template src/bin/cli.rs).\n\
             \n\
             Runs pending migrations in registry order, recording one batch.\n\
             Prints applied names, or that nothing was pending.",
        ),
        "migrate:rollback" => Some(
            "lumos migrate:rollback [--batches N]  — app binary only.\n\
             \n\
             Reverts the last batch (default 1) in reverse order.",
        ),
        "migrate:status" => Some(
            "lumos migrate:status  — app binary only.\n\
             \n\
             Lists registered migrations with batch numbers and ran state.",
        ),
        "db:seed" => Some(
            "lumos db:seed [--seeder Name]  — app binary only.\n\
             \n\
             Runs every registered seeder in order, or only --seeder (matched\n\
             by seeder name).",
        ),
        "route:list" => Some(
            "lumos route:list  — app binary only.\n\
             \n\
             Prints the METHOD / PATH / ACTION table of the route registry.\n\
             Register controllers with RouteRegistry::resource next to each\n\
             mount; raw axum routes need manual registry.route calls.",
        ),
        _ => command
            .strip_prefix("make:")
            .and_then(MakeKind::parse)
            .map(|kind| match kind {
                MakeKind::Controller => "lumos make:controller <Name> [--force] → src/controllers/<snake>.rs: empty controller with an index action.",
                MakeKind::Model => "lumos make:model <Name> [--force] → src/models/<snake>.rs: Model derive with id + name, naive plural table.",
                MakeKind::Migration => "lumos make:migration <Name> [--force] → database/migrations/<stamp>_<snake>.rs: Migration skeleton (fill up/down, then register it).",
                MakeKind::Seeder => "lumos make:seeder <Name> [--force] → database/seeders/<snake>.rs: Seeder skeleton (fill run(), then register it).",
                MakeKind::Resource => "lumos make:resource <Name> [--force] → src/resources/<snake>.rs: JsonApiResource skeleton behind the jsonapi flags.",
                MakeKind::Middleware => "lumos make:middleware <Name> [--force] → src/middleware/<snake>.rs: from_fn middleware skeleton.",
            }),
    }
}

/// Direction error for app-linked commands invoked on the bare binary.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{app_command_hint, Command};
///
/// let hint = app_command_hint(&Command::Migrate);
/// assert!(hint.contains("cargo run"));
/// ```
pub fn app_command_hint(command: &Command) -> String {
    let name = match command {
        Command::Migrate => "migrate",
        Command::MigrateRollback { .. } => "migrate:rollback",
        Command::MigrateStatus => "migrate:status",
        Command::Seed { .. } => "db:seed",
        Command::RouteList => "route:list",
        _ => return String::new(),
    };
    format!(
        "`{name}` needs your app code, so it runs from your app binary (see the `new` template src/bin/cli.rs):\n  cargo run --bin cli -- {name}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn parses_every_subcommand() {
        assert!(matches!(
            parse(&argv(&[])).unwrap(),
            Command::Help { command: None }
        ));
        assert!(matches!(
            parse(&argv(&["new", "blog", "--path", "p", "--force"])).unwrap(),
            Command::New { name, path: Some(_), force: true } if name == "blog"
        ));
        assert!(matches!(
            parse(&argv(&["serve", "--port=8080", "--release", "--", "--foo"])).unwrap(),
            Command::Serve { port: 8080, release: true, passthrough, .. } if passthrough == vec!["--foo"]
        ));
        assert!(matches!(
            parse(&argv(&["make:model", "User"])).unwrap(),
            Command::Make {
                kind: MakeKind::Model,
                ..
            }
        ));
        assert!(matches!(
            parse(&argv(&["migrate"])).unwrap(),
            Command::Migrate
        ));
        assert!(matches!(
            parse(&argv(&["migrate:rollback", "--batches", "2"])).unwrap(),
            Command::MigrateRollback { batches: 2 }
        ));
        assert!(matches!(
            parse(&argv(&["migrate:status"])).unwrap(),
            Command::MigrateStatus
        ));
        assert!(matches!(
            parse(&argv(&["db:seed", "--seeder", "Admin"])).unwrap(),
            Command::Seed { seeder: Some(_) }
        ));
        assert!(matches!(
            parse(&argv(&["route:list"])).unwrap(),
            Command::RouteList
        ));
        assert!(matches!(parse(&argv(&["-V"])).unwrap(), Command::Version));
        assert!(matches!(
            parse(&argv(&["serve", "--help"])).unwrap(),
            Command::Help { command: Some(cmd) } if cmd == "serve"
        ));
        assert!(matches!(
            parse(&argv(&["help", "serve"])).unwrap(),
            Command::Help { command: Some(cmd) } if cmd == "serve"
        ));
        assert!(parse(&argv(&["help", "frobnicate"])).is_err());
    }

    #[test]
    fn rejects_syntax_errors_with_names() {
        for words in [
            vec!["frobnicate"],
            vec!["make"],
            vec!["make:widget", "X"],
            vec!["--port", "1"],
            vec!["serve", "--bogus"],
            vec!["serve", "-x"],
            vec!["serve", "--port"],
            vec!["serve", "--port", "0"],
            vec!["serve", "--port", "abc"],
            vec!["new"],
            vec!["new", "a", "b"],
            vec!["new", "a", "--", "x"],
            vec!["migrate", "extra"],
            vec!["migrate", "--force"],
            vec!["route:list", "--json"],
            vec!["db:seed", "--seeder"],
            vec!["migrate:rollback", "--batches", "0"],
            vec!["help", "extra", "args"],
        ] {
            assert!(parse(&argv(&words)).is_err(), "{words:?}");
        }
    }

    #[test]
    fn help_flag_wins_over_other_errors() {
        // `--help` short-circuits: even beside unknown flags it shows help.
        assert!(matches!(
            parse(&argv(&["serve", "--bogus", "--help"])).unwrap(),
            Command::Help { .. }
        ));
        // ...but unknown commands stay unknown.
        assert!(parse(&argv(&["frobnicate", "--help"])).is_err());
    }
}
