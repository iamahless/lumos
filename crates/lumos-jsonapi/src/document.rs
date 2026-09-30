//! Full JSON:API v1.1 document rendering.
//!
//! [`single`] renders one resource, [`collection`] a pre-paged slice with
//! its total, [`collection_memory`] a loaded dataset end-to-end
//! (filter → sort → paginate in memory), [`created`] a 201 with `Location`.
//! All four validate `include` / `fields` paths against the resources and
//! 400 on unknown names; all answer `application/vnd.api+json`.
//!
//! Documents carry linkage for every relationship, a deduped compound
//! `included` array for loaded `include` targets (same type+id never twice,
//! per spec), and self/first/last/prev/next collection links. Sparse
//! fieldsets filter attributes and relationships alike: a name must appear
//! in `fields[type]` to be rendered. `include` targets the app did not load
//! are 400s — linkage-only includes would silently under-deliver.
//!
//! # Examples
//!
//! ```rust
//! use lumos_jsonapi::{single, ApiQuery, AttributeMap, NamedRelationship, Resource};
//!
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//! impl Resource for User {
//!     const TYPE: &'static str = "users";
//!     fn resource_id(&self) -> String {
//!         self.id.to_string()
//!     }
//!     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
//!         let mut map = AttributeMap::new();
//!         map.insert("name".to_string(), self.name.clone().into());
//!         Ok(map)
//!     }
//!     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
//!         Vec::new()
//!     }
//! }
//!
//! let query = ApiQuery::parse(&[], "/users").unwrap();
//! let response = single(&User { id: 1, name: "Ada".to_string() }, &query).unwrap();
//! assert_eq!(response.status(), lumos_core::StatusCode::OK);
//! ```

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use lumos_core::{AppError, Result};

use crate::negotiate::{api_response, ApiResult};
use crate::query::{memory_page, ApiQuery};
use crate::resource::{DynResource, NamedRelationship, Relationship, Resource};

/// Looks up a relationship by name on a rendered relationship list.
fn find_relation<'a>(
    relations: &'a [NamedRelationship<'a>],
    name: &str,
) -> Option<&'a Relationship<'a>> {
    relations
        .iter()
        .find(|named| named.name == name)
        .map(|named| &named.relation)
}

/// Tracks `included` emission: which (type, id) pairs rendered, and which
/// sub-paths already resolved below each. The same target reached twice
/// with different sub-paths resolves the delta without re-rendering.
#[derive(Debug, Default)]
struct Emitted {
    resolved: BTreeMap<(&'static str, String), Vec<Vec<String>>>,
}

impl Emitted {
    /// Marks a target emitted. Returns true on first sight.
    fn mark(&mut self, resource_type: &'static str, id: String) -> bool {
        use std::collections::btree_map::Entry;
        match self.resolved.entry((resource_type, id)) {
            Entry::Vacant(slot) => {
                slot.insert(Vec::new());
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    /// Records sub-paths resolved below a target, returning the newly added
    /// ones (already-resolved paths resolve once, so cycles terminate).
    fn add_paths(
        &mut self,
        resource_type: &'static str,
        id: String,
        paths: &[Vec<String>],
    ) -> Vec<Vec<String>> {
        let recorded = self.resolved.entry((resource_type, id)).or_default();
        let mut fresh = Vec::new();
        for path in paths {
            if !recorded.contains(path) {
                recorded.push(path.clone());
                fresh.push(path.clone());
            }
        }
        fresh
    }
}

/// Renders one resource as a `200` JSON:API document.
///
/// `include` / `fields` apply; collection parameters (sort/filter/page) are
/// ignored for single resources. Unknown include/fieldset names are 400s.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{single, ApiQuery, AttributeMap, NamedRelationship, Resource};
///
/// struct Status {
///     id: String,
/// }
/// impl Resource for Status {
///     const TYPE: &'static str = "statuses";
///     fn resource_id(&self) -> String {
///         self.id.clone()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let query = ApiQuery::parse(&[], "/statuses").unwrap();
/// let response = single(&Status { id: "ok".to_string() }, &query).unwrap();
/// assert_eq!(response.headers()["content-type"], "application/vnd.api+json");
/// ```
pub fn single<T: Resource>(resource: &T, query: &ApiQuery) -> ApiResult<lumos_core::Response> {
    let erased: &dyn DynResource = resource;
    validate_query(erased, query)?;
    let mut included = Vec::new();
    let mut emitted = Emitted::default();
    let data = render_object(erased, query, &mut included, &mut emitted)?;
    Ok(document(data, included, lumos_core::StatusCode::OK))
}

/// Renders the current page of a collection as a `200` JSON:API document.
///
/// `items` is exactly this page (the app pages in SQL); `total` is the
/// full filtered count for link building. Query paths validate against the
/// page items — an empty page validates nothing and renders empty data.
/// For fully in-memory datasets see [`collection_memory`].
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{collection, ApiQuery, AttributeMap, NamedRelationship, Resource};
///
/// struct Item {
///     id: i64,
/// }
/// impl Resource for Item {
///     const TYPE: &'static str = "items";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let query = ApiQuery::parse(&[], "/items").unwrap();
/// let items = vec![Item { id: 1 }];
/// let response = collection(&items, 1, &query).unwrap();
/// assert_eq!(response.status(), lumos_core::StatusCode::OK);
/// ```
pub fn collection<T: Resource>(
    items: &[T],
    total: u64,
    query: &ApiQuery,
) -> ApiResult<lumos_core::Response> {
    for item in items {
        validate_query(item, query)?;
    }
    render_collection(
        items.iter().map(|item| item as &dyn DynResource).collect(),
        total,
        query,
    )
}

/// Filters, sorts, and paginates loaded items, then renders the page.
///
/// Same document as [`collection`], with `total` set to the filtered count.
/// Sort/filter fields must exist in every item's attributes (400 otherwise).
/// SQL-backed apps page in the database and call [`collection`] instead.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{collection_memory, ApiQuery, AttributeMap, NamedRelationship, Resource};
///
/// struct Item {
///     id: i64,
/// }
/// impl Resource for Item {
///     const TYPE: &'static str = "items";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let query = ApiQuery::parse(&[], "/items").unwrap();
/// let items = vec![Item { id: 1 }, Item { id: 2 }];
/// let response = collection_memory(&items, &query).unwrap();
/// assert_eq!(response.status(), lumos_core::StatusCode::OK);
/// ```
pub fn collection_memory<T: Resource>(
    items: &[T],
    query: &ApiQuery,
) -> ApiResult<lumos_core::Response> {
    let (indices, total) = memory_page(items, query)?;
    let page: Vec<&dyn DynResource> = indices
        .iter()
        .map(|index| &items[*index] as &dyn DynResource)
        .collect();
    for item in page.iter().copied() {
        validate_query(item, query)?;
    }
    render_collection(page, total, query)
}

/// Renders a newly created resource as `201` with a `Location` header.
///
/// Behaves like [`single`] plus the creation status and location. An
/// invalid location is a 500 — a 201 without `Location` must never ship.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{created, ApiQuery, AttributeMap, NamedRelationship, Resource};
///
/// struct Item {
///     id: i64,
/// }
/// impl Resource for Item {
///     const TYPE: &'static str = "items";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let query = ApiQuery::parse(&[], "/items").unwrap();
/// let response = created(&Item { id: 7 }, "/items/7", &query).unwrap();
/// assert_eq!(response.status(), lumos_core::StatusCode::CREATED);
/// assert_eq!(response.headers()["location"], "/items/7");
/// ```
pub fn created<T: Resource>(
    resource: &T,
    location: &str,
    query: &ApiQuery,
) -> ApiResult<lumos_core::Response> {
    let mut response = single(resource, query)?;
    *response.status_mut() = lumos_core::StatusCode::CREATED;
    let value = lumos_core::axum::http::HeaderValue::from_str(location)
        .map_err(|_| AppError::internal(format!("invalid Location header value: {location:?}")))?;
    response
        .headers_mut()
        .insert(lumos_core::axum::http::header::LOCATION, value);
    Ok(response)
}

/// Validates `include` paths and `fields` names against one resource.
/// Unknown relationships, unloaded include targets, and unknown fieldset
/// names are 400s. Sort/filter validate in [`memory_page`] (memory path)
/// or the app's SQL layer (paged path).
fn validate_query(resource: &dyn DynResource, query: &ApiQuery) -> Result<()> {
    let relations = resource.relationships();
    for path in &query.include {
        validate_path(resource, &relations, path)?;
    }
    if let Some(names) = query.fields.get(resource.resource_type()) {
        let attributes = resource.attributes()?;
        for name in names {
            let known_attribute = attributes.contains_key(name);
            let known_relation = relations.iter().any(|named| named.name == name);
            if !known_attribute && !known_relation {
                return Err(AppError::bad_request(format!(
                    "unknown field {name:?} for type {:?}",
                    resource.resource_type()
                )));
            }
        }
    }
    Ok(())
}

/// Validates one include path, walking loaded targets.
fn validate_path(
    resource: &dyn DynResource,
    relations: &[NamedRelationship<'_>],
    path: &[String],
) -> Result<()> {
    let Some((head, rest)) = path.split_first() else {
        return Ok(());
    };
    let Some(relation) = find_relation(relations, head) else {
        return Err(AppError::bad_request(format!(
            "unknown relationship {head:?} on type {:?}",
            resource.resource_type()
        )));
    };
    if rest.is_empty() {
        // Every target on a terminal path must be loaded: linkage-only
        // includes would silently under-deliver.
        let unloaded = match relation {
            Relationship::ToOne(target) => {
                target.as_ref().is_some_and(|entry| entry.loaded().is_none())
            }
            Relationship::ToMany(entries) => entries.iter().any(|entry| entry.loaded().is_none()),
        };
        if unloaded {
            return Err(AppError::bad_request(format!(
                "include path {path:?} is not loaded on type {:?}",
                resource.resource_type()
            )));
        }
        return Ok(());
    }
    // Non-terminal segments recurse into loaded targets.
    let targets: Vec<&dyn DynResource> = match relation {
        Relationship::ToOne(target) => target
            .as_ref()
        .and_then(|entry| entry.loaded())
            .into_iter()
            .collect(),
        Relationship::ToMany(entries) => entries.iter().filter_map(|entry| entry.loaded()).collect(),
    };
    if targets.is_empty() {
        return Err(AppError::bad_request(format!(
            "include path {path:?} is not loaded on type {:?}",
            resource.resource_type()
        )));
    }
    for target in targets {
        validate_path(target, &target.relationships(), rest)?;
    }
    Ok(())
}

/// Renders page items plus links into the final response.
fn render_collection(
    page: Vec<&dyn DynResource>,
    total: u64,
    query: &ApiQuery,
) -> ApiResult<lumos_core::Response> {
    let mut included = Vec::new();
    let mut emitted = Emitted::default();
    let mut data = Vec::with_capacity(page.len());
    for item in page {
        data.push(render_object(item, query, &mut included, &mut emitted)?);
    }
    let links = query.page_links(total);
    let mut document = Map::new();
    document.insert("data".to_string(), Value::Array(data));
    if !included.is_empty() {
        document.insert("included".to_string(), Value::Array(included));
    }
    document.insert(
        "links".to_string(),
        serde_json::json!({
            "self": links.self_link,
            "first": links.first,
            "last": links.last,
            "prev": links.prev,
            "next": links.next,
        }),
    );
    Ok(api_response(
        Value::Object(document),
        lumos_core::StatusCode::OK,
    ))
}

/// Assembles the `{data, included?}` envelope for single resources.
fn document(
    data: Value,
    included: Vec<Value>,
    status: lumos_core::StatusCode,
) -> lumos_core::Response {
    let mut document = Map::new();
    document.insert("data".to_string(), data);
    if !included.is_empty() {
        document.insert("included".to_string(), Value::Array(included));
    }
    api_response(Value::Object(document), status)
}

/// Renders one resource object, collecting `included` entries.
fn render_object(
    resource: &dyn DynResource,
    query: &ApiQuery,
    included: &mut Vec<Value>,
    emitted: &mut Emitted,
) -> Result<Value> {
    let allowed = query.fields.get(resource.resource_type());
    let mut attributes = resource.attributes()?;
    if let Some(names) = allowed {
        attributes.retain(|name, _| names.iter().any(|wanted| wanted == name));
    }
    let mut object = Map::new();
    object.insert(
        "type".to_string(),
        Value::String(resource.resource_type().to_string()),
    );
    object.insert("id".to_string(), Value::String(resource.resource_id()));
    object.insert("attributes".to_string(), Value::Object(attributes));

    let relations = resource.relationships();
    let mut rendered = Map::new();
    for named in &relations {
        if allowed.is_some_and(|names| !names.iter().any(|wanted| wanted == named.name)) {
            continue;
        }
        rendered.insert(named.name.to_string(), render_linkage(named));
    }
    if !rendered.is_empty() {
        object.insert("relationships".to_string(), Value::Object(rendered));
    }
    resolve_includes(&relations, query, included, emitted)?;
    Ok(Value::Object(object))
}

/// Renders one relationship object (`{data: linkage}`).
fn render_linkage(named: &NamedRelationship<'_>) -> Value {
    let linkage = match &named.relation {
        Relationship::ToOne(target) => target.as_ref().map(linkage_object).unwrap_or(Value::Null),
        Relationship::ToMany(entries) => Value::Array(entries.iter().map(linkage_object).collect()),
    };
    let mut object = Map::new();
    object.insert("data".to_string(), linkage);
    Value::Object(object)
}

/// Resolves every include path rooted at this resource's relationships.
/// Paths sharing a relationship emit their targets once with the union of
/// tails, so divergent paths (`author.books` + `author.awards`) both land.
fn resolve_includes(
    relations: &[NamedRelationship<'_>],
    query: &ApiQuery,
    included: &mut Vec<Value>,
    emitted: &mut Emitted,
) -> Result<()> {
    for named in relations {
        let mut tails: Vec<Vec<String>> = Vec::new();
        for path in &query.include {
            let Some((head, rest)) = path.split_first() else {
                continue;
            };
            if head == named.name {
                tails.push(rest.to_vec());
            }
        }
        if tails.is_empty() {
            continue;
        }
        for target in relation_targets(&named.relation) {
            emit_included(target, &tails, query, included, emitted)?;
        }
    }
    Ok(())
}

/// Borrows the loaded targets of one relationship (validation guarantees
/// non-emptiness wherever an include path needs them).
fn relation_targets<'a>(relation: &Relationship<'a>) -> Vec<&'a dyn DynResource> {
    match relation {
        Relationship::ToOne(target) => target
            .as_ref()
        .and_then(|entry| entry.loaded())
            .into_iter()
            .collect(),
        Relationship::ToMany(entries) => entries.iter().filter_map(|entry| entry.loaded()).collect(),
    }
}

/// Emits one loaded target into `included` (first sight only), resolving
/// the not-yet-resolved sub-paths below it. Re-encountered targets resolve
/// their delta without re-rendering, so divergent paths and cycles both
/// terminate correctly.
fn emit_included(
    target: &dyn DynResource,
    tails: &[Vec<String>],
    query: &ApiQuery,
    included: &mut Vec<Value>,
    emitted: &mut Emitted,
) -> Result<()> {
    let fresh = emitted.mark(target.resource_type(), target.resource_id());
    let mut effective: Vec<Vec<String>> = tails.to_vec();
    if !fresh {
        effective = emitted.add_paths(target.resource_type(), target.resource_id(), tails);
        if effective.is_empty() {
            return Ok(());
        }
    } else {
        emitted.add_paths(target.resource_type(), target.resource_id(), tails);
    }
    let narrowed = ApiQuery {
        include: effective,
        fields: query.fields.clone(),
        sort: Vec::new(),
        filters: Vec::new(),
        page: query.page,
        base_path: query.base_path.clone(),
    };
    if fresh {
        let rendered = render_object(target, &narrowed, included, emitted)?;
        included.push(rendered);
    } else {
        resolve_includes(&target.relationships(), &narrowed, included, emitted)?;
    }
    Ok(())
}

/// Builds one linkage object (`{type, id}`).
fn linkage_object(target: &crate::RelationTarget<'_>) -> Value {
    serde_json::json!({"type": target.resource_type(), "id": target.id()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ToMany, ToOne};

    #[derive(Clone)]
    struct Author {
        id: i64,
        name: String,
    }

    impl Resource for Author {
        const TYPE: &'static str = "authors";

        fn resource_id(&self) -> String {
            self.id.to_string()
        }

        fn attributes(&self) -> Result<Map<String, Value>> {
            let mut map = Map::new();
            map.insert("name".to_string(), self.name.clone().into());
            Ok(map)
        }

        fn relationships(&self) -> Vec<NamedRelationship<'_>> {
            Vec::new()
        }
    }

    struct Article {
        id: i64,
        title: String,
        author: ToOne<Author>,
        reviewers: ToMany<Author>,
    }

    impl Resource for Article {
        const TYPE: &'static str = "articles";

        fn resource_id(&self) -> String {
            self.id.to_string()
        }

        fn attributes(&self) -> Result<Map<String, Value>> {
            let mut map = Map::new();
            map.insert("title".to_string(), self.title.clone().into());
            Ok(map)
        }

        fn relationships(&self) -> Vec<NamedRelationship<'_>> {
            vec![
                NamedRelationship {
                    name: "author",
                    relation: Relationship::ToOne(Some(self.author.as_target())),
                },
                NamedRelationship {
                    name: "reviewers",
                    relation: Relationship::ToMany(self.reviewers.targets()),
                },
            ]
        }
    }

    fn article() -> Article {
        Article {
            id: 1,
            title: "Hello".to_string(),
            author: ToOne::loaded(Author {
                id: 7,
                name: "Ada".to_string(),
            }),
            reviewers: ToMany::new(vec![
                ToOne::loaded(Author {
                    id: 7,
                    name: "Ada".to_string(),
                }),
                ToOne::loaded(Author {
                    id: 9,
                    name: "Grace".to_string(),
                }),
            ]),
        }
    }

    #[test]
    fn emitted_tracks_sightings_and_path_deltas() {
        let mut emitted = Emitted::default();
        assert!(emitted.mark("tags", "1".to_string()));
        assert!(!emitted.mark("tags", "1".to_string()));
        assert!(emitted.mark("tags", "2".to_string()));
        let books = vec!["books".to_string()];
        let awards = vec!["awards".to_string()];
        assert_eq!(
            emitted.add_paths("tags", "1".to_string(), std::slice::from_ref(&books)),
            vec![books.clone()]
        );
        assert!(emitted
            .add_paths("tags", "1".to_string(), std::slice::from_ref(&books))
            .is_empty());
        assert_eq!(
            emitted.add_paths("tags", "1".to_string(), std::slice::from_ref(&awards)),
            vec![awards]
        );
    }

    /// Reads a response body as JSON.
    async fn body_json(response: lumos_core::Response) -> Value {
        let bytes = lumos_core::axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn single_renders_linkage_and_deduped_included() {
        let query = ApiQuery::parse(
            &[("include".to_string(), "author,reviewers".to_string())],
            "/articles",
        )
        .unwrap();
        let response = single(&article(), &query).unwrap();
        assert_eq!(
            response.headers()["content-type"],
            "application/vnd.api+json"
        );
        assert_eq!(response.headers()["vary"], "Accept");
        let json = body_json(response).await;
        assert_eq!(json["data"]["type"], "articles");
        assert_eq!(json["data"]["relationships"]["author"]["data"]["id"], "7");
        // Author 7 appears once despite two include paths reaching them.
        assert_eq!(json["included"].as_array().unwrap().len(), 2);
        assert_eq!(json["included"][0]["attributes"]["name"], "Ada");
    }

    #[tokio::test]
    async fn fieldsets_filter_attributes_and_relationships() {
        let query = ApiQuery::parse(
            &[("fields[articles]".to_string(), "title".to_string())],
            "/articles",
        )
        .unwrap();
        let json = body_json(single(&article(), &query).unwrap()).await;
        assert!(json["data"]["attributes"].get("title").is_some());
        assert!(json["data"].get("relationships").is_none());

        let bad = ApiQuery::parse(
            &[("fields[articles]".to_string(), "nope".to_string())],
            "/a",
        )
        .unwrap();
        assert!(single(&article(), &bad).is_err());
    }

    #[tokio::test]
    async fn unknown_or_unloaded_includes_are_400s() {
        let unknown =
            ApiQuery::parse(&[("include".to_string(), "nope".to_string())], "/a").unwrap();
        let error = single(&article(), &unknown).unwrap_err();
        assert_eq!(error.0.status_code(), lumos_core::StatusCode::BAD_REQUEST);

        let bare = Article {
            id: 1,
            title: "t".to_string(),
            author: ToOne::<Author>::id("7"),
            reviewers: ToMany::default(),
        };
        let unloaded =
            ApiQuery::parse(&[("include".to_string(), "author".to_string())], "/a").unwrap();
        assert!(single(&bare, &unloaded).is_err());
    }

    #[tokio::test]
    async fn deep_paths_and_cycles_terminate() {
        struct Node {
            id: i64,
            peers: ToMany<Node>,
        }
        impl Resource for Node {
            const TYPE: &'static str = "nodes";
            fn resource_id(&self) -> String {
                self.id.to_string()
            }
            fn attributes(&self) -> Result<Map<String, Value>> {
                Ok(Map::new())
            }
            fn relationships(&self) -> Vec<NamedRelationship<'_>> {
                vec![NamedRelationship {
                    name: "peers",
                    relation: Relationship::ToMany(self.peers.targets()),
                }]
            }
        }

        // A cycle: 1 → 2 → 1. Each renders once; resolution terminates.
        let two = Node {
            id: 2,
            peers: ToMany::new(vec![ToOne::loaded(Node {
                id: 1,
                peers: ToMany::default(),
            })]),
        };
        let one = Node {
            id: 1,
            peers: ToMany::new(vec![ToOne::loaded(two)]),
        };
        let query = ApiQuery::parse(
            &[("include".to_string(), "peers.peers".to_string())],
            "/nodes",
        )
        .unwrap();
        let json = body_json(single(&one, &query).unwrap()).await;
        let ids: Vec<&str> = json["included"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"1") && ids.contains(&"2"));
    }

    #[tokio::test]
    async fn collections_page_link_and_201() {
        let query = ApiQuery::parse(&[], "/articles").unwrap();
        let items = vec![article()];
        let json = body_json(collection(&items, 30, &query).unwrap()).await;
        assert_eq!(json["data"].as_array().unwrap().len(), 1);
        assert!(json["links"]["last"]
            .as_str()
            .unwrap()
            .contains("page[number]=2"));

        let created_response = created(&article(), "/articles/1", &query).unwrap();
        assert_eq!(created_response.status(), lumos_core::StatusCode::CREATED);
        assert_eq!(created_response.headers()["location"], "/articles/1");

        let bad_location = created(&article(), "not a\nheader", &query);
        assert!(bad_location.is_err());
    }
}
