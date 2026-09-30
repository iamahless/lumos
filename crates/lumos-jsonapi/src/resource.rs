//! API resources: the seam between domain types and JSON:API documents.
//!
//! A [`Resource`] exposes a JSON:API type name, a string id, serializable
//! attributes, and typed relationships. Implement it by hand (the escape
//! hatch — full control, no codegen) or derive it with
//! `JsonApiResource` (fields become attributes unless marked otherwise).
//! Models are never serialized directly: the resource decides what clients
//! see.
//!
//! Relationships use [`ToOne`] / [`ToMany`]: linkage (type + id) always,
//! the loaded resource only when the app provides it. `include` renders
//! loaded targets into the compound `included` array; unloaded targets
//! render linkage only.
//!
//! [`Resource`] carries an associated `TYPE` const, which cannot go
//! behind `dyn`; renderers handle mixed resources as [`DynResource`]
//! instead (blanket-implemented for every `Resource`, so nobody writes it
//! by hand).
//!
//! # Examples
//!
//! ```rust
//! use lumos_jsonapi::{AttributeMap, NamedRelationship, Resource};
//!
//! struct User {
//!     id: i64,
//!     name: String,
//! }
//!
//! impl Resource for User {
//!     const TYPE: &'static str = "users";
//!
//!     fn resource_id(&self) -> String {
//!         self.id.to_string()
//!     }
//!
//!     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
//!         let mut map = AttributeMap::new();
//!         map.insert(
//!             "name".to_string(),
//!             lumos_jsonapi::to_attribute("name", &self.name)?,
//!         );
//!         Ok(map)
//!     }
//!
//!     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
//!         Vec::new()
//!     }
//! }
//!
//! let user = User { id: 1, name: "Ada".to_string() };
//! assert_eq!(user.resource_id(), "1");
//! ```

use serde::Serialize;

use serde_json::Value;

use lumos_core::{AppError, Result};

/// Attribute map: field name to JSON value.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::AttributeMap;
///
/// let mut map = AttributeMap::new();
/// map.insert("name".to_string(), "Ada".into());
/// assert_eq!(map["name"], "Ada");
/// ```
pub type AttributeMap = serde_json::Map<String, Value>;

/// A JSON:API resource: type, id, attributes, relationships.
///
/// `TYPE` is the static JSON:API type (`"users"`); [`Resource::resource_type`]
/// exposes it through `&dyn Resource` for mixed rendering. Attributes
/// serialize field-by-field so one bad field fails loudly with its name.
/// Relationships return linkage plus whatever the app loaded — renderers
/// never fetch.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{AttributeMap, NamedRelationship, Resource};
///
/// struct Ping;
/// impl Resource for Ping {
///     const TYPE: &'static str = "pings";
///     fn resource_id(&self) -> String {
///         "1".to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         Ok(AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// assert_eq!(Ping::TYPE, "pings");
/// assert_eq!(Ping.resource_id(), "1");
/// ```
///
/// See the [module documentation](self) for a fuller manual implementation.
pub trait Resource {
    /// JSON:API type name, e.g. `"users"`. Static contexts (`JsonApiBody`
    /// type checks, linkage building) use `T::TYPE` directly.
    const TYPE: &'static str;

    /// Instance view of [`Resource::TYPE`].
    fn resource_type(&self) -> &'static str {
        Self::TYPE
    }

    /// JSON:API id: always a string, even for integer keys.
    fn resource_id(&self) -> String;

    /// Serializable attributes (never `type`/`id` — those are structural).
    /// A field that cannot serialize fails the render as a 500 naming it.
    fn attributes(&self) -> Result<AttributeMap>;

    /// Relationships as (name, to-one/to-many) pairs. Linkage always;
    /// loaded targets only when the app provides them.
    fn relationships(&self) -> Vec<NamedRelationship<'_>>;
}

/// Type-erased [`Resource`] for mixed rendering (`included` arrays hold many
/// types). Blanket-implemented for every `Resource`: behaves as `Resource`
/// minus the `TYPE` const, which is what keeps `Resource` itself off `dyn`.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{AttributeMap, DynResource, NamedRelationship, Resource};
///
/// struct Tag {
///     id: i64,
/// }
/// impl Resource for Tag {
///     const TYPE: &'static str = "tags";
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
/// let tag = Tag { id: 1 };
/// let erased: &dyn DynResource = &tag;
/// assert_eq!(erased.resource_type(), "tags");
/// ```
pub trait DynResource {
    /// JSON:API type name (forwards to `T::TYPE`).
    fn resource_type(&self) -> &'static str;

    /// JSON:API id: always a string, even for integer keys.
    fn resource_id(&self) -> String;

    /// Serializable attributes.
    fn attributes(&self) -> Result<AttributeMap>;

    /// Relationships as (name, to-one/to-many) pairs.
    fn relationships(&self) -> Vec<NamedRelationship<'_>>;
}

impl<T: Resource> DynResource for T {
    fn resource_type(&self) -> &'static str {
        T::TYPE
    }

    fn resource_id(&self) -> String {
        Resource::resource_id(self)
    }

    fn attributes(&self) -> Result<AttributeMap> {
        Resource::attributes(self)
    }

    fn relationships(&self) -> Vec<NamedRelationship<'_>> {
        Resource::relationships(self)
    }
}

/// A named relationship on a resource: the object key plus its linkage.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{NamedRelationship, Relationship};
///
/// let named = NamedRelationship { name: "author", relation: Relationship::ToOne(None) };
/// assert_eq!(named.name, "author");
/// ```
#[derive(Debug)]
pub struct NamedRelationship<'a> {
    /// Object key under `relationships`, e.g. `"author"`.
    pub name: &'static str,
    /// To-one or to-many linkage (plus loaded targets, if any).
    pub relation: Relationship<'a>,
}

/// To-one or to-many linkage for one relationship.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::Relationship;
///
/// assert!(matches!(Relationship::ToOne(None), Relationship::ToOne(None)));
/// assert!(matches!(Relationship::ToMany(vec![]), Relationship::ToMany(_)));
/// ```
#[derive(Debug)]
pub enum Relationship<'a> {
    /// Zero or one target (`data: null` when [`None`]).
    ToOne(Option<RelationTarget<'a>>),
    /// Zero or more targets.
    ToMany(Vec<RelationTarget<'a>>),
}

/// One linkage entry: type + id always, loaded resource when provided.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{RelationTarget, Resource, ToOne};
///
/// struct User {
///     id: i64,
/// }
///
/// impl Resource for User {
///     const TYPE: &'static str = "users";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
///         Ok(lumos_jsonapi::AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let author = ToOne::loaded(User { id: 7 });
/// let target = author.as_target();
/// assert_eq!(target.resource_type, "users");
/// assert_eq!(target.id, "7");
/// assert!(target.loaded.is_some());
/// ```
pub struct RelationTarget<'a> {
    /// JSON:API type of the target.
    pub resource_type: &'static str,
    /// JSON:API id of the target.
    pub id: String,
    /// Loaded target for `included` rendering, when the app provided it.
    pub loaded: Option<&'a dyn DynResource>,
}

impl std::fmt::Debug for RelationTarget<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelationTarget")
            .field("resource_type", &self.resource_type)
            .field("id", &self.id)
            .field("loaded", &self.loaded.is_some())
            .finish()
    }
}

/// To-one relationship value: linkage always, loaded resource optionally.
///
/// [`ToOne::id`] links an unloaded target (linkage only);
/// [`ToOne::loaded`] links a target the app already fetched (eligible for
/// `included`). Nullable relations use `Option<ToOne<T>>`.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{Resource, ToOne};
///
/// struct User {
///     id: i64,
/// }
///
/// impl Resource for User {
///     const TYPE: &'static str = "users";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
///         Ok(lumos_jsonapi::AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let linked = ToOne::<User>::id("7");
/// assert_eq!(linked.as_target().id, "7");
/// assert!(linked.as_target().loaded.is_none());
///
/// let loaded = ToOne::loaded(User { id: 7 });
/// assert!(loaded.as_target().loaded.is_some());
/// ```
#[derive(Debug, Clone)]
pub struct ToOne<T: Resource> {
    id: String,
    loaded: Option<T>,
}

impl<T: Resource> ToOne<T> {
    /// Links target `id` without loading it (linkage only).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::{Resource, ToOne};
    ///
    /// struct Tag;
    /// impl Resource for Tag {
    ///     const TYPE: &'static str = "tags";
    ///     fn resource_id(&self) -> String {
    ///         String::new()
    ///     }
    ///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
    ///         Ok(lumos_jsonapi::AttributeMap::new())
    ///     }
    ///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let tag = ToOne::<Tag>::id("4");
    /// assert_eq!(tag.as_target().resource_type, "tags");
    /// ```
    pub fn id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            loaded: None,
        }
    }

    /// Links an already-fetched target (eligible for `included`).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::{Resource, ToOne};
    ///
    /// struct Tag {
    ///     id: i64,
    /// }
    /// impl Resource for Tag {
    ///     const TYPE: &'static str = "tags";
    ///     fn resource_id(&self) -> String {
    ///         self.id.to_string()
    ///     }
    ///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
    ///         Ok(lumos_jsonapi::AttributeMap::new())
    ///     }
    ///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let tag = ToOne::loaded(Tag { id: 4 });
    /// assert_eq!(tag.as_target().id, "4");
    /// ```
    pub fn loaded(resource: T) -> Self {
        let id = resource.resource_id();
        Self {
            id,
            loaded: Some(resource),
        }
    }

    /// Borrows this link as a renderable target.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::{Resource, ToOne};
    ///
    /// struct Tag;
    /// impl Resource for Tag {
    ///     const TYPE: &'static str = "tags";
    ///     fn resource_id(&self) -> String {
    ///         "9".to_string()
    ///     }
    ///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
    ///         Ok(lumos_jsonapi::AttributeMap::new())
    ///     }
    ///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let tag = ToOne::loaded(Tag);
    /// let target = tag.as_target();
    /// assert_eq!((target.resource_type, target.id.as_str()), ("tags", "9"));
    /// ```
    pub fn as_target(&self) -> RelationTarget<'_> {
        RelationTarget {
            resource_type: T::TYPE,
            id: self.id.clone(),
            loaded: self
                .loaded
                .as_ref()
                .map(|resource| resource as &dyn DynResource),
        }
    }
}

/// To-many relationship value: a list of [`ToOne`] links.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{Resource, ToMany, ToOne};
///
/// struct Tag {
///     id: i64,
/// }
/// impl Resource for Tag {
///     const TYPE: &'static str = "tags";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
///         Ok(lumos_jsonapi::AttributeMap::new())
///     }
///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let tags = ToMany::new(vec![ToOne::loaded(Tag { id: 1 }), ToOne::<Tag>::id("2")]);
/// assert_eq!(tags.targets().len(), 2);
/// ```
#[derive(Debug, Clone)]
pub struct ToMany<T: Resource> {
    items: Vec<ToOne<T>>,
}

impl<T: Resource> Default for ToMany<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T: Resource> ToMany<T> {
    /// Builds a to-many value from links ([`ToMany::default`] is empty).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::{Resource, ToMany, ToOne};
    ///
    /// struct Tag;
    /// impl Resource for Tag {
    ///     const TYPE: &'static str = "tags";
    ///     fn resource_id(&self) -> String {
    ///         String::new()
    ///     }
    ///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
    ///         Ok(lumos_jsonapi::AttributeMap::new())
    ///     }
    ///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let tags = ToMany::new(vec![ToOne::<Tag>::id("1")]);
    /// assert_eq!(tags.targets().len(), 1);
    /// assert!(ToMany::<Tag>::default().targets().is_empty());
    /// ```
    pub fn new(items: Vec<ToOne<T>>) -> Self {
        Self { items }
    }

    /// Borrows every link as a renderable target.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::{Resource, ToMany, ToOne};
    ///
    /// struct Tag;
    /// impl Resource for Tag {
    ///     const TYPE: &'static str = "tags";
    ///     fn resource_id(&self) -> String {
    ///         "3".to_string()
    ///     }
    ///     fn attributes(&self) -> lumos_core::Result<lumos_jsonapi::AttributeMap> {
    ///         Ok(lumos_jsonapi::AttributeMap::new())
    ///     }
    ///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let tags = ToMany::new(vec![ToOne::loaded(Tag)]);
    /// assert_eq!(tags.targets()[0].id, "3");
    /// ```
    pub fn targets(&self) -> Vec<RelationTarget<'_>> {
        self.items.iter().map(ToOne::as_target).collect()
    }
}

/// Serializes one attribute field, failing loudly with its name.
///
/// Used by hand-written [`Resource`] impls and generated derive code alike.
/// Only exotic values fail (e.g. non-string-keyed maps); the 500 message
/// names the field so the fix is obvious.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::to_attribute;
///
/// assert_eq!(to_attribute("name", &"Ada").unwrap(), "Ada");
/// ```
pub fn to_attribute<T: Serialize>(field: &str, value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| {
        AppError::internal(format!("attribute {field:?} is not serializable: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tag {
        id: i64,
    }

    impl Resource for Tag {
        const TYPE: &'static str = "tags";

        fn resource_id(&self) -> String {
            self.id.to_string()
        }

        fn attributes(&self) -> Result<AttributeMap> {
            Ok(AttributeMap::new())
        }

        fn relationships(&self) -> Vec<NamedRelationship<'_>> {
            Vec::new()
        }
    }

    #[test]
    fn dyn_resource_exposes_type() {
        let tag = Tag { id: 1 };
        let erased: &dyn DynResource = &tag;
        assert_eq!(erased.resource_type(), "tags");
        assert_eq!(erased.resource_id(), "1");
    }

    #[test]
    fn exotic_values_fail_with_field_name() {
        struct Unserializable;
        impl serde::Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(
                &self,
                _: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("boom"))
            }
        }
        let error = to_attribute("weird", &Unserializable).unwrap_err();
        assert!(error.to_string().contains("weird"), "{error}");
    }
}
