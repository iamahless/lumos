//! Relationships: [`HasMany`], [`BelongsTo`], and eager loading.
//!
//! Relation fields hold *loaded* data only. Lazy queries come from generated
//! per-relation methods taking a target — `user.posts(&db)` returns a full
//! [`Query`](crate::Query) pre-filtered to the parent, so every builder
//! method (`.where_eq`, `.paginate`, [`.on(&tx)`](crate::Query::on)) works
//! unchanged. `to-many` relations also get `create_posts`-style methods that
//! stamp the foreign key automatically.
//!
//! Eager loading (`.with(["posts", "team"])`) runs one batched query per
//! relation level and distributes rows to parents by comparing encoded keys,
//! so any key types work uniformly. Nested paths recurse before distribution.

use crate::db::Target;
use crate::model::Model;
use crate::value::BindValue;
use crate::{Error, Result};

/// To-many relation handle (model field type).
///
/// Starts unloaded; [`get`](HasMany::get) borrows the loaded rows.
/// Construct loaded instances with [`loaded`](HasMany::loaded) (used by
/// eager loading and handy in tests).
///
/// # Examples
///
/// ```rust
/// use rusticate::HasMany;
///
/// let rel: HasMany<String> = HasMany::loaded(vec!["a".to_string()]);
/// assert_eq!(rel.get().unwrap(), &vec!["a".to_string()]);
/// assert!(HasMany::<String>::default().get().is_err());
/// ```
#[derive(Debug, Clone)]
pub struct HasMany<T> {
    loaded: Option<Vec<T>>,
}

/// Manual `Default` without a `T: Default` bound (a derive would demand one,
/// and related models are never required to be `Default`).
impl<T> Default for HasMany<T> {
    fn default() -> Self {
        Self { loaded: None }
    }
}

impl<T> HasMany<T> {
    /// Builds a loaded handle (eager loading and manual assembly).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::HasMany;
    ///
    /// let rel = HasMany::loaded(vec![1, 2]);
    /// assert!(rel.is_loaded());
    /// ```
    pub fn loaded(items: Vec<T>) -> Self {
        Self {
            loaded: Some(items),
        }
    }

    /// Borrows the loaded rows, or [`Error::RelationNotLoaded`] naming the
    /// target type and the `with()` call that would load it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::HasMany;
    ///
    /// let rel = HasMany::loaded(vec!["a"]);
    /// assert_eq!(rel.get().unwrap(), &vec!["a"]);
    /// ```
    pub fn get(&self) -> Result<&Vec<T>> {
        self.loaded.as_ref().ok_or_else(|| {
            Error::RelationNotLoaded(format!(
                "relation to {} was not loaded; eager-load it with `.with([...])`",
                std::any::type_name::<T>()
            ))
        })
    }

    /// Returns `true` once rows are loaded (possibly zero rows).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::HasMany;
    ///
    /// assert!(!HasMany::<u8>::default().is_loaded());
    /// ```
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }
}

/// To-one (inverse) relation handle (model field type).
///
/// [`get`](BelongsTo::get) returns `Ok(Some)` / `Ok(None)` once loaded
/// (absent parent vs. present), or [`Error::RelationNotLoaded`] before that.
///
/// # Examples
///
/// ```rust
/// use rusticate::BelongsTo;
///
/// let rel: BelongsTo<String> = BelongsTo::loaded(Some("t".to_string()));
/// assert_eq!(rel.get().unwrap(), &Some("t".to_string()));
/// ```
#[derive(Debug, Clone)]
pub struct BelongsTo<T> {
    loaded: Option<Option<T>>,
}

/// Manual `Default` without a `T: Default` bound (see [`HasMany`]).
impl<T> Default for BelongsTo<T> {
    fn default() -> Self {
        Self { loaded: None }
    }
}

impl<T> BelongsTo<T> {
    /// Builds a loaded handle (`None` = loaded, no related row).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::BelongsTo;
    ///
    /// let rel: BelongsTo<String> = BelongsTo::loaded(None);
    /// assert!(rel.is_loaded());
    /// ```
    pub fn loaded(item: Option<T>) -> Self {
        Self { loaded: Some(item) }
    }

    /// Borrows the loaded row: `Ok(Some)` present, `Ok(None)` absent,
    /// `Err` when never loaded.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::BelongsTo;
    ///
    /// let rel = BelongsTo::loaded(Some("t"));
    /// assert_eq!(rel.get().unwrap(), &Some("t"));
    /// ```
    pub fn get(&self) -> Result<&Option<T>> {
        self.loaded.as_ref().ok_or_else(|| {
            Error::RelationNotLoaded(format!(
                "relation to {} was not loaded; eager-load it with `.with([...])`",
                std::any::type_name::<T>()
            ))
        })
    }

    /// Returns `true` once loaded (present or absent).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::BelongsTo;
    ///
    /// assert!(!BelongsTo::<u8>::default().is_loaded());
    /// ```
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }
}

/// Fetches children whose `column` matches any of `keys` (one `WHERE IN`
/// query). Empty keys match nothing via the builder's `0 = 1` rule, so
/// callers need no special case.
///
/// Hidden: generated `load_relation` implementations call this; applications
/// use `.with([...])` instead.
#[doc(hidden)]
pub async fn fetch_many_by_keys<C: Model>(
    target: &Target,
    column: &str,
    keys: Vec<BindValue>,
) -> Result<Vec<C>> {
    C::query(target.clone()).where_in(column, keys).get().await
}
