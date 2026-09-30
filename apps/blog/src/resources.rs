//! JSON:API resources: what clients see. Models are never serialized
//! directly — these DTOs decide the wire shape (note the absent
//! `password_hash` on [`ApiUser`]).
//!
//! Write inputs live here too: small `Deserialize` structs with manual
//! `Resource` impls (the `TYPE` const drives `JsonApiBody`'s type check),
//! validated with [`lumos::validate`] for 422s.

use lumos::rusticate::Model;
use lumos::{JsonApiResource, NamedRelationship, Resource, ToOne, Validate};
use serde::Deserialize;
use std::collections::HashMap;

use crate::models::{Comment, Post, User};

/// Public author view: id, name, email. No password hash, ever.
#[derive(JsonApiResource)]
#[resource(type = "users")]
pub struct ApiUser {
    #[resource(id)]
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl From<&User> for ApiUser {
    fn from(user: &User) -> Self {
        Self {
            id: user.id,
            name: user.name.clone(),
            email: user.email.clone(),
        }
    }
}

/// Public post view, with author linkage (loaded on `?include=author`).
#[derive(JsonApiResource)]
#[resource(type = "posts")]
pub struct ApiPost {
    #[resource(id)]
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub body: String,
    #[resource(relation)]
    pub author: ToOne<ApiUser>,
}

impl ApiPost {
    /// Renders `post`, loading nothing: author stays linkage-only unless
    /// the caller passes a loaded user.
    pub fn just_link(post: &Post) -> Self {
        Self::with_author(post, None)
    }

    /// Renders `post`, embedding `author` when provided.
    pub fn with_author(post: &Post, author: Option<&User>) -> Self {
        Self {
            id: post.id,
            user_id: post.user_id,
            title: post.title.clone(),
            body: post.body.clone(),
            author: match author {
                Some(user) => ToOne::loaded(ApiUser::from(user)),
                None => ToOne::id(post.user_id.to_string()),
            },
        }
    }
}

/// Public comment view, with post linkage.
#[derive(JsonApiResource)]
#[resource(type = "comments")]
pub struct ApiComment {
    #[resource(id)]
    pub id: i64,
    pub post_id: i64,
    pub user_id: i64,
    pub body: String,
    #[resource(relation)]
    pub post: ToOne<ApiPostRef>,
}

/// Linkage-only post reference (avoids recursing post → comments → post).
#[derive(JsonApiResource)]
#[resource(type = "posts")]
pub struct ApiPostRef {
    #[resource(id)]
    pub id: i64,
}

impl From<&Comment> for ApiComment {
    fn from(comment: &Comment) -> Self {
        Self {
            id: comment.id,
            post_id: comment.post_id,
            user_id: comment.user_id,
            body: comment.body.clone(),
            post: ToOne::id(comment.post_id.to_string()),
        }
    }
}

/// `POST /posts` / `PATCH /posts/{id}` attributes.
#[derive(Debug, Deserialize, Validate)]
pub struct PostInput {
    #[validate(length(min = 1, max = 120))]
    pub title: String,
    #[validate(length(min = 1, max = 10_000))]
    pub body: String,
}

impl Resource for PostInput {
    const TYPE: &'static str = "posts";

    fn resource_id(&self) -> String {
        String::new()
    }

    fn attributes(&self) -> lumos::Result<lumos::AttributeMap> {
        Ok(lumos::AttributeMap::new())
    }

    fn relationships(&self) -> Vec<NamedRelationship<'_>> {
        Vec::new()
    }
}

/// `POST /comments` attributes (`post_id` rides as a plain attribute;
/// relationship-object input stays out of scope for the example).
#[derive(Debug, Deserialize, Validate)]
pub struct CommentInput {
    pub post_id: i64,
    #[validate(length(min = 1, max = 5_000))]
    pub body: String,
}

impl Resource for CommentInput {
    const TYPE: &'static str = "comments";

    fn resource_id(&self) -> String {
        String::new()
    }

    fn attributes(&self) -> lumos::Result<lumos::AttributeMap> {
        Ok(lumos::AttributeMap::new())
    }

    fn relationships(&self) -> Vec<NamedRelationship<'_>> {
        Vec::new()
    }
}

/// `POST /sessions` credentials (plain JSON, not JSON:API).
#[derive(Debug, Deserialize, Validate)]
pub struct LoginInput {
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 1))]
    pub password: String,
}

/// Converts a model query row into its resource, loading authors only
/// when `include_author` (driven by `?include=author`).
pub async fn render_posts(
    db: &lumos::rusticate::DB,
    posts: Vec<Post>,
    include_author: bool,
) -> lumos::Result<Vec<ApiPost>> {
    if !include_author {
        return Ok(posts.iter().map(ApiPost::just_link).collect());
    }
    let mut ids: Vec<i64> = posts.iter().map(|post| post.user_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let authors = User::query(db).where_in("id", ids).get().await?;
    let authors_by_id: HashMap<i64, &User> =
        authors.iter().map(|author| (author.id, author)).collect();
    Ok(posts
        .iter()
        .map(|post| {
            let author = authors_by_id.get(&post.user_id).copied();
            ApiPost::with_author(post, author)
        })
        .collect())
}
