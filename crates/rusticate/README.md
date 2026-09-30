# lumos-rs-rusticate

Rusticate is an Eloquent-inspired ORM for Rust. It supports SQLite, PostgreSQL, and MySQL through sqlx's `Any` driver. The database target is explicit in every query, so the same query can run against a database connection or a transaction.

## Installation

```toml
[dependencies]
rusticate = { package = "lumos-rs-rusticate", version = "0.1" }
```

## A model and query

```rust,no_run
use rusticate::{Changeset, Model, Schema, DB};

#[derive(Model)]
#[model(table = "users")]
struct User {
    #[model(id, auto_increment)]
    id: i64,
    name: String,
}

#[tokio::main]
async fn main() -> rusticate::Result<()> {
    let db = DB::memory().await?;
    Schema::new(&db).create("users", |table| {
        table.id();
        table.string("name");
    }).await?;

    let user = User::create(&db, Changeset::new().set("name", "Ada")).await?;
    let found = User::find_or_fail(&db, user.id).await?;
    assert_eq!(found.name, "Ada");
    Ok(())
}
```

## Models and relations

`#[derive(Model)]` generates persistence methods, field conversion, and relation helpers. Model fields support primary-key, timestamp, soft-delete, cast, `has_many`, and `belongs_to` options. `HasMany<T>` and `BelongsTo<T>` provide relation queries, while `Query::with` loads relations for retrieved models.

`Model` supplies `query`, `find`, `find_or_fail`, `create`, `save`, `update`, and `delete`. `Changeset` collects values for create and update operations. `Page` represents paginated results.

## Queries and transactions

`Query` provides conditions, ordering, limits, offsets, aggregate methods, pagination, chunks, and relation loading. It binds values rather than interpolating them into SQL. `Target` accepts either `&DB` or `&Transaction`; use `Query::on` to run a built query inside a transaction.

`DB` opens connections from a URL or creates an in-memory SQLite database. `Schema`, `Migrator`, and `Migration` manage schema changes. `RawQuery` is available for SQL that does not fit the query builder.

## Scope macros and fixtures

Use `#[scopes]` to turn public `scope_*` associated functions into chainable `Query` methods. `factory!` creates deterministic fixture builders, and `Seeder` defines explicit seed operations.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
