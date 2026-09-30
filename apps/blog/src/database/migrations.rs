//! Schema migrations: users, posts, comments. Every `up` has a `down`
//! so `migrate:rollback` restores the previous schema.

use lumos::rusticate::{async_trait, Migration, Result, Schema};

/// Authors table: credentials plus profile name.
pub struct CreateUsers;

#[async_trait]
impl Migration for CreateUsers {
    async fn up(&self, schema: &mut Schema) -> Result<()> {
        schema
            .create("users", |t| {
                t.id();
                t.string("name");
                t.string("email").unique();
                t.string("password_hash");
                t.timestamps();
            })
            .await
    }

    async fn down(&self, schema: &mut Schema) -> Result<()> {
        schema.drop("users").await
    }
}

/// Posts table: one author each.
pub struct CreatePosts;

#[async_trait]
impl Migration for CreatePosts {
    async fn up(&self, schema: &mut Schema) -> Result<()> {
        schema
            .create("posts", |t| {
                t.id();
                t.foreign_id("user_id").references("id").on("users");
                t.string("title");
                t.text("body");
                t.timestamps();
            })
            .await
    }

    async fn down(&self, schema: &mut Schema) -> Result<()> {
        schema.drop("posts").await
    }
}

/// Comments table: one post and one author each.
pub struct CreateComments;

#[async_trait]
impl Migration for CreateComments {
    async fn up(&self, schema: &mut Schema) -> Result<()> {
        schema
            .create("comments", |t| {
                t.id();
                t.foreign_id("post_id").references("id").on("posts");
                t.foreign_id("user_id").references("id").on("users");
                t.text("body");
                t.timestamps();
            })
            .await
    }

    async fn down(&self, schema: &mut Schema) -> Result<()> {
        schema.drop("comments").await
    }
}
