# lumos-core

`lumos-core` is the Lumos kernel. It owns the application boot lifecycle, service container, configuration loading, routing, request extractors, response helpers, error documents, middleware, route registration, and logging.

Most applications depend on `lumos`, which re-exports this crate's public API. Depend on `lumos-core` directly when a lower-level integration needs the kernel without the facade.

## Installation

```toml
[dependencies]
lumos-core = "0.1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## Routing

```rust,no_run
use lumos_core::{get, serve, Router};

#[tokio::main]
async fn main() -> lumos_core::Result<()> {
    let router = Router::new().route("/health", get(|| async { "ok" }));
    serve(router, "127.0.0.1:3000").await
}
```

`Router` and the HTTP verb helpers build routes. `Json`, `Path`, `Query`, and `State` are request extractors. `ok`, `json`, `created`, `no_content`, `redirect`, and `see_other` create responses.

## Application and services

`Application` runs provider registration before provider boot. `ServiceProvider` implementations register bindings with `Container`; boot can resolve services once all providers are registered. `Config` loads application configuration and environment values. `RouteRegistry` records framework routes for `route:list`.

## Error behavior

`AppError` renders a consistent error document. Validation failures are status 422, malformed request bodies are 400, unauthenticated requests are 401, and forbidden requests are 403. With `orm`, Rusticate `NotFound` errors map to 404 and unique-constraint errors map to 409.

## Features

`orm` adds Rusticate error mapping. `validation` adds validation helpers and extractors. `auth` adds password and session support and requires `validation`. `views` adds Askama views. `http-cache` adds the Tower HTTP dependency. `jsonapi` and `jsonapi-lite` provide feature wiring for the facade and cannot be enabled together.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
