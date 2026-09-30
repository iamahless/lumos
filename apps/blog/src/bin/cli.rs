//! Blog CLI: `cargo run -p blog --bin cli -- <migrate|db:seed|route:list>`.
//!
//! Reads `DATABASE_URL` (same default file as the server) so migrate,
//! seed, and serve always agree on the database.

use blog::database::migrations::{CreateComments, CreatePosts, CreateUsers};
use blog::database::seeders::DemoSeeder;
use lumos::rusticate::DB;

#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:blog.db?mode=rwc".to_string());
    let db = match DB::connect(&url).await {
        Ok(db) => db,
        Err(error) => {
            eprintln!("cannot connect to {url}: {error}");
            std::process::exit(1);
        }
    };
    let (_router, registry) = match blog::build(db.clone()) {
        Ok(built) => built,
        Err(error) => {
            eprintln!("failed to build routes: {error}");
            std::process::exit(1);
        }
    };
    let context = lumos_cli::AppContext::new()
        .db(db)
        .migration(CreateUsers)
        .migration(CreatePosts)
        .migration(CreateComments)
        .seeder(DemoSeeder)
        .registry(registry);
    std::process::exit(lumos_cli::run_app(&argv, &context).await);
}
