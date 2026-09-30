# lumos-macros

`lumos-macros` contains the procedural macros used by Lumos and Rusticate. They generate routing, dependency-injection, ORM, JSON:API, and query-scope code at compile time.

Applications normally import these macros through the framework crates instead of depending on `lumos-macros` directly:

```rust
use lumos::{controller, JsonApiResource};
use rusticate::{scopes, Model};
```

The generated code refers to `::lumos` for `controller` and `JsonApiResource`, and to `::rusticate` for `Model` and `scopes`. Use the facade crates unless you are maintaining an unusual workspace layout.

## Macros

| Macro | Import | Purpose |
| --- | --- | --- |
| `#[controller]` | `lumos::controller` | Builds controller fields from the container and maps conventional actions to routes. |
| `#[derive(Model)]` | `rusticate::Model` | Implements Rusticate's `Model` trait and relation helpers for a struct. |
| `#[scopes]` | `rusticate::scopes` | Adds chainable query-scope methods from `scope_*` associated functions. |
| `#[derive(JsonApiResource)]` | `lumos::JsonApiResource` | Implements the JSON:API `Resource` trait from a resource struct. |

## Controllers

Apply `#[controller]` to a struct with named fields to generate `from_container`. Every field is resolved with `Container::resolve_value`, so each registered field type must implement `Clone`. Use `Arc<T>` for a service that is not cloneable.

```rust,ignore
use lumos::{controller, Container, Result};

#[controller]
pub struct UserController {
    users: UserService,
}

impl UserController {
    pub fn new(container: &Container) -> Result<Self> {
        Self::from_container(container)
    }
}
struct UserService;
```

Apply the macro to an inherent implementation block to generate `routes` and `route_entries`. Public conventional methods become routes when they take `&self`, are not generic, and return a type that implements `IntoResponse`. Handler arguments are forwarded as Axum extractors.

| Method | HTTP method | Relative path |
| --- | --- | --- |
| `index` | `GET` | `/` |
| `store` | `POST` | `/` |
| `show` | `GET` | `/{id}` |
| `update` | `PUT`, `PATCH` | `/{id}` |
| `destroy` | `DELETE` | `/{id}` |

Mount a controller under a prefix with `routes!`:

```rust,ignore
#[controller]
impl UserController {
    pub async fn index(&self) -> lumos::Result<lumos::Response> {
        Ok(lumos::ok(&[] as &[u8]))
    }
}

let users = std::sync::Arc::new(UserController::from_container(&container)?);
let router = lumos::routes! {
    resource("/users", UserController, users)
};
```

`#[controller(crate = "lumos_core")]` changes the generated framework path for a crate that uses `lumos-core` directly.

## Models

`#[derive(Model)]` generates the Rusticate model implementation for a named-field struct. The derive uses `::rusticate`, so the owning crate must depend on `rusticate` directly.

```rust,ignore
use rusticate::{HasMany, Model};

#[derive(Model)]
#[model(table = "users", timestamps = true)]
pub struct User {
    #[model(id, auto_increment)]
    pub id: i64,
    pub name: String,
    #[model(has_many = "Post")]
    pub posts: HasMany<Post>,
    #[model(created_at)]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[model(updated_at)]
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

Supported container options include `table`, `timestamps`, and `soft_deletes`. Field options include:

- `id` and `auto_increment` for a primary key.
- `hidden` for a field excluded from persistence output.
- `casts = "json"`, `"string"`, `"bool"`, or `"datetime"` for generated value conversion.
- `has_many = "Post"` and `belongs_to = "Team"` for relations, with optional `foreign_key` and `local_key` overrides.
- `created_at`, `updated_at`, and `deleted_at` for lifecycle fields. Conventionally named fields are inferred when the corresponding container option is enabled.

## Query scopes

`#[scopes]` turns public `scope_*` associated functions into methods on a generated `{Model}Scopes` trait. Each scope takes and returns `Query<Model>`.

```rust,ignore
use rusticate::{scopes, Query};

#[scopes]
impl User {
    pub fn scope_active(query: Query<User>) -> Query<User> {
        query.where_eq("active", true)
    }
}

use crate::UserScopes;

let users = User::query(&db).active().get().await?;
```

The original `User::scope_active(User::query(&db))` call remains available. Scope methods may not shadow existing `Query` methods. Use `#[scopes(crate = "path_to_rusticate")]` only when the default `::rusticate` path is unavailable.

## JSON:API resources

`#[derive(JsonApiResource)]` implements `lumos_jsonapi::Resource` for a named-field struct. Enable either the `jsonapi` or `jsonapi-lite` feature on `lumos` and import the macro from `lumos`.

```rust,ignore
use lumos::{JsonApiResource, ToMany, ToOne};

#[derive(JsonApiResource)]
#[resource(type = "articles")]
pub struct Article {
    #[resource(id)]
    pub id: i64,
    pub title: String,
    #[resource(hidden)]
    pub internal_note: String,
    #[resource(relation)]
    pub author: ToOne<User>,
    #[resource(relation, rename = "reviewers")]
    pub reviewed_by: ToMany<User>,
}
```

`#[resource(type = "...")]` is required. Exactly one field needs `#[resource(id)]`; it is rendered with `Display` and is never an attribute. Other fields become attributes unless marked `hidden`. Use `rename = "..."` to change an attribute or relationship name. Relationship fields must be `ToOne<T>`, `ToMany<T>`, or `Option<ToOne<T>>` and need `#[resource(relation)]`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
