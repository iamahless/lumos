# Building a Lumos application

This guide follows the project layout from `lumos new`. The blog application in `apps/blog` shows the full flow.

## Project layout

```text
config/              TOML configuration files
src/lib.rs           router and RouteRegistry construction
src/main.rs          HTTP server bootstrap
src/bin/cli.rs       app-linked database and route commands
src/controllers/     request handlers
src/models/          Rusticate persistence models
src/resources/       JSON:API resources and input DTOs
database/            migrations and seeders
```

Keep `build` in `src/lib.rs` usable from executables and integration tests. The blog's `build(DB)` returns `(Router, RouteRegistry)` for this reason.

## Providers and services

Use provider `register` for synchronous composition and `boot` for work that needs all providers registered. Registration after boot begins is rejected. Successful boot is idempotent; if a provider fails, retrying resumes at that provider. Container bindings are concrete-type based: `singleton` and `singleton_value` share one value; `bind` and `bind_value` run a factory on each resolution. Re-registering a type replaces its prior binding.

Use an `Arc<T>` controller field for a non-`Clone` shared service, or `T` for a cloneable value. `#[controller]` generates `from_container` from those fields.

## Configure and start

Store TOML files under `config/`. `Config::load()` optionally loads `.env` and then reads each `config/*.toml` file. Read a whole file with `Config::file` or one value with a dotted key such as `app.name`.

For example, `config/app.toml` may contain:

```toml
name = "my-app"
[server]
port = 3000
```

Read a whole file with `Config::file` or one value with `config.get::<String>("app.name")?`. Missing files, invalid TOML, and type mismatches fail at startup with a message for the operator.

Build a router explicitly and serve it:

```rust
use lumos::{get, serve, Router};

async fn health() -> &'static str { "ok" }

#[tokio::main]
async fn main() -> lumos::Result<()> {
    serve(Router::new().route("/health", get(health)), "127.0.0.1:3000").await
}
```

Use `Application` and providers when service bindings and route mounting need one home. Provider `register` hooks bind and mount. Their asynchronous `boot` hooks run after registration is complete.

## Controllers and routes

`#[controller]` on a struct generates `from_container`, resolving each field from `Container`. Applying it to an implementation generates conventional resource routes for public methods:

| Method | Route |
| --- | --- |
| `index` | `GET /` |
| `show` | `GET /{id}` |
| `store` | `POST /` |
| `update` | `PUT` and `PATCH /{id}` |
| `destroy` | `DELETE /{id}` |

Mount generated resource routes with `routes!`. Add a `RouteRegistry` entry for each route that should appear in `route:list`. Axum routers cannot be introspected, so raw routes are absent unless you register them yourself.

Use `group!` for a prefixed router with middleware and `route` entries in `routes!` for hand-written routes. The registry only powers `route:list`; it does not participate in dispatch. Register raw routes manually when they must appear in the CLI output.

## Database lifecycle

Enable `orm` for Rusticate and `migrations` when the application runs migrations. Models derive `Model`; migrations implement `Migration`; seeders implement `Seeder`.

Use the application CLI binary for commands that require application-owned registrations:

```sh
cargo run --bin cli -- migrate
cargo run --bin cli -- migrate:status
cargo run --bin cli -- migrate:rollback
cargo run --bin cli -- db:seed
cargo run --bin cli -- route:list
```

The standalone `lumos` executable owns `new`, `serve`, and `make:*` commands. Generated applications contain a second CLI binary that creates an `AppContext` with their database, migrations, seeders, and route registry.

| Commands | Run from | Why |
| --- | --- | --- |
| `lumos new`, `lumos serve`, `lumos make:*` | Framework CLI | No app registrations needed |
| `migrate*`, `db:seed`, `route:list` | App CLI binary | Needs app database, registrations, or registry |

Generators create controller, model, migration, seeder, resource, and middleware skeletons. They refuse to overwrite an existing target unless `--force` is explicit.

## Authentication and views

Enable `auth` for Argon2 password helpers, `SessionStore`, `CurrentUser`, and cookie helpers. Add a session store as an Axum request extension. `CurrentUser` accepts either a bearer token or the Lumos session cookie; call `CurrentUser::require` for a scope check.

Enable `views` for Askama `View` responses. Enable `validation` for `Validated<T>` request bodies and `ValidatedQuery<T>` query strings. Both map validator failures to the standard 422 error document.

`login(store, user_id, scopes, ttl)` creates a session after the application has verified credentials. `CurrentUser` prefers bearer tokens, then checks the `lumos_session` cookie. Missing or invalid credentials are `401`; a failed `CurrentUser::require` scope check is `403`. Expired sessions are removed from their store. `MemorySessionStore` is for examples and tests; production apps should implement durable `SessionStore` storage.
