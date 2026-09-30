# Lumos framework architecture

Lumos is a compile-time Rust web framework. Applications normally depend on the `lumos` facade; it selects optional capabilities and re-exports the APIs needed to build an application without hiding the underlying crates.

## Crate responsibilities

| Crate | Owns | Does not own |
| --- | --- | --- |
| `lumos` | Public facade, feature selection, route macros | Runtime framework behavior |
| `lumos-core` | HTTP kernel, application lifecycle, configuration, dependency injection, errors, middleware, routing | ORM and JSON:API rendering |
| `rusticate` | Database pools, transactions, models, queries, relations, migrations, observers, factories | HTTP concerns |
| `lumos-jsonapi` | JSON:API negotiation, documents, query parsing, resource projection | Database query execution |
| `lumos-macros` | Controller, model, scope, and resource code generation | Runtime state |
| `lumos-testing` | In-process HTTP and database test utilities | Application production wiring |
| `lumos-cli` | Project scaffolding, generators, development commands | Application-specific registrations |

`lumos` depends on the optional subsystems. `lumos-core` and `rusticate` have no internal dependency on each other. `lumos-jsonapi` uses `lumos-core` and leaves database filtering and sorting to the application. The ORM can therefore run without HTTP, and JSON:API can render resources that do not use Rusticate.

## Composition root

`Application` is the composition root. Register providers, bind services in `Container`, and mount routers during construction. Calling `boot()` closes provider registration and runs boot hooks in registration order. If a hook fails, a retry starts at that provider.

Use `Container::singleton` or `singleton_value` for shared services and `bind` or `bind_value` for per-resolution services. A concrete type has one binding: registering again replaces the previous binding. Controller derives resolve their fields from this container, so service ownership remains visible at application startup.

## HTTP boundary

Lumos re-exports Axum routing and extraction primitives. Use `Router`, `get`, `post`, `Path`, `Query`, `State`, and `Json` directly. Return `lumos::Result<Response>` from handlers that use framework error documents. `AppError` maps failures to HTTP responses.

The framework guarantees these contracts:

- malformed request bodies produce `400`; validation failures produce `422`;
- missing authentication is `401`; failed authorization is `403`;
- `created(location, value)` always produces a `201` with `Location`;
- plain JSON and JSON:API errors share the `errors` document shape.

Use raw Axum or Tokio through `lumos::axum` and `lumos::tokio` whenever that is clearer than a framework helper.

## Feature selection

The core HTTP kernel has no default optional features. Enable only the capabilities an application uses: `orm`, `migrations`, `validation`, `auth`, `views`, `jsonapi`, or `jsonapi-lite`. The two JSON:API modes are mutually exclusive. `auth` enables validation, and JSON:API modes enable the ORM facade feature because they are intended for resource-backed APIs.

## Request lifecycle and boundaries

Lumos keeps Axum's request pipeline intact. A request enters an Axum `Router`, passes the application's middleware, and reaches either a hand-written handler or a `#[controller]` route. Extractors deserialize paths, queries, bodies, and request extensions. Handlers return a response or `AppError`; the error responder renders the latter as Lumos's stable error document.

```text
request -> Axum router and middleware -> handler/extractor
        -> optional Rusticate query or transaction
        -> optional JSON:API resource renderer -> response
```

JSON:API validates media types and document shape at the HTTP boundary. Rusticate owns persistence. Controllers map `ApiQuery` values to storage policy.

## Response and extension policy

Use `AppError::{not_found,bad_request,conflict,unauthorized,forbidden,internal}` for errors. Rusticate errors convert into this taxonomy, so `?` retains their HTTP status mappings. Use `ok`, `json`, `created`, and `no_content` for plain JSON endpoints. `created` requires a `Location` argument. JSON:API `single`, `collection`, and `created` live in `lumos-jsonapi`.

When extending an application, put cross-cutting request work in an Axum layer or `MiddlewareRegistry`, application services in a provider and `Container`, model-wide write behavior in a Rusticate observer, and client-facing shape in a resource type. Use raw Axum and Tokio through `lumos::axum` and `lumos::tokio` when that is clearer. All handles are instance-based; avoid global state.

## Feature selection

Start with no optional features and enable only what the application needs:

```toml
lumos = { version = "0.1", default-features = false, features = ["orm", "validation", "auth", "jsonapi"] }
```

`migrations` implies `orm`; `auth` implies `validation`; `jsonapi` and `jsonapi-lite` are mutually exclusive. `cache`, `queue`, and `http-cache` are public feature placeholders, not production implementations in this release.
