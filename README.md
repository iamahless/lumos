# Lumos

A lightweight, MVC, batteries-included Rust web framework with an Eloquent-inspired ORM. Minimal by default, feature-flag driven, and JSON:API-capable — built so a Laravel/Lumen developer feels at home while the framework stays idiomatic Rust: no runtime reflection, no global mutable state, all magic at compile time via proc-macros.

> **Status: Phase 3 — validation + auth + views.** On top of the Phase 2 Rusticate ORM: `Validated` request extractors (`validator` derive → `422` with per-field pointers), argon2 passwords + token sessions + the `CurrentUser` guard (`401`/`403` split), and Askama templates via the `View` responder — all in `lumos-core` behind `validation`/`auth`/`views`, re-exported from the facade. JSON:API, CLI, and the example app land in Phases 4–7.

## Quickstart

```toml
# Cargo.toml
[dependencies]
lumos = { version = "0.1", default-features = false }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use lumos::{get, serve, Router};

async fn hello() -> &'static str {
    "Hello, Lumos!"
}

#[tokio::main]
async fn main() -> lumos::Result<()> {
    serve(Router::new().route("/", get(hello)), "127.0.0.1:3000").await
}
```

## Crates

| Crate           | Role                                                                     |
| --------------- | ------------------------------------------------------------------------ |
| `lumos`         | Facade: feature flags + curated re-exports. This is what apps depend on. |
| `lumos-core`    | Kernel: router, HTTP, container, config, errors, middleware, providers.  |
| `lumos-macros`  | Proc-macros (`#[controller]`, `Model`, `#[scopes]`; resources in Phase 4). |
| `rusticate`     | Standalone ORM. Never depends on `lumos-core`.                            |
| `lumos-jsonapi` | JSON:API serialization + negotiation (Phase 4).                          |
| `lumos-testing` | Test helpers (Phase 7).                                                  |
| `lumos-cli`     | `lumos` developer binary (Phase 5).                                      |

Dependency direction: `lumos` → `{lumos-core, rusticate, lumos-jsonapi}`;
`lumos-jsonapi` → `{lumos-core, rusticate}` (the sole bridge);
`lumos-core` → nothing internal; `rusticate` → nothing internal.

## Feature flags

| Feature        | Default | Enables                                      |
| -------------- | ------- | -------------------------------------------- |
| *(core)*       | yes     | Routing, request/response, error handling    |
| `orm`          | no      | Rusticate ORM                                |
| `migrations`   | no      | ORM migrations (implies `orm`)               |
| `validation`   | no      | `Validated` extractors + `Validate` (→ 422)  |
| `auth`         | no      | Argon2, sessions, guards (→ `validation`)   |
| `views`        | no      | Askama templates via `View`                  |
| `cache`        | no      | Cache drivers                                |
| `queue`        | no      | Queue drivers (v0.2)                         |
| `http-cache`   | no      | ETag / cache-control middleware              |
| `jsonapi`      | no      | Full JSON:API v1.1 (Phase 4, implies `orm`)  |
| `jsonapi-lite` | no      | Simplified JSON:API (Phase 4, implies `orm`) |

`jsonapi` + `jsonapi-lite` together is a compile error. Every flag
compiles standalone; CI checks each flag, the all-safe set, and the
exclusivity violation.

## MVC in 30 seconds

```rust
use lumos::{controller, routes, Container, Path, Response, Result, Router};
use std::sync::Arc;

#[controller]
pub struct UserController {
    users: UserService, // auto-resolved from the container
}

#[controller]
impl UserController {
    pub async fn show(&self, Path(id): Path<i64>) -> Result<Response> {
        Ok(lumos::ok(&format!("user {id}")))
    }
}

// In a provider or main:
let controller = Arc::new(UserController::from_container(&container)?);
let api: Router = routes! {
    resource("/users", UserController, controller),
};
```

`#[controller]` on the struct generates `from_container` (DI); on the impl
block it generates `routes()` mapping `index/show/store/update/destroy` to
`GET/POST/PUT+PATCH/DELETE` on `/` and `/{id}`. Only the `pub` convention
methods you write become routes.

## ORM in 30 seconds

```rust
use lumos::rusticate::{Changeset, Model, Schema, DB};

#[derive(Model)]
#[model(table = "users", timestamps = true)]
pub struct User {
    #[model(id, auto_increment)]
    pub id: i64,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

let db = DB::connect("sqlite:app.db").await?;
Schema::new(&db).create("users", |t| {
    t.id();
    t.string("name");
    t.timestamps();
}).await?;

User::create(&db, Changeset::new().set("name", "Ada")).await?;
let found = User::query(&db).where_eq("name", "Ada").first_or_fail().await?;
let page = User::query(&db).order_by("id").paginate(20, 1).await?;

db.transaction(|tx| async move {
    // Queries join the transaction via `.on(&tx)` or by taking `&tx` directly.
    User::query(&db).on(&tx).where_eq("name", "Ada").first_or_fail().await?;
    Ok::<(), lumos::rusticate::Error>(())
}).await?;
```

One implementation serves SQLite, Postgres, and MySQL (backend chosen
from the connection URL); `rusticate::Error` maps onto HTTP statuses
(`NotFound` → 404, `UniqueViolation` → 409) so `?` converts directly in
actions. Relations (`.with(["posts"])`), casts, scopes, observers,
migrations, seeders, and factories are all in; see the `rusticate` crate
docs for the full tour.

## Why each core dependency exists

`lumos-core` holds 11 required dependencies (≤ 15 budget); each flag below adds its own:

| Dependency            | Why                                                                                       |
| --------------------- | ----------------------------------------------------------------------------------------- |
| axum                  | Router, server, extractors. Pulls hyper + matchit transitively — never declared directly. |
| tokio                 | Async runtime (`net`, `rt-multi-thread`, `macros`, `signal` only).                        |
| serde                 | DTO/config (de)serialization.                                                             |
| serde_json            | JSON bodies and error documents.                                                          |
| thiserror             | `AppError` taxonomy → status-code mapping.                                                |
| tracing               | Backend for the `Log` facade.                                                             |
| tracing-subscriber    | Default `fmt` + `RUST_LOG` subscriber, installed by `Log::init`.                          |
| tower                 | `Service`/`Layer` traits for middleware composition.                                      |
| async-trait           | Object-safe async traits (`ServiceProvider`, `SessionStore`).                             |
| toml                  | `config/*.toml` parsing.                                                                  |
| dotenvy               | `.env` loading.                                                                           |
| tower-http (optional) | ETag / cache-control middleware behind `http-cache`; zero cost when off.                  |
| rusticate (optional)  | ORM error mapping behind `orm`; zero cost when off.                                       |
| validator (optional)  | `Validate` derive behind `validation`; zero cost when off.                                |
| argon2 (optional)     | Password hashing + session RNG behind `auth`; zero cost when off.                         |
| askama (optional)     | Compile-time templates behind `views`; zero cost when off.                                |

Deliberately absent: `hyper`/`matchit` (via axum), `sqlx` (`rusticate`
only), `syn`/`quote`/`proc-macro2` (`lumos-macros` only),
`password-hash`/`getrandom` (via argon2's re-exports), `cookie`
(hand-rolled parsing), `mime` + `bytes` (Phase 4 negotiation).

## Contracts

- Validation failures → `422`, never `400`. Malformed bodies → `400`, never `500`.
- `401` is unauthenticated, `403` is forbidden — never confused.
- Every `201` carries a `Location` header (`created()` takes it as a required argument).
- Every error renders the same `{ "errors": [...] }` document, JSON:API or not.
- Models are never serialized directly (enforced by the Resource layer in Phase 4).
- Escape hatches: raw axum (`lumos::axum`), raw tokio (`lumos::tokio`), `Application::into_router`, `MiddlewareRegistry`, and `routes!` all compose with hand-written code.

## Development

Cargo's home must be writable; if your sandbox blocks `~/.cargo`:

```sh
export CARGO_HOME=/tmp/lumos-cargo-home
cargo test --workspace          # unit + integration + doctests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Feature matrix (also in CI):

```sh
for f in orm migrations validation auth views cache queue http-cache jsonapi jsonapi-lite; do
  cargo check -p lumos --no-default-features --features "$f"
done
# Must FAIL:
cargo check -p lumos --features jsonapi,jsonapi-lite
```

MSRV 1.88. License: MIT OR Apache-2.0.
