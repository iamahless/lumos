# lumos

Lumos is an MVC web framework for Rust. It provides routing, requests, responses, errors, configuration, a service container, middleware, and application lifecycle support. Optional features add persistence, validation, authentication, views, caching, queues, HTTP caching, and JSON:API.

## Installation

```toml
[dependencies]
lumos = { version = "0.1", default-features = false }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Enable only the capabilities used by the application:

```toml
lumos = { version = "0.1", features = ["orm", "validation", "auth"] }
```

## A small application

```rust,no_run
use lumos::{get, serve, Router};

async fn hello() -> &'static str {
    "Hello, Lumos!"
}

#[tokio::main]
async fn main() -> lumos::Result<()> {
    let router = Router::new().route("/", get(hello));
    serve(router, "127.0.0.1:3000").await
}
```

## Features

| Feature | Adds |
| --- | --- |
| `orm` | Rusticate models, queries, migrations, and relations. |
| `migrations` | Migration support through the ORM. |
| `validation` | `Validated`, `ValidatedQuery`, and the `Validate` derive. |
| `auth` | Argon2 password helpers, sessions, and authentication guards. |
| `views` | Askama template rendering with `View`. |
| `cache` | Cache driver interfaces. |
| `queue` | Queue driver interfaces. |
| `http-cache` | ETag and cache-control middleware. |
| `jsonapi` | Full JSON:API v1.1 rendering. |
| `jsonapi-lite` | Flat JSON:API rendering without includes or sparse fieldsets. |

`jsonapi` and `jsonapi-lite` cannot be enabled together.

## Framework surface

The facade re-exports `lumos-core` for HTTP and application primitives, `rusticate` when `orm` is enabled, and `lumos-jsonapi` when either JSON:API feature is enabled. It also re-exports `#[controller]` and `#[derive(JsonApiResource)]`.

Use `routes!` to mount controllers and explicit routes. Controllers can be built from the container with `#[controller]`. The root repository README and the blog example describe application structure, providers, migrations, and test helpers in more detail.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
