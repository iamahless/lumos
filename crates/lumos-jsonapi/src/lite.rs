//! Simplified JSON:API rendering (the `jsonapi-lite` flag).
//!
//! Same [`Resource`] trait, same negotiation, same sort/filter/page — but
//! documents stay flat: full attributes, linkage-only relationships, no
//! compound `included`, no sparse fieldsets. Requesting `include` or
//! `fields` is a 400 naming the flag (`jsonapi-lite` trades those features
//! for a renderer with no graph walk). Output remains spec-valid JSON:API,
//! so lite clients upgrade to full without changing parsers.
//!
//! Signatures mirror the full renderer one-to-one ([`single`],
//! [`collection`], [`collection_memory`], [`created`]) so handlers switch
//! flags without rewriting calls.
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

use serde_json::{Map, Value};

use lumos_core::{AppError, Result};

use crate::negotiate::{api_response, ApiResult};
use crate::query::{memory_page, ApiQuery};
#[cfg(test)]
use crate::resource::AttributeMap;
use crate::resource::{Relationship, Resource};

/// Renders one resource as a flat `200` document.
///
/// `include` / `fields` are 400s (unsupported in lite); collection
/// parameters are ignored for single resources.
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
    reject_compound(query)?;
    let mut document = Map::new();
    document.insert("data".to_string(), render_object(resource)?);
    Ok(api_response(
        Value::Object(document),
        lumos_core::StatusCode::OK,
    ))
}

/// Renders the current page of a collection as a flat `200` document.
///
/// `items` is exactly this page; `total` is the full count for links.
/// See [`collection_memory`] for the in-memory pipeline.
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
    reject_compound(query)?;
    render_collection(items.iter().collect(), total, query)
}

/// Filters, sorts, and paginates loaded items, then renders the page.
///
/// Same document as [`collection`], with `total` set to the filtered count.
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
    reject_compound(query)?;
    let (indices, total) = memory_page(items, query)?;
    render_collection(
        indices.iter().map(|index| &items[*index]).collect(),
        total,
        query,
    )
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

/// Rejects the compound-document parameters lite does not implement.
fn reject_compound(query: &ApiQuery) -> Result<()> {
    if !query.include.is_empty() {
        return Err(AppError::bad_request(
            "include is not supported by jsonapi-lite; enable the jsonapi flag for compound documents"
                .to_string(),
        ));
    }
    if !query.fields.is_empty() {
        return Err(AppError::bad_request(
            "fields is not supported by jsonapi-lite; enable the jsonapi flag for sparse fieldsets"
                .to_string(),
        ));
    }
    Ok(())
}

/// Renders page items plus links into the final response.
fn render_collection<T: Resource + ?Sized>(
    page: Vec<&T>,
    total: u64,
    query: &ApiQuery,
) -> ApiResult<lumos_core::Response> {
    let mut data = Vec::with_capacity(page.len());
    for item in page {
        data.push(render_object(item)?);
    }
    let links = query.page_links(total);
    let mut document = Map::new();
    document.insert("data".to_string(), Value::Array(data));
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

/// Renders one flat resource object: type, id, full attributes, and
/// linkage-only relationships (never `included`).
fn render_object<T: Resource + ?Sized>(resource: &T) -> Result<Value> {
    let mut object = Map::new();
    object.insert(
        "type".to_string(),
        Value::String(resource.resource_type().to_string()),
    );
    object.insert("id".to_string(), Value::String(resource.resource_id()));
    object.insert(
        "attributes".to_string(),
        Value::Object(resource.attributes()?),
    );
    let mut rendered = Map::new();
    for named in resource.relationships() {
        let linkage = match &named.relation {
            Relationship::ToOne(target) => {
                target.as_ref().map(linkage_object).unwrap_or(Value::Null)
            }
            Relationship::ToMany(entries) => {
                Value::Array(entries.iter().map(linkage_object).collect())
            }
        };
        let mut relation = Map::new();
        relation.insert("data".to_string(), linkage);
        rendered.insert(named.name.to_string(), Value::Object(relation));
    }
    if !rendered.is_empty() {
        object.insert("relationships".to_string(), Value::Object(rendered));
    }
    Ok(Value::Object(object))
}

/// Builds one linkage object (`{type, id}`).
fn linkage_object(target: &crate::RelationTarget<'_>) -> Value {
    serde_json::json!({"type": target.resource_type, "id": target.id})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::NamedRelationship;

    struct Article {
        id: i64,
        title: String,
        author_id: String,
    }

    impl Resource for Article {
        const TYPE: &'static str = "articles";

        fn resource_id(&self) -> String {
            self.id.to_string()
        }

        fn attributes(&self) -> Result<AttributeMap> {
            let mut map = AttributeMap::new();
            map.insert("title".to_string(), self.title.clone().into());
            Ok(map)
        }

        fn relationships(&self) -> Vec<NamedRelationship<'_>> {
            vec![NamedRelationship {
                name: "author",
                relation: Relationship::ToOne(Some(crate::resource::RelationTarget {
                    resource_type: "authors",
                    id: self.author_id.clone(),
                    loaded: None,
                })),
            }]
        }
    }

    fn article() -> Article {
        Article {
            id: 1,
            title: "Hello".to_string(),
            author_id: "7".to_string(),
        }
    }

    async fn body_json(response: lumos_core::Response) -> Value {
        let bytes = lumos_core::axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn flat_render_keeps_linkage_without_included() {
        let query = ApiQuery::parse(&[], "/articles").unwrap();
        let json = body_json(single(&article(), &query).unwrap()).await;
        assert_eq!(json["data"]["attributes"]["title"], "Hello");
        assert_eq!(json["data"]["relationships"]["author"]["data"]["id"], "7");
        assert!(json.get("included").is_none());
    }

    #[tokio::test]
    async fn compound_parameters_are_400s() {
        for (name, value) in [("include", "author"), ("fields[articles]", "title")] {
            let query =
                ApiQuery::parse(&[(name.to_string(), value.to_string())], "/articles").unwrap();
            let error = single(&article(), &query).unwrap_err();
            assert_eq!(error.0.status_code(), lumos_core::StatusCode::BAD_REQUEST);
            assert!(error.0.to_string().contains("jsonapi-lite"), "{}", error.0);
        }
    }

    #[tokio::test]
    async fn memory_and_links_work_like_full() {
        let query = ApiQuery::parse(&[("sort".to_string(), "-title".to_string())], "/a").unwrap();
        let items = vec![article(), article()];
        let json = body_json(collection_memory(&items, &query).unwrap()).await;
        assert_eq!(json["data"].as_array().unwrap().len(), 2);
        assert!(json["links"]["self"]
            .as_str()
            .unwrap()
            .contains("sort=-title"));
    }
}
