# lumos-rs-jsonapi

`lumos-jsonapi` renders JSON:API documents and validates JSON:API request negotiation for Lumos applications. It provides resource traits, request-query parsing, response renderers, relationship linkage, filtering, sorting, pagination, sparse fieldsets, and include handling.

Applications usually enable `jsonapi` or `jsonapi-lite` on `lumos` and use its re-exports.

## Features

| Feature | Behavior |
| --- | --- |
| `jsonapi` | Full JSON:API v1.1 documents with compound `included` resources, sparse fieldsets, and pagination links. |
| `jsonapi-lite` | Flat documents with attributes and relationship linkage. `include` and `fields` requests return 400. |

The features are mutually exclusive.

## Resources

Implement `Resource` directly or derive it through `lumos::JsonApiResource`.

```rust,ignore
use lumos::{JsonApiResource, ToOne};

#[derive(JsonApiResource)]
#[resource(type = "articles")]
struct Article {
    #[resource(id)]
    id: i64,
    title: String,
    #[resource(relation)]
    author: ToOne<User>,
}
```

`Resource` supplies the resource type, ID, attributes, and relationship linkage. `ToOne`, `ToMany`, `Relationship`, and `NamedRelationship` represent relationships without exposing persistence models directly.

## Requests and responses

Use `ApiQuery` in handlers to parse filters, sorts, pages, includes, and fieldsets. Use `single`, `collection`, `collection_memory`, or `created` to render a response. The renderer selected by the active feature returns `ApiResult<Response>`.

Routes that opt into JSON:API require `Accept: application/vnd.api+json`. Write requests must also send that content type. Unsupported Accept values return 406, unsupported write content types return 415, and JSON:API responses include `Vary: Accept`.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option.
