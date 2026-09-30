# Lumos data APIs and testing

## Rusticate models and transactions

Derive `Model` for a named struct and describe table metadata with `#[model]`. The derive creates hydration, changesets, timestamps, soft-delete behavior, and typed relationship helpers. Build queries with `Model::query`, use `Changeset` for create/update input, and keep models separate from wire resources.

```rust
let created = User::create(&db, Changeset::new().set("name", "Ada")).await?;
let found = User::find_or_fail(&db, created.id).await?;
let page = User::query(&db).where_eq("active", true).paginate(20, 1).await?;
```

`create` and `update` require non-empty changesets. `update` stamps `updated_at` when configured, runs pre-write observers, and writes the model after those hooks run. `save` writes current non-key fields. Soft-delete models make `delete` stamp a deletion time; `force_delete` physically deletes. Define allowed request filters and sort fields in the controller.

Use `DB::transaction` for the ordinary all-or-nothing case. The closure gets a cloneable transaction handle; point a query at it with `.on(&tx)`. For manual lifecycle control use `DB::begin`, then commit or roll back exactly once. An unfinished transaction rolls back when dropped.

Transactions are cloneable handles to one serialized connection. A query uses a transaction only when it receives that target or `.on(&tx)`; queries built from the original `DB` remain outside it. Once committed or rolled back, every clone is finished. Migration registration order is the schema contract; `MigrationStatus::ran()` derives applied state from its batch.

Observers run around writes. Pre-write hooks can change the model; Lumos persists the final post-hook model state. Migration status has a batch number when applied; call `MigrationStatus::ran()` instead of maintaining a separate boolean.

## JSON API resources

Enable either `jsonapi` for full compound documents or `jsonapi-lite` for the flat renderer. Never serialize models directly. Define a separate `Resource` or derive `JsonApiResource`; it owns the type, id, attributes, and relationships exposed to clients.

`ToOne::id` creates linkage only. `ToOne::loaded` creates a loaded relation; the loaded resource supplies the identity used for both linkage and `included`, so the two cannot disagree. The renderer never fetches data; an application must eager-load every relationship named by `include`.

JSON:API reads accept `application/vnd.api+json`; an explicit incompatible `Accept` header is `406`. Writes require that content type, so a plain JSON write is `415`. Malformed JSON or document envelopes are `400`. `JsonApiBody<T>` validates the `data` envelope and resource type; update handlers should compare an optional body id with the URL id and return `409` on disagreement.

`ApiQuery` parses filters, sort order, pagination, sparse fieldsets, and include paths. Map those values to a Rusticate query in application code.

Use `single` for one resource, `collection` for database-supplied items plus a total, `collection_memory` for a small in-memory collection, and JSON:API `created` for a 201 response. The full renderer supports includes and sparse fieldsets. The lite renderer rejects compound includes. Both reject unknown fields and relationship paths.

## In process tests

Use `TestDb` for an isolated database and `TestClient` for Axum requests:

```rust
let db = TestDb::memory(&[&CreateUsers]).await?;
db.seed(&[&DemoSeeder]).await?;
let client = TestClient::new(app::build(db.db().clone())?.0);

client.get("/health").await.assert_ok();
client.post_json("/sessions", &login).await.assert_ok();
```

`TestClient` maintains a simple cookie jar. `json()` and `vnd()` set one coherent body encoding and its media headers; a later encoding call replaces the earlier one. Use `header` for intentionally raw or repeated headers. `TestResponse` retains both raw bytes and a parsed JSON cache, so assertions can cover protocol details and JSON:API data in one response object.

`TestDb::memory` creates an isolated database and runs supplied migrations. Use `seed` for fixtures. `reset` clears named tables in the supplied order, so children must precede parents for foreign keys; SQLite IDs reset too.

`TestClient` sends requests directly through a router, so no listening port is needed. Use convenience methods for common verbs or `request(method, uri)` for custom requests. `json()` and `vnd()` install one coherent body encoding; a later encoding replaces it. `TestResponse` provides status, headers, raw/text body, parsed JSON, JSON:API data, and error-pointer assertions with body excerpts on failure.

The blog HTTP suite demonstrates login, authorization, CRUD, JSON:API negotiation, filters, sorting, pagination, sparse fieldsets, and includes. Extend that suite whenever a framework-level HTTP behavior changes.
