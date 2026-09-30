//! Comments: listed (optionally per post), shown, created, deleted.
//!
//! There is deliberately no `update`: comments are immutable, so PATCH
//! has no route (axum answers 405) and `route:list` shows no PATCH row.
//! Only present convention methods become routes.

use lumos::rusticate::{Changeset, Model, DB};
use lumos::{controller, ApiQuery, ApiResult, AppError, CurrentUser, JsonApiBody, Path, Response};

use crate::into_api;
use crate::models::{Comment, Post};
use crate::resources::{ApiComment, CommentInput};

/// Comments controller: the database arrives via `from_container`.
#[controller]
pub struct CommentsController {
    db: DB,
}

#[controller]
impl CommentsController {
    /// Lists comments: `filter[post_id|user_id]`, paging, sorting.
    pub async fn index(&self, query: ApiQuery) -> ApiResult<Response> {
        let mut sql = Comment::query(&self.db);
        for filter in &query.filters {
            match filter.field.as_str() {
                "post_id" | "user_id" => {
                    let id = filter.value.parse::<i64>().map_err(|_| {
                        AppError::bad_request(format!(
                            "invalid filter[{}]: {:?}",
                            filter.field, filter.value
                        ))
                    })?;
                    sql = sql.where_eq(&filter.field, id);
                }
                _ => {}
            }
        }
        for sort in &query.sort {
            match sort.field.as_str() {
                "id" | "post_id" | "created_at" => {
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
        let items: Vec<ApiComment> = page.items.iter().map(ApiComment::from).collect();
        lumos::collection(&items, page.total, &query)
    }

    /// Shows one comment (404 when missing).
    pub async fn show(&self, Path(id): Path<i64>, query: ApiQuery) -> ApiResult<Response> {
        let comment = into_api(Comment::find_or_fail(&self.db, id).await)?;
        lumos::single(&ApiComment::from(&comment), &query)
    }

    /// Creates a comment as the current user (201 + Location). The post
    /// must exist (404 otherwise).
    pub async fn store(
        &self,
        user: CurrentUser,
        body: JsonApiBody<CommentInput>,
    ) -> ApiResult<Response> {
        lumos::validate(&body.resource)?;
        into_api(Post::find_or_fail(&self.db, body.resource.post_id).await)?;
        let author: i64 = user.0.user_id.parse::<i64>().map_err(|_| {
            AppError::internal(format!("corrupt session user id: {:?}", user.0.user_id))
        })?;
        let comment = into_api(
            Comment::create(
                &self.db,
                Changeset::new()
                    .set("post_id", body.resource.post_id)
                    .set("user_id", author)
                    .set("body", body.resource.body.clone()),
            )
            .await,
        )?;
        let query = ApiQuery::parse(&[], "/comments")?;
        lumos::lumos_jsonapi::created(
            &ApiComment::from(&comment),
            &format!("/comments/{}", comment.id),
            &query,
        )
    }

    /// Deletes a comment: only its author may (403 otherwise).
    pub async fn destroy(&self, user: CurrentUser, Path(id): Path<i64>) -> ApiResult<Response> {
        let mut comment = into_api(Comment::find_or_fail(&self.db, id).await)?;
        let author: i64 = user.0.user_id.parse::<i64>().map_err(|_| {
            AppError::internal(format!("corrupt session user id: {:?}", user.0.user_id))
        })?;
        if comment.user_id != author {
            return Err(AppError::Forbidden.into());
        }
        into_api(comment.delete(&self.db).await)?;
        Ok(lumos::no_content())
    }
}
