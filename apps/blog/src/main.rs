//! Blog server: `cargo run -p blog` here, or `lumos serve` from `apps/blog`.
//!
//! Pending migrations run on boot (dev convenience, logged loudly); use
//! the `cli` binary for explicit migrate/seed/route commands.

use blog::database::migrations::{CreateComments, CreatePosts, CreateUsers};
use lumos::rusticate::{Migrator, DB};

#[tokio::main]
async fn main() -> lumos::Result<()> {
    lumos::Log::init()?;
    let config = lumos::Config::load()?;
    let name: String = config.get("app.name")?;
    lumos::Log::info(&format!("starting {name}"));

    // `mode=rwc` creates the file on first run; drop it once deployed.
    let url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:blog.db?mode=rwc".to_string());
    let db = DB::connect(&url).await?;
    let applied = Migrator::new(&db)
        .run(&[&CreateUsers, &CreatePosts, &CreateComments])
        .await?;
    for migration in &applied {
        lumos::Log::info(&format!("migrated {migration}"));
    }

    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let (router, _registry) = blog::build(db)?;
    lumos::serve(router, &format!("{host}:{port}")).await
}
