//! File generators: `make:*` renderers, the `new` template, and writing.
//!
//! Rendering is pure. Every `render_*` returns paths plus content, so
//! tests assert on strings without touching the filesystem.
//! [`write_tree`] performs the IO: it creates parent directories, refuses
//! to overwrite without `force`, and reports written paths. Generators
//! print follow-up hints (the `mod` line, registration calls) instead of
//! editing user files: scaffolds never rewrite code they didn't create.
//!
//! # Examples
//!
//! ```rust
//! use lumos_cli::render_controller;
//!
//! let generated = render_controller("User").unwrap();
//! assert_eq!(generated.files.len(), 1);
//! assert!(generated.files[0].content.contains("struct User"));
//! ```

use std::path::{Path, PathBuf};

use crate::args::MakeKind;
use crate::error::CliError;

/// One file to write: relative path plus full content.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_controller;
///
/// let generated = render_controller("User").unwrap();
/// assert_eq!(generated.files[0].path, std::path::PathBuf::from("src/controllers/user.rs"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the project root, e.g. `src/models/user.rs`.
    pub path: PathBuf,
    /// Complete file content.
    pub content: String,
}

/// Generator output: files plus follow-up hints printed after writing.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_model;
///
/// let generated = render_model("User").unwrap();
/// assert!(!generated.notes.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// Files to write (in order).
    pub files: Vec<GeneratedFile>,
    /// Hints printed after writing (`mod` lines, registration calls).
    pub notes: Vec<String>,
}

/// Writes files under `root`, creating parent directories.
///
/// Refuses existing files without `force` (naming the first collision);
/// returns the written absolute paths. Parent directories are created as
/// needed; IO failures name the path.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_cli::{render_controller, write_tree};
///
/// let generated = render_controller("User").unwrap();
/// let written = write_tree("/tmp/demo".as_ref(), &generated.files, false).unwrap();
/// assert_eq!(written.len(), 1);
/// ```
pub fn write_tree(
    root: &Path,
    files: &[GeneratedFile],
    force: bool,
) -> Result<Vec<PathBuf>, CliError> {
    if !force {
        for file in files {
            let target = root.join(&file.path);
            if target.exists() {
                return Err(CliError::new(format!(
                    "`{}` already exists: re-run with --force to overwrite.",
                    target.display()
                )));
            }
        }
    }
    let mut written = Vec::with_capacity(files.len());
    for file in files {
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CliError::new(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
        std::fs::write(&target, &file.content).map_err(|error| {
            CliError::new(format!("cannot write {}: {error}", target.display()))
        })?;
        written.push(target);
    }
    Ok(written)
}

/// Validates an UpperCamelCase type name (`User`, `UserController`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::validate_type_name;
///
/// assert!(validate_type_name("User").is_ok());
/// assert!(validate_type_name("user").is_err());
/// ```
pub fn validate_type_name(name: &str) -> Result<(), CliError> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(first) if first.is_ascii_uppercase())
        && chars.all(|char| char.is_ascii_alphanumeric() || char == '_');
    if name.is_empty() || !valid {
        return Err(CliError::new(format!(
            "invalid name `{name}`: expected UpperCamelCase (letters, digits, `_`, starting uppercase)."
        )));
    }
    Ok(())
}

/// Validates a cargo package name (`blog`, `my-app`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::validate_crate_name;
///
/// assert!(validate_crate_name("blog").is_ok());
/// assert!(validate_crate_name("Blog").is_err());
/// ```
pub fn validate_crate_name(name: &str) -> Result<(), CliError> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|char| {
            char.is_ascii_lowercase() || char.is_ascii_digit() || char == '-' || char == '_'
        });
    if name.is_empty() || !valid {
        return Err(CliError::new(format!(
            "invalid project name `{name}`: expected lowercase letters, digits, `-`, `_`."
        )));
    }
    Ok(())
}

/// Converts `UserController` to `user_controller`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::to_snake;
///
/// assert_eq!(to_snake("UserController"), "user_controller");
/// assert_eq!(to_snake("APIKey"), "api_key");
/// ```
pub fn to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let chars: Vec<char> = name.chars().collect();
    for (index, char) in chars.iter().enumerate() {
        if char.is_ascii_uppercase() {
            let prev_lower = index > 0 && chars[index - 1].is_ascii_lowercase();
            let next_lower = index + 1 < chars.len() && chars[index + 1].is_ascii_lowercase();
            let prev_is_separator = index > 0 && chars[index - 1] == '_';
            if index > 0 && (prev_lower || next_lower) && !prev_is_separator {
                out.push('_');
            }
            out.push(char.to_ascii_lowercase());
        } else {
            out.push(*char);
        }
    }
    out
}

/// Converts `create_users_table` (or `CreateUsers`) to `CreateUsersTable`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::to_camel;
///
/// assert_eq!(to_camel("create_users"), "CreateUsers");
/// assert_eq!(to_camel("User"), "User");
/// ```
pub fn to_camel(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Naive plural table name: snake-case plus `s` (`User` → `users`).
/// Irregular plurals need a manual edit (said so in the `mod` note).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::to_table;
///
/// assert_eq!(to_table("User"), "users");
/// ```
pub fn to_table(name: &str) -> String {
    format!("{}s", to_snake(name))
}

/// Current UTC timestamp for migration filenames (`YYYYMMDDHHMMSS`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::migration_timestamp;
///
/// let stamp = migration_timestamp();
/// assert_eq!(stamp.len(), 14);
/// assert!(stamp.chars().all(|char| char.is_ascii_digit()));
/// ```
pub fn migration_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S").to_string()
}

/// Renders `make:*` output for any kind (dispatches on `kind`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::{render_make, MakeKind};
///
/// let generated = render_make(MakeKind::Model, "User", "20240101000000").unwrap();
/// assert!(generated.files[0].content.contains("table = \"users\""));
/// ```
pub fn render_make(kind: MakeKind, name: &str, stamp: &str) -> Result<Generated, CliError> {
    match kind {
        MakeKind::Controller => render_controller(name),
        MakeKind::Model => render_model(name),
        MakeKind::Migration => render_migration(name, stamp),
        MakeKind::Seeder => render_seeder(name),
        MakeKind::Resource => render_resource(name),
        MakeKind::Middleware => render_middleware(name),
    }
}

/// Renders `make:controller <Name>`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_controller;
///
/// let generated = render_controller("User").unwrap();
/// assert_eq!(generated.files[0].path, std::path::PathBuf::from("src/controllers/user.rs"));
/// ```
pub fn render_controller(name: &str) -> Result<Generated, CliError> {
    validate_type_name(name)?;
    let snake = to_snake(name);
    let content = format!(
        "use lumos::{{controller, ok, Response, Result}};\n\
         \n\
         #[controller]\n\
         pub struct {name} {{}}\n\
         \n\
         #[controller]\n\
         impl {name} {{\n\
         \x20   pub async fn index(&self) -> Result<Response> {{\n\
         \x20       Ok(ok(&[] as &[&str]))\n\
         \x20   }}\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("src/controllers/{snake}.rs")),
            content,
        }],
        notes: vec![format!("add `pub mod {snake};` to your controllers module")],
    })
}

/// Renders `make:model <Name>`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_model;
///
/// let generated = render_model("User").unwrap();
/// assert!(generated.files[0].content.contains("table = \"users\""));
/// ```
pub fn render_model(name: &str) -> Result<Generated, CliError> {
    validate_type_name(name)?;
    let snake = to_snake(name);
    let table = to_table(name);
    let content = format!(
        "use rusticate::Model;\n\
         \n\
         #[derive(Model)]\n\
         #[model(table = \"{table}\", timestamps = true)]\n\
         pub struct {name} {{\n\
         \x20   #[model(id, auto_increment)]\n\
         \x20   pub id: i64,\n\
         \x20   pub name: String,\n\
         \x20   #[model(created_at)]\n\
         \x20   pub created_at: chrono::DateTime<chrono::Utc>,\n\
         \x20   #[model(updated_at)]\n\
         \x20   pub updated_at: chrono::DateTime<chrono::Utc>,\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("src/models/{snake}.rs")),
            content,
        }],
        notes: vec![
            format!("add `pub mod {snake};` to your models module"),
            "table name is a naive plural — edit #[model(table)] for irregular nouns".to_string(),
            "needs `rusticate` and `chrono` in [dependencies]".to_string(),
        ],
    })
}

/// Renders `make:migration <Name>` (accepts `snake_case` or `CamelCase`).
///
/// The struct name is the CamelCase form; `stamp` prefixes the filename
/// (pass [`migration_timestamp`]).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_migration;
///
/// let generated = render_migration("create_users", "20240101000000").unwrap();
/// assert_eq!(
///     generated.files[0].path,
///     std::path::PathBuf::from("database/migrations/20240101000000_create_users.rs")
/// );
/// assert!(generated.files[0].content.contains("struct CreateUsers"));
/// ```
pub fn render_migration(name: &str, stamp: &str) -> Result<Generated, CliError> {
    if name.is_empty()
        || !name
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '_')
    {
        return Err(CliError::new(format!(
            "invalid migration name `{name}`: expected letters, digits, `_`."
        )));
    }
    let struct_name = to_camel(name);
    if struct_name.is_empty() {
        return Err(CliError::new(format!("invalid migration name `{name}`.")));
    }
    let snake = to_snake(&struct_name);
    let content = format!(
        "//! Migration: {name}.\n\
         use rusticate::{{async_trait, Migration, Result, Schema}};\n\
         \n\
         pub struct {struct_name};\n\
         \n\
         #[async_trait]\n\
         impl Migration for {struct_name} {{\n\
         \x20   async fn up(&self, _schema: &mut Schema) -> Result<()> {{\n\
         \x20       // Define the change, then register it in your migrations list.\n\
         \x20       todo!(\"define {name} up\")\n\
         \x20   }}\n\
         \n\
         \x20   async fn down(&self, _schema: &mut Schema) -> Result<()> {{\n\
         \x20       todo!(\"define {name} down\")\n\
         \x20   }}\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("database/migrations/{stamp}_{snake}.rs")),
            content,
        }],
        notes: vec![
            "fill in up()/down(), then add it to your migrations list (see src/bin/cli.rs)"
                .to_string(),
        ],
    })
}

/// Renders `make:seeder <Name>`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_seeder;
///
/// let generated = render_seeder("AdminSeeder").unwrap();
/// assert_eq!(generated.files[0].path, std::path::PathBuf::from("database/seeders/admin_seeder.rs"));
/// ```
pub fn render_seeder(name: &str) -> Result<Generated, CliError> {
    validate_type_name(name)?;
    let snake = to_snake(name);
    let content = format!(
        "//! Seeder: {name}.\n\
         use rusticate::{{async_trait, Result, Seeder, DB}};\n\
         \n\
         pub struct {name};\n\
         \n\
         #[async_trait]\n\
         impl Seeder for {name} {{\n\
         \x20   async fn run(&self, _db: &DB) -> Result<()> {{\n\
         \x20       // Insert seed rows (Model::create + Changeset), then register\n\
         \x20       // this seeder in your seeders list.\n\
         \x20       todo!(\"define {snake} rows\")\n\
         \x20   }}\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("database/seeders/{snake}.rs")),
            content,
        }],
        notes: vec![
            "fill in run(), then add it to your seeders list (see src/bin/cli.rs)".to_string(),
        ],
    })
}

/// Renders `make:resource <Name>`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_resource;
///
/// let generated = render_resource("User").unwrap();
/// assert!(generated.files[0].content.contains("type = \"users\""));
/// ```
pub fn render_resource(name: &str) -> Result<Generated, CliError> {
    validate_type_name(name)?;
    let snake = to_snake(name);
    let resource_type = to_table(name);
    let content = format!(
        "//! JSON:API resource: {name}.\n\
         use lumos::JsonApiResource;\n\
         \n\
         #[derive(JsonApiResource)]\n\
         #[resource(type = \"{resource_type}\")]\n\
         pub struct {name} {{\n\
         \x20   #[resource(id)]\n\
         \x20   pub id: i64,\n\
         \x20   pub name: String,\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("src/resources/{snake}.rs")),
            content,
        }],
        notes: vec![
            format!("add `pub mod {snake};` to your resources module"),
            "needs the `jsonapi` or `jsonapi-lite` feature on lumos".to_string(),
        ],
    })
}

/// Renders `make:middleware <Name>`.
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_middleware;
///
/// let generated = render_middleware("RequestId").unwrap();
/// assert_eq!(generated.files[0].path, std::path::PathBuf::from("src/middleware/request_id.rs"));
/// ```
pub fn render_middleware(name: &str) -> Result<Generated, CliError> {
    validate_type_name(name)?;
    let snake = to_snake(name);
    let content = format!(
        "//! Middleware: {snake}.\n\
         use lumos::{{Next, Request, Response}};\n\
         \n\
         pub async fn {snake}(request: Request, next: Next) -> Response {{\n\
         \x20   next.run(request).await\n\
         }}\n"
    );
    Ok(Generated {
        files: vec![GeneratedFile {
            path: PathBuf::from(format!("src/middleware/{snake}.rs")),
            content,
        }],
        notes: vec![
            format!("add `pub mod {snake};` to your middleware module, then wrap routes with `from_fn({snake})`"),
        ],
    })
}

/// Renders the `new` project template: lib + server + CLI bins, one
/// controller, config, README, gitignore.
///
/// `lumos_version` is the `lumos` dependency version for the generated
/// `Cargo.toml` (the binary passes its own `CARGO_PKG_VERSION`).
///
/// # Examples
///
/// ```rust
/// use lumos_cli::render_new;
///
/// let generated = render_new("blog", "0.1").unwrap();
/// let paths: Vec<&str> = generated.files.iter().map(|file| file.path.to_str().unwrap()).collect();
/// assert!(paths.contains(&"Cargo.toml"));
/// assert!(paths.contains(&"src/main.rs"));
/// assert!(paths.contains(&"src/bin/cli.rs"));
/// ```
pub fn render_new(name: &str, lumos_version: &str) -> Result<Generated, CliError> {
    validate_crate_name(name)?;
    let lib_name = name.replace('-', "_");
    let title = to_camel(&lib_name);
    let cargo = format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [dependencies]\n\
         lumos = {{ package = \"lumos-rs\", version = \"{lumos_version}\" }}\n\
         lumos-cli = {{ package = \"lumos-rs-cli\", version = \"{lumos_version}\" }}\n\
         tokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n\
         \n\
         [[bin]]\n\
         name = \"cli\"\n\
         path = \"src/bin/cli.rs\"\n"
    );
    let lib = format!(
        "//! {title} application: controllers, routes, registry.\n\
         pub mod controllers;\n\
         \n\
         use controllers::users::UsersController;\n\
         use lumos::{{routes, Container, RouteRegistry, Router}};\n\
         use std::sync::Arc;\n\
         \n\
         /// Builds the router and its registry side by side: every mount has\n\
         /// a matching registry call, so `route:list` stays truthful.\n\
         pub fn build() -> lumos::Result<(Router, RouteRegistry)> {{\n\
         \x20   let container = Container::new();\n\
         \x20   let users = Arc::new(UsersController::from_container(&container)?);\n\
         \x20   let router: Router = routes! {{\n\
         \x20       resource(\"/users\", UsersController, users),\n\
         \x20       route(\"/health\", lumos::get(health)),\n\
         \x20   }};\n\
         \x20   let mut registry = RouteRegistry::new();\n\
         \x20   registry.resource(\"/users\", UsersController::route_entries());\n\
         \x20   registry.route(\"GET\", \"/health\", \"health\");\n\
         \x20   Ok((router, registry))\n\
         }}\n\
         \n\
         async fn health() -> &'static str {{\n\
         \x20   \"ok\"\n\
         }}\n"
    );
    let controllers = "pub mod users;\n".to_string();
    let users = "use lumos::{controller, ok, Path, Response, Result};\n\
         \n\
         #[controller]\n\
         pub struct UsersController {}\n\
         \n\
         #[controller]\n\
         impl UsersController {\n\
         \x20   pub async fn index(&self) -> Result<Response> {\n\
         \x20       Ok(ok(&[\"user-1\"]))\n\
         \x20   }\n\
         \n\
         \x20   pub async fn show(&self, Path(id): Path<i64>) -> Result<Response> {\n\
         \x20       Ok(ok(&format!(\"user {id}\")))\n\
         \x20   }\n\
         }\n"
    .to_string();
    let main = format!(
        "//! {title} server: `cargo run` here, or `lumos serve`.\n\
         use {lib_name}::build;\n\
         \n\
         #[tokio::main]\n\
         async fn main() -> lumos::Result<()> {{\n\
         \x20   lumos::Log::init()?;\n\
         \x20   let config = lumos::Config::load()?;\n\
         \x20   let name: String = config.get(\"app.name\")?;\n\
         \x20   lumos::Log::info(&format!(\"starting {{name}}\"));\n\
         \x20   let host = std::env::var(\"HOST\").unwrap_or_else(|_| \"127.0.0.1\".to_string());\n\
         \x20   let port = std::env::var(\"PORT\").unwrap_or_else(|_| \"3000\".to_string());\n\
         \x20   let (router, _registry) = build()?;\n\
         \x20   lumos::serve(router, &format!(\"{{host}}:{{port}}\")).await\n\
         }}\n"
    );
    let cli = format!(
        "//! {title} CLI: `cargo run --bin cli -- <migrate|db:seed|route:list>`.\n\
         use {lib_name}::build;\n\
         \n\
         #[tokio::main]\n\
         async fn main() {{\n\
         \x20   let argv: Vec<String> = std::env::args().skip(1).collect();\n\
         \x20   let (_router, registry) = match build() {{\n\
         \x20       Ok(built) => built,\n\
         \x20       Err(error) => {{\n\
         \x20           eprintln!(\"failed to build routes: {{error}}\");\n\
         \x20           std::process::exit(1);\n\
         \x20       }}\n\
         \x20   }};\n\
         \x20   let context = lumos_cli::AppContext::new().registry(registry);\n\
         \x20   // With rusticate: connect a DB and register work here:\n\
         \x20   // let context = context.db(db).migration(CreateUsers).seeder(AdminSeeder);\n\
         \x20   std::process::exit(lumos_cli::run_app(&argv, &context).await);\n\
         }}\n"
    );
    // First lookup segment names the file: `get("app.name")` reads key
    // `name` from `app.toml`, so keys stay top-level here.
    let config = format!(
        "# {title} configuration (see lumos Config).\n\
         name = \"{name}\"\n\
         env = \"local\"\n"
    );
    let readme = format!(
        "# {title}\n\
         \n\
         Built with Lumos.\n\
         \n\
         ```sh\n\
         cargo run                      # serve on 127.0.0.1:3000\n\
         lumos serve --port 8080        # same via the CLI\n\
         cargo run --bin cli -- route:list\n\
         lumos make:controller Widget   # generators\n\
         ```\n\
         \n\
         Config lives in `config/` + `.env` (see `.env.example`). Routes and\n\
         their registry entries are built together in `src/lib.rs`.\n"
    );
    let gitignore = "/target\n.env\n*.sqlite*\n".to_string();
    let env_example = "# Copy to .env and adjust.\n# DATABASE_URL=sqlite://db.sqlite\n".to_string();
    let files = vec![
        ("Cargo.toml", cargo),
        ("src/lib.rs", lib),
        ("src/controllers.rs", controllers),
        ("src/controllers/users.rs", users),
        ("src/main.rs", main),
        ("src/bin/cli.rs", cli),
        ("config/app.toml", config),
        (".env.example", env_example),
        ("README.md", readme),
        (".gitignore", gitignore),
    ];
    Ok(Generated {
        files: files
            .into_iter()
            .map(|(path, content)| GeneratedFile {
                path: PathBuf::from(path),
                content,
            })
            .collect(),
        notes: vec![
            format!("created {name}/ — run `cd {name} && cargo run`"),
            "add a database later: rusticate + DATABASE_URL, then uncomment the cli.rs lines"
                .to_string(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_make_kind_renders_path_content_and_note() {
        let kinds = [
            (
                MakeKind::Controller,
                "src/controllers/user.rs",
                "struct User",
            ),
            (MakeKind::Model, "src/models/user.rs", "table = \"users\""),
            (
                MakeKind::Migration,
                "20240101000000_create_users.rs",
                "struct CreateUsers",
            ),
            (
                MakeKind::Seeder,
                "database/seeders/admin_seeder.rs",
                "struct AdminSeeder",
            ),
            (
                MakeKind::Resource,
                "src/resources/user.rs",
                "type = \"users\"",
            ),
            (
                MakeKind::Middleware,
                "src/middleware/request_id.rs",
                "fn request_id",
            ),
        ];
        for (kind, path_suffix, content) in kinds {
            let name = match kind {
                MakeKind::Migration => "create_users",
                MakeKind::Seeder => "AdminSeeder",
                MakeKind::Middleware => "RequestId",
                _ => "User",
            };
            let generated = render_make(kind, name, "20240101000000").unwrap();
            assert_eq!(generated.files.len(), 1, "{kind:?}");
            assert!(
                generated.files[0]
                    .path
                    .to_str()
                    .unwrap()
                    .ends_with(path_suffix),
                "{kind:?}: {}",
                generated.files[0].path.display()
            );
            assert!(generated.files[0].content.contains(content), "{kind:?}");
            assert!(!generated.notes.is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn migration_names_accept_snake_and_camel() {
        let snake = render_migration("create_users", "S").unwrap();
        assert!(snake.files[0].content.contains("struct CreateUsers"));
        let camel = render_migration("CreateUsers", "S").unwrap();
        assert!(camel.files[0].content.contains("struct CreateUsers"));
        assert!(render_migration("has space", "S").is_err());
        assert!(render_migration("", "S").is_err());
        assert!(render_make(MakeKind::Model, "user", "S").is_err());
    }

    #[test]
    fn new_template_names_the_crate_everywhere() {
        let generated = render_new("my-app", "0.1").unwrap();
        assert_eq!(generated.files.len(), 10);
        let cargo = generated
            .files
            .iter()
            .find(|file| file.path.to_str() == Some("Cargo.toml"))
            .unwrap();
        assert!(cargo.content.contains("name = \"my-app\""));
        assert!(cargo
            .content
            .contains("lumos = { package = \"lumos-rs\", version = \"0.1\" }"));
        assert!(cargo
            .content
            .contains("lumos-cli = { package = \"lumos-rs-cli\", version = \"0.1\" }"));
        let main = generated
            .files
            .iter()
            .find(|file| file.path.to_str() == Some("src/main.rs"))
            .unwrap();
        assert!(main.content.contains("use my_app::build;"));
        assert!(render_new("Blog", "0.1").is_err());
        assert!(render_new("", "0.1").is_err());
    }

    #[test]
    fn title_cases_camel_parts() {
        assert_eq!(to_camel("create_users_table"), "CreateUsersTable");
        assert_eq!(to_camel("User"), "User");
        assert_eq!(to_snake("APIKey"), "api_key");
        assert_eq!(to_snake("User"), "user");
    }
}
