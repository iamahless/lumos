//! The `orm` feature path: facade re-exports plus `rusticate::Error` →
//! `AppError` mapping (`?` converts directly in actions).

#![cfg(feature = "orm")]

use lumos::rusticate::{Changeset, Model, Schema, DB};
use lumos::{AppError, StatusCode};

#[derive(Model)]
#[model(table = "users")]
pub struct User {
    #[model(id, auto_increment)]
    pub id: i64,
    pub name: String,
}

async fn show(db: &DB, id: i64) -> lumos::Result<String> {
    let user = User::find_or_fail(db, id).await?;
    Ok(user.name)
}

#[tokio::test]
async fn orm_errors_map_to_http_statuses() {
    let db = DB::memory().await.unwrap();
    Schema::new(&db)
        .create("users", |t| {
            t.id();
            t.string("name");
        })
        .await
        .unwrap();

    let missing = show(&db, 1).await.unwrap_err();
    assert_eq!(missing.status_code(), StatusCode::NOT_FOUND);
    assert_eq!(missing.code(), "not_found");

    let conflict = AppError::from(lumos::rusticate::Error::UniqueViolation(
        "email".to_string(),
    ));
    assert_eq!(conflict.status_code(), StatusCode::CONFLICT);

    let internal = AppError::from(lumos::rusticate::Error::invalid_query("bad"));
    assert_eq!(internal.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(internal.detail(), "An unexpected error occurred.");

    let user = User::create(&db, Changeset::new().set("name", "Ada"))
        .await
        .unwrap();
    assert_eq!(show(&db, user.id).await.unwrap(), "Ada");
}
