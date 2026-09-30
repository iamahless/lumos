# Lumos

Lumos is an MVC Rust web framework with an Eloquent-inspired ORM. It is minimal by default, uses feature flags for optional capabilities, and supports JSON:API. Its API is familiar to Laravel and Lumen developers while remaining idiomatic Rust: it uses no runtime reflection or global mutable state, and proc macros generate framework code at compile time.

> Status: Phase 7, testing helpers. `lumos-testing` provides `TestDb` for migration, seeding, and reset; `TestClient` with a cookie jar; chainable `TestResponse` assertions; and a `factory!` re-export. The blog suite uses these helpers.

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
| `lumos-macros`  | Proc-macros (`#[controller]`, `Model`, `#[scopes]`, `JsonApiResource`).  |
| `rusticate`     | Standalone ORM. Never depends on `lumos-core`.                           |
| `lumos-jsonapi` | JSON:API serialization + negotiation (`jsonapi` / `jsonapi-lite`).       |
| `lumos-testing` | Test helpers: `TestDb`, `TestClient`, response assertions.               |
| `lumos-cli`     | `lumos` dev CLI: `new` / `serve` / `make:*` + app-linked migrate/seed.   |

Dependency direction: `lumos` → `{lumos-core, rusticate, lumos-jsonapi}`; `lumos-jsonapi` → `{lumos-core}` (a `rusticate` query bridge stays deferred: sort/filter are structured values apps map onto queries themselves); `lumos-cli` → `{lumos-core, rusticate}` (runtime glue; the parser and generators are dependency-free); `lumos-core` → nothing internal; `rusticate` → nothing internal.

## Documentation

The reference guide expands on this overview:

- [Framework architecture](docs/architecture.md) explains crate ownership, dependency direction, and extension boundaries.
- [Building an application](docs/building-an-app.md) covers configuration, providers, controllers, routing, migrations, and the CLI.
- [Data APIs and testing](docs/data-apis-and-testing.md) covers Rusticate, JSON:API, authentication, and in-process tests.

The runnable [blog example](apps/blog/README.md) is the end-to-end reference application. Its HTTP test suite is the best source for concrete behavior at the protocol boundary.

## Feature flags

| Feature        | Default | Enables                                     |
| -------------- | ------- | ------------------------------------------- |
| *(core)*       | yes     | Routing, request/response, error handling   |
| `orm`          | no      | Rusticate ORM                               |
| `migrations`   | no      | ORM migrations (implies `orm`)              |
| `validation`   | no      | `Validated` extractors + `Validate` (→ 422) |
| `auth`         | no      | Argon2, sessions, guards (→ `validation`)   |
| `views`        | no      | Askama templates via `View`                 |
| `cache`        | no      | Cache drivers                               |
| `queue`        | no      | Queue drivers (v0.2)                        |
| `http-cache`   | no      | ETag / cache-control middleware             |
| `jsonapi`      | no      | Full JSON:API v1.1 (implies `orm`)          |
| `jsonapi-lite` | no      | Simplified JSON:API (implies `orm`)         |

`jsonapi` + `jsonapi-lite` together is a compile error. Every flag compiles standalone; CI checks each flag, the all-safe set, and the exclusivity violation.

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

`#[controller]` on the struct generates `from_container` (DI); on the impl block it generates `routes()` mapping `index/show/store/update/destroy` to `GET/POST/PUT+PATCH/DELETE` on `/` and `/{id}`. Only the `pub` convention methods you write become routes.

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

One implementation serves SQLite, Postgres, and MySQL (backend chosen from the connection URL); `rusticate::Error` maps onto HTTP statuses (`NotFound` → 404, `UniqueViolation` → 409) so `?` converts directly in actions. Relations (`.with(["posts"])`), casts, scopes, observers, migrations, seeders, and factories are all in; see the `rusticate` crate docs for the full tour.

## CLI in 30 seconds

```sh
cargo install --path crates/lumos-cli   # ships the `lumos` binary
lumos new blog && cd blog               # runnable API skeleton
lumos serve --port 8080                 # cargo run with HOST/PORT set
lumos make:controller Widget            # + make:model/migration/seeder/resource/middleware
```

Commands that need your code, `migrate`, `migrate:rollback`, `migrate:status`, `db:seed`, and `route:list`, run from the skeleton's own binary, pre-wired through `lumos_cli::AppContext`:

```sh
cargo run --bin cli -- route:list
cargo run --bin cli -- migrate
```

`route:list` reads an explicit `RouteRegistry` you build next to each mount (axum routers can't be introspected, so raw axum routes stay invisible by design); the `#[controller]` macro's `route_entries()` keeps registry entries in sync with the routes it generates.

## Example app

`apps/blog` is the full tour in one runnable crate: users, posts, and comments behind JSON:API, with session login, `Validated` inputs, and migrations + seeders. It mirrors `lumos new` output (lib + server + cli bins), so it doubles as the reference project layout:

```sh
cd apps/blog
export DATABASE_URL=sqlite:blog.db?mode=rwc
cargo run -p blog --bin cli -- migrate && cargo run -p blog --bin cli -- db:seed
cargo run -p blog                    # serves 127.0.0.1:3000
cargo test -p blog                   # integration tour (in-memory DB)
```

Its `tests/http.rs` covers login and logout, CRUD, 422 responses, the 401/403 split, 406/415 negotiation, 409 ID mismatch, filters, sorting, pagination, and fieldsets. Add coverage there when adding framework behavior.

## Testing

```toml
# Cargo.toml (dev-dependencies)
lumos-testing = { version = "0.1" }
```

```rust
use lumos_testing::{TestClient, TestDb};

let db = TestDb::memory(&[&CreateUsers]).await?;
db.seed(&[&DemoSeeder]).await?;
let client = TestClient::new(app::build(db.db().clone())?.0);

client.get("/health").await.assert_ok();
client.post_json("/sessions", &login).await.assert_ok(); // jar keeps the cookie
client
    .post_vnd("/posts", &new_post)
    .await
    .assert_created()
    .assert_data("posts", "1");

db.reset(&["posts", "users"]).await?; // children first; sqlite ids restart
```

`factory!` (from rusticate, re-exported) builds deterministic fixtures with sequence numbers; see the `lumos-testing` crate docs.

## Why each core dependency exists

`lumos-core` holds 11 required dependencies (≤ 15 budget); each flag below adds its own:

| Dependency            | Why                                                                                       |
| --------------------- | ----------------------------------------------------------------------------------------- |
| axum                  | Router, server, and extractors. Pulls hyper and matchit transitively; neither is declared directly. |
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

Deliberately absent: `hyper`/`matchit` (via axum), `sqlx` (`rusticate` only), `syn`/`quote`/`proc-macro2` (`lumos-macros` only), `password-hash`/`getrandom` (via argon2's re-exports), `cookie` (hand-rolled parsing), `bytes` (via axum's body API). `mime` joined in Phase 4 (`lumos-jsonapi` negotiation); Phase 5 added none (the CLI's arg parser is hand-rolled, and `chrono` was already in the tree for migration timestamps).

## Contracts

- Validation failures → `422`, never `400`. Malformed bodies → `400`, never `500`.
- `401` means unauthenticated. `403` means forbidden.
- Every `201` carries a `Location` header (`created()` takes it as a required argument).
- Every error renders the same `{ "errors": [...] }` document, JSON:API or not.
- Models are never serialized directly (the `Resource` trait + `JsonApiResource` derive decide what clients see).
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
