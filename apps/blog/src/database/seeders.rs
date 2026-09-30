//! Demo seed data: one admin, one author, two posts, one comment.
//!
//! Seeding is idempotent-ish by convention, not by guard: run it once per
//! fresh database (`migrate` then `db:seed`), like any seed script.

use lumos::rusticate::{async_trait, Changeset, Model, Result, Seeder, DB};

use crate::models::{Comment, Post, User};

/// Seeds the demo content. Named for `db:seed --seeder DemoSeeder`.
pub struct DemoSeeder;

impl DemoSeeder {
    /// Admin credentials printed after seeding.
    pub const ADMIN_EMAIL: &'static str = "admin@example.com";
    /// Password for every seeded user (dev only, never in production).
    pub const PASSWORD: &'static str = "password";
}

#[async_trait]
impl Seeder for DemoSeeder {
    fn name(&self) -> String {
        "DemoSeeder".to_string()
    }

    async fn run(&self, db: &DB) -> Result<()> {
        let hash = lumos::hash_password(Self::PASSWORD)
            .map_err(|error| rusticate_error(format!("cannot hash seed password: {error}")))?;
        let admin = User::create(
            db,
            Changeset::new()
                .set("name", "Admin")
                .set("email", Self::ADMIN_EMAIL)
                .set("password_hash", hash.clone()),
        )
        .await?;
        let author = User::create(
            db,
            Changeset::new()
                .set("name", "Ada")
                .set("email", "ada@example.com")
                .set("password_hash", hash),
        )
        .await?;
        let post = Post::create(
            db,
            Changeset::new()
                .set("user_id", author.id)
                .set("title", "Hello, Lumos")
                .set("body", "The example blog is running."),
        )
        .await?;
        Post::create(
            db,
            Changeset::new()
                .set("user_id", admin.id)
                .set("title", "Second post")
                .set("body", "Seeded alongside the first."),
        )
        .await?;
        Comment::create(
            db,
            Changeset::new()
                .set("post_id", post.id)
                .set("user_id", admin.id)
                .set("body", "Welcome aboard!"),
        )
        .await?;
        Ok(())
    }
}

/// Wraps an operational failure as a rusticate error (seeders speak
/// rusticate's `Result`, so framework errors convert at the boundary).
fn rusticate_error(message: String) -> lumos::rusticate::Error {
    lumos::rusticate::Error::Config(message)
}
