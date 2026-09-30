//! Blog domain models: users write posts, posts collect comments.
//!
//! Relations stay lazy by default; controllers eager-load with `.with()`
//! only when the request asks (`?include=author`).

use lumos::rusticate::{BelongsTo, HasMany, Model};

/// A blog author. `password_hash` is hidden from serialization and from
/// the JSON:API resource alike.
#[derive(Debug, Model)]
#[model(table = "users", timestamps = true)]
pub struct User {
    #[model(id, auto_increment)]
    pub id: i64,
    pub name: String,
    pub email: String,
    #[model(hidden)]
    pub password_hash: String,
    #[model(has_many = "Post")]
    pub posts: HasMany<Post>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A blog post, owned by one author.
#[derive(Debug, Model)]
#[model(table = "posts", timestamps = true)]
pub struct Post {
    #[model(id, auto_increment)]
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub body: String,
    #[model(belongs_to = "User", foreign_key = "user_id")]
    pub author: BelongsTo<User>,
    #[model(has_many = "Comment")]
    pub comments: HasMany<Comment>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A comment on a post, written by a user.
#[derive(Debug, Model)]
#[model(table = "comments", timestamps = true)]
pub struct Comment {
    #[model(id, auto_increment)]
    pub id: i64,
    pub post_id: i64,
    pub user_id: i64,
    pub body: String,
    #[model(belongs_to = "Post", foreign_key = "post_id")]
    pub post: BelongsTo<Post>,
    #[model(belongs_to = "User", foreign_key = "user_id")]
    pub author: BelongsTo<User>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
