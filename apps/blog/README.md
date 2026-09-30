# Blog (Lumos example app)

This JSON:API blog has users, posts, and comments. It uses `#[controller]` routing and `RouteRegistry`, rusticate models with relations, migrations, seeders, `Validated` inputs (422), session authentication (401 vs 403), and JSON:API documents with filtering, sorting, pagination, sparse fieldsets, and includes.

## Run

```sh
cd apps/blog
export DATABASE_URL=sqlite:blog.db?mode=rwc   # default when unset; rwc creates the file
cargo run -p blog --bin cli -- migrate
cargo run -p blog --bin cli -- db:seed
cargo run -p blog                    # serves 127.0.0.1:3000 (migrates on boot)
cargo run -p blog --bin cli -- route:list
```

Seeded logins: `admin@example.com` / `password` and `ada@example.com` / `password`.

## Try it

```sh
curl -s localhost:3000/health
# JSON:API reads need the media type:
curl -s -H 'Accept: application/vnd.api+json' \
  'localhost:3000/posts?include=author&sort=-id&page[size]=5'
# Login (wrong password → 401):
curl -s -c jar -H 'Content-Type: application/json' -d \
  '{"email":"ada@example.com","password":"password"}' \
  localhost:3000/sessions
# Write with the session cookie (anonymous → 401, stranger's post → 403):
curl -s -b jar -H 'Accept: application/vnd.api+json' \
  -H 'Content-Type: application/vnd.api+json' -d \
  '{"data":{"type":"posts","attributes":{"title":"Hi","body":"Hello."}}}' \
  localhost:3000/posts -D - -o /dev/null   # 201 + Location
```

## Test

```sh
cargo test -p blog   # integration tour over an in-memory database
```

`tests/http.rs` walks the whole product on `lumos-testing` (`TestDb` setup, `TestClient` with its cookie jar, chainable assertions): login/logout, post CRUD, comment flow, validation 422s, 401/403 splits, negotiation 406/415s, id-mismatch 409s, filters, sorting, pagination, and fieldsets. New framework behavior should extend that tour, not live only in unit tests.
