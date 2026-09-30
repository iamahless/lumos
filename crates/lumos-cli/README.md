# lumos-rs-cli

`lumos-cli` is the developer CLI for Lumos. The `lumos` binary creates projects, generates application files, starts a development server, and prints help and version information. Its library API supports migration, seeding, and route-list commands from an application's own CLI binary.

## Installation

Install the binary from crates.io:

```sh
cargo install lumos-rs-cli
```

Or depend on it in an application that needs app-linked commands:

```toml
[dependencies]
lumos-cli = { package = "lumos-rs-cli", version = "0.1" }
```

## Project commands

```sh
lumos new my-app
lumos serve --port 8080
lumos make:controller User
lumos make:model User
lumos make:migration create_users
lumos make:resource User
lumos make:middleware Audit
lumos make:seeder Demo
```

`new` writes a runnable application skeleton. The generated project includes a small application binary and `src/bin/cli.rs`, which wires application-specific migration, seed, and route commands through `AppContext`.

## App-linked commands

`migrate`, `migrate:rollback`, `migrate:status`, `db:seed`, and `route:list` require application migrations, providers, and route registration. Run these from the generated application's CLI binary rather than the standalone `lumos` binary.

The library exposes `run` for project-local commands and `run_app` with `AppContext` for app-linked commands. `render_new`, `render_make`, and the other render helpers return generated file content before writing, which is useful for custom tooling.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
