//! Posts: full JSON:API CRUD. Reads are public; writes require a
//! session, and edits require ownership (403 otherwise).

use lumos::rusticate::{Changeset, Model, DB};
use lumos::{controller, ApiQuery, ApiResult, AppError, CurrentUser, JsonApiBody, Path, Response};

use crate::into_api;
use crate::models::Post;
use crate::resources::{render_posts, ApiPost, PostInput};

/// Posts controller: the database arrives via `from_container`.
#[controller]
pub struct PostsController {
    db: DB,
}

#[controller]
impl PostsController {
    /// Lists posts: `filter[user_id|title]`, `sort[-id|user_id|title]`,
    /// `page[number|size]`, `include=author`, `fields[posts]`.
    pub async fn index(&self, query: ApiQuery) -> ApiResult<Response> {
        let mut sql = Post::query(&self.db);
        for filter in &query.filters {
            match filter.field.as_str() {
                "user_id" => {
                    let id = filter.value.parse::<i64>().map_err(|_| {
                        AppError::bad_request(format!(
                            "invalid filter[user_id]: {:?}",
                            filter.value
                        ))
                    })?;
                    sql = sql.where_eq("user_id", id);
                }
                "title" => sql = sql.where_eq("title", filter.value.clone()),
                // Unknown filters fall through to `collection`, which 400s
                // on fields outside the resource's attributes.
                _ => {}
            }
        }
        for sort in &query.sort {
            match sort.field.as_str() {
                "id" | "user_id" | "title" | "created_at" => {
                    sql = if sort.descending {
                        sql.order_by_desc(&sort.field)
                    } else {
                        sql.order_by(&sort.field)
                    };
                }
                _ => {}
            }
        }
        let page = into_api(sql.paginate(query.page.size, query.page.number).await)?;
        let items = render_posts(&self.db, page.items, wants_author(&query)).await?;
        lumos::collection(&items, page.total, &query)
    }

    /// Shows one post (404 when missing).
    pub async fn show(&self, Path(id): Path<i64>, query: ApiQuery) -> ApiResult<Response> {
        let post = into_api(Post::find_or_fail(&self.db, id).await)?;
        let items = render_posts(&self.db, vec![post], wants_author(&query)).await?;
        lumos::single(&items[0], &query)
    }

    /// Creates a post as the current user (201 + Location).
    pub async fn store(
        &self,
        user: CurrentUser,
        body: JsonApiBody<PostInput>,
    ) -> ApiResult<Response> {
        lumos::validate(&body.resource)?;
        let post = into_api(
            Post::create(
                &self.db,
                Changeset::new()
                    .set("user_id", author_id(&user)?)
                    .set("title", body.resource.title.clone())
                    .set("body", body.resource.body.clone()),
            )
            .await,
        )?;
        let query = ApiQuery::parse(&[], "/posts")?;
        lumos::lumos_jsonapi::created(
            &ApiPost::just_link(&post),
            &format!("/posts/{}", post.id),
            &query,
        )
    }

    /// Replaces a post: only its author may (403 otherwise), and a body
    /// id that disagrees with the URL is a 409.
    pub async fn update(
        &self,
        user: CurrentUser,
        Path(id): Path<i64>,
        body: JsonApiBody<PostInput>,
    ) -> ApiResult<Response> {
        lumos::validate(&body.resource)?;
        if body
            .id
            .as_deref()
            .is_some_and(|given| given != id.to_string())
        {
            return Err(AppError::conflict(format!(
                "body id {:?} does not match URL id {id}",
                body.id
            ))
            .into());
        }
        let mut post = into_api(Post::find_or_fail(&self.db, id).await)?;
        require_owner(&user, &post)?;
        into_api(
            post.update(
                &self.db,
                Changeset::new()
                    .set("title", body.resource.title.clone())
                    .set("body", body.resource.body.clone()),
            )
            .await,
        )?;
        into_api(post.refresh(&self.db).await)?;
        let query = ApiQuery::parse(&[], format!("/posts/{id}"))?;
        lumos::single(&ApiPost::just_link(&post), &query)
    }

    /// Deletes a post: only its author may (403 otherwise).
    pub async fn destroy(&self, user: CurrentUser, Path(id): Path<i64>) -> ApiResult<Response> {
        let mut post = into_api(Post::find_or_fail(&self.db, id).await)?;
        require_owner(&user, &post)?;
        into_api(post.delete(&self.db).await)?;
        Ok(lumos::no_content())
    }
}

/// True when `?include=` names the author (nested paths count: the
/// first segment decides what we eager-load).
fn wants_author(query: &ApiQuery) -> bool {
    query
        .include
        .iter()
        .any(|path| path.first().is_some_and(|segment| segment == "author"))
}

/// The session's user id as a row id. Sessions are minted from model
/// ids at login, so a non-numeric value is server corruption (500).
fn author_id(user: &CurrentUser) -> lumos::Result<i64> {
    user.0
        .user_id
        .parse::<i64>()
        .map_err(|_| AppError::internal(format!("corrupt session user id: {:?}", user.0.user_id)))
}

/// Rejects edits by anyone but the post's author (403).
fn require_owner(user: &CurrentUser, post: &Post) -> lumos::Result<()> {
    if post.user_id == author_id(user)? {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
