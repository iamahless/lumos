# Publishing a Lumos release

Lumos publishes seven Cargo packages from one workspace version. The current release is `0.1.3`.

| Cargo package | Rust import |
| --- | --- |
| `lumos-rs` | `lumos` |
| `lumos-rs-core` | `lumos_core` |
| `lumos-rs-macros` | `lumos_macros` |
| `lumos-rs-rusticate` | `rusticate` |
| `lumos-rs-jsonapi` | `lumos_jsonapi` |
| `lumos-rs-testing` | `lumos_testing` |
| `lumos-rs-cli` | `lumos_cli` |

The package name is what Cargo downloads. The local dependency key is the Rust import name. For example, an application depends on `lumos-rs` through this alias:

```toml
lumos = { package = "lumos-rs", version = "0.1" }
```

## Prepare the version

Run the release helper from a clean worktree:

```sh
scripts/prepare-release.sh 0.1.4
```

The helper updates `[workspace.package].version`, the seven internal workspace dependency constraints, and `Cargo.lock`. It then runs `cargo metadata --no-deps` and `cargo check --workspace`. Review the resulting diff before committing it.

```sh
git add Cargo.toml Cargo.lock
git commit -m "Release v0.1.4"
git tag v0.1.4
git push origin main v0.1.4
```

The tag starts the publish workflow. The workflow checks out the tag, confirms that the workspace version matches the tag, checks the internal dependency mapping, runs formatting and the workspace tests, then publishes packages in dependency order.

## Prerequisites

The GitHub Actions `crates-io` environment needs a `CARGO_REGISTRY_TOKEN` secret. The token's crates.io account must be an owner of every package in the release. A `403` response means the account is not an accepted owner of that package. Add or accept the crates.io ownership invitation before rerunning the workflow.

Each crates.io package version is immutable. Do not change code and retry a version that has already been uploaded. Prepare the next patch version instead.

## Resume a partial release

crates.io limits how many new packages an account can publish in a short period. A release can therefore stop after some packages have uploaded. Do not create a new tag for that case.

Open the **publish** workflow in GitHub Actions, choose **Run workflow**, select `main`, and enter the existing version without the `v` prefix. For example, enter `0.1.4` to resume `v0.1.4`.

The workflow checks out the existing tag, skips every package that already has that exact version on crates.io, and continues with the remaining packages. When crates.io returns a `429`, it reads the server's retry time, waits until that time, and retries the current package. Other publish errors stop the workflow so that they can be fixed before another run.

## Verify the release

Check the GitHub Actions run first. Each package should report either that it was published or that its exact version was already published. Then verify the public packages:

```sh
cargo info lumos-rs@0.1.4
cargo info lumos-rs-cli@0.1.4
```

Install the published CLI rather than the checkout version when testing the distribution:

```sh
cargo install lumos-rs-cli
lumos new example-app
```

The generated application should resolve `lumos-rs` and `lumos-rs-cli` from crates.io at the release version.
