# lumos-rs-testing

`lumos-testing` provides test helpers for Lumos applications. It includes an in-memory or file-backed test database, an in-process HTTP client with a cookie jar, response assertions, and a re-export of Rusticate's `factory!` macro.

## Installation

```toml
[dev-dependencies]
lumos-testing = { package = "lumos-rs-testing", version = "0.1" }
```

## Database lifecycle

`TestDb` creates a database for a test suite. It can run migrations, seed data, and reset state between tests. Pass the migrations required by the test to `TestDb::memory` or the file-backed constructor, then call `reset` when a clean database is needed.

## In-process HTTP tests

`TestClient` sends requests directly to a Lumos `Router`; it does not bind a socket. The client keeps cookies between requests, so a login response can establish a session for later requests.

```rust,ignore
use lumos_testing::TestClient;

let client = TestClient::new(router);
client.get("/health").await.assert_ok();
```

`TestRequest` builds requests with headers, JSON bodies, and HTTP methods. `TestResponse` exposes chainable assertions for status codes, headers, JSON payloads, JSON:API documents, and validation errors.

## Factories

Import `factory!` from this crate when fixtures belong beside HTTP and database helpers:

```rust,ignore
use lumos_testing::factory;
```

The macro is implemented by Rusticate and is re-exported here for test-suite ergonomics.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
