//! Parsed JSON:API query strings: include, sparse fieldsets, sort, filter, page.
//!
//! [`ApiQuery`] parses decoded query pairs (order and duplicates preserved)
//! into structured values plus the request path for link building. Unknown
//! top-level parameters are ignored so apps can mix their own; malformed
//! JSON:API parameters are 400s naming the culprit.
//!
//! [`ApiQuery`] is also an extractor: handlers take it directly and get a
//! parsed query or a 400. [`memory_page`] runs the filter → sort → paginate
//! pipeline over in-memory items for datasets the app already loaded;
//! SQL-backed apps map [`ApiQuery::sort`] / [`ApiQuery::filters`] onto
//! their queries instead (see the [`memory_page`] docs for the mapping).
//!
//! # Examples
//!
//! ```rust
//! use lumos_jsonapi::ApiQuery;
//!
//! let query = ApiQuery::parse(
//!     &[
//!         ("include".to_string(), "author".to_string()),
//!         ("sort".to_string(), "-created_at".to_string()),
//!         ("page[number]".to_string(), "2".to_string()),
//!     ],
//!     "/articles",
//! )
//! .unwrap();
//! assert_eq!(query.include, vec![vec!["author".to_string()]]);
//! assert!(query.sort[0].descending);
//! assert_eq!(query.page.number, 2);
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;

use lumos_core::axum::extract::{FromRequestParts, Query};
use lumos_core::axum::http::request::Parts;
use serde_json::Value;

use lumos_core::{AppError, Result};

#[cfg(test)]
use crate::resource::AttributeMap;
use crate::resource::Resource;

/// One `sort` entry: field plus direction (`-` prefix means descending).
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::SortField;
///
/// let entry = SortField { field: "title".to_string(), descending: true };
/// assert!(entry.descending);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortField {
    /// Attribute name to sort by.
    pub field: String,
    /// True when the entry started with `-`.
    pub descending: bool,
}

/// One `filter[field]=value` entry: equality against the raw string.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::Filter;
///
/// let filter = Filter { field: "status".to_string(), value: "open".to_string() };
/// assert_eq!(filter.value, "open");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Attribute name to compare.
    pub field: String,
    /// Raw (decoded) value; equality is type-aware (see [`memory_page`]).
    pub value: String,
}

/// `page[number]` / `page[size]` after validation.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::Page;
///
/// let page = Page { number: 2, size: 10 };
/// assert_eq!(page.number, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// 1-based page number (default 1).
    pub number: u64,
    /// Items per page (default [`DEFAULT_PAGE_SIZE`]).
    pub size: u64,
}

/// Default page size when `page[size]` is absent.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{ApiQuery, DEFAULT_PAGE_SIZE};
///
/// let query = ApiQuery::parse(&[], "/t").unwrap();
/// assert_eq!(query.page.size, DEFAULT_PAGE_SIZE);
/// ```
pub const DEFAULT_PAGE_SIZE: u64 = 20;

/// Maximum `page[size]`; larger values are 400s, never silently clamped.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::MAX_PAGE_SIZE;
///
/// assert_eq!(MAX_PAGE_SIZE, 100);
/// ```
pub const MAX_PAGE_SIZE: u64 = 100;

/// Parsed JSON:API query plus the request path for link building.
///
/// Constructed with [`ApiQuery::parse`] (pure, unit-testable) or extracted
/// directly in handlers. Duplicate parameters merge, except `page[number]`
/// / `page[size]` where the last value wins.
///
/// Extraction also enforces `Accept` (406 unless JSON:API is allowed), so
/// taking `ApiQuery` is what opts a read route into negotiation. Routes
/// that render without a query call
/// [`check_accept`](crate::negotiate::check_accept) on the headers
/// themselves.
///
/// # Examples
///
/// ```rust,no_run
/// use lumos_jsonapi::ApiQuery;
///
/// async fn index(query: ApiQuery) -> String {
///     format!("page {}", query.page.number)
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ApiQuery {
    /// `include` paths, each a list of relationship names (`a.b` →
    /// `["a", "b"]`). Empty segments are 400s.
    pub include: Vec<Vec<String>>,
    /// `fields[type]` lists by type. An empty list is meaningful (no
    /// attributes); a missing entry means all attributes.
    pub fields: HashMap<String, Vec<String>>,
    /// `sort` entries in order.
    pub sort: Vec<SortField>,
    /// `filter[field]` equalities (AND-combined).
    pub filters: Vec<Filter>,
    /// Validated pagination window.
    pub page: Page,
    /// Request path (`/articles`), prefixing generated page links.
    pub base_path: String,
}

impl ApiQuery {
    /// Parses decoded query pairs (`Vec<(name, value)>`, order preserved).
    ///
    /// Rules: `include`/`sort` split on `,` (empty entries are 400s);
    /// `fields[type]` / `filter[field]` require the bracketed name;
    /// `page[number]` / `page[size]` must be integers ≥ 1 with size ≤
    /// [`MAX_PAGE_SIZE`]; unknown `page[*]` and bare `fields`/`filter`
    /// are 400s; every other parameter is ignored.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::ApiQuery;
    ///
    /// let query = ApiQuery::parse(
    ///     &[("filter[status]".to_string(), "open".to_string())],
    ///     "/tickets",
    /// )
    /// .unwrap();
    /// assert_eq!(query.filters.len(), 1);
    /// ```
    pub fn parse(pairs: &[(String, String)], base_path: impl Into<String>) -> Result<Self> {
        let mut query = Self {
            include: Vec::new(),
            fields: HashMap::new(),
            sort: Vec::new(),
            filters: Vec::new(),
            page: Page {
                number: 1,
                size: DEFAULT_PAGE_SIZE,
            },
            base_path: base_path.into(),
        };
        for (name, value) in pairs {
            match name.as_str() {
                "include" => parse_include(value, &mut query.include)?,
                "sort" => parse_sort(value, &mut query.sort)?,
                "fields" | "filter" => {
                    return Err(AppError::bad_request(format!(
                        "malformed parameter {name:?}: expected {name}[name]=value"
                    )));
                }
                _ => {
                    if let Some(inner) = bracketed(name, "fields") {
                        query
                            .fields
                            .entry(inner)
                            .or_default()
                            .extend(split_list(value));
                    } else if let Some(inner) = bracketed(name, "filter") {
                        query.filters.push(Filter {
                            field: inner,
                            value: value.clone(),
                        });
                    } else if let Some(inner) = bracketed(name, "page") {
                        parse_page(&inner, value, &mut query.page)?;
                    } else if ["fields[", "filter[", "page["]
                        .iter()
                        .any(|family| name.starts_with(family))
                    {
                        return Err(AppError::bad_request(format!(
                            "malformed parameter {name:?}: expected family[name]=value"
                        )));
                    }
                    // Anything else is the app's own parameter: ignored.
                }
            }
        }
        Ok(query)
    }

    /// Builds self/first/last/prev/next links for `total` items.
    ///
    /// Links reuse the canonical query (same include/fields/sort/filter,
    /// same size, varying number). `prev` is absent on page one, `next`
    /// absent on the last page; an overrun page renders empty data with
    /// valid links rather than 400ing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lumos_jsonapi::ApiQuery;
    ///
    /// let query = ApiQuery::parse(&[], "/articles").unwrap();
    /// let links = query.page_links(45);
    /// assert!(links.self_link.contains("page[number]=1"));
    /// assert!(links.last.contains("page[number]=3"));
    /// assert!(links.prev.is_none());
    /// assert!(links.next.is_some());
    /// ```
    pub fn page_links(&self, total: u64) -> PageLinks {
        let pages = total.div_ceil(self.page.size).max(1);
        let last = pages;
        PageLinks {
            self_link: self.page_url(self.page.number),
            first: self.page_url(1),
            last: self.page_url(last),
            prev: (self.page.number > 1).then(|| self.page_url(self.page.number - 1)),
            next: (self.page.number < last).then(|| self.page_url(self.page.number + 1)),
        }
    }

    /// Canonical URL for one page number: base path plus the structured
    /// query (values percent-encoded, structure raw).
    fn page_url(&self, number: u64) -> String {
        let mut parts = Vec::new();
        if !self.include.is_empty() {
            let paths: Vec<String> = self.include.iter().map(|path| path.join(".")).collect();
            parts.push(format!("include={}", paths.join(",")));
        }
        let mut types: Vec<&String> = self.fields.keys().collect();
        types.sort();
        for resource_type in types {
            parts.push(format!(
                "fields[{}]={}",
                encode(resource_type),
                self.fields[resource_type]
                    .iter()
                    .map(|field| encode(field))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !self.sort.is_empty() {
            let entries: Vec<String> = self
                .sort
                .iter()
                .map(|entry| format!("{}{}", if entry.descending { "-" } else { "" }, entry.field))
                .collect();
            parts.push(format!("sort={}", entries.join(",")));
        }
        for filter in &self.filters {
            parts.push(format!(
                "filter[{}]={}",
                encode(&filter.field),
                encode(&filter.value)
            ));
        }
        parts.push(format!("page[number]={number}"));
        parts.push(format!("page[size]={}", self.page.size));
        format!("{}?{}", self.base_path, parts.join("&"))
    }
}

/// Pagination links for a collection document.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::ApiQuery;
///
/// let query = ApiQuery::parse(&[], "/t").unwrap();
/// let links = query.page_links(0);
/// assert!(links.self_link.contains("/t?"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLinks {
    /// Link to the current page.
    pub self_link: String,
    /// Link to page one.
    pub first: String,
    /// Link to the last page.
    pub last: String,
    /// Link to the previous page ([`None`] on page one).
    pub prev: Option<String>,
    /// Link to the next page ([`None`] on the last page).
    pub next: Option<String>,
}

impl<S> FromRequestParts<S> for ApiQuery
where
    S: Send + Sync,
{
    type Rejection = crate::negotiate::JsonApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        crate::negotiate::check_accept(&parts.headers)?;
        let Query(pairs) = Query::<Vec<(String, String)>>::from_request_parts(parts, state)
            .await
            .map_err(|rejection| AppError::bad_request(rejection.to_string()))?;
        Self::parse(&pairs, parts.uri.path()).map_err(crate::negotiate::JsonApiError::from)
    }
}

/// Extracts the bracketed name from `family[name]`, rejecting malformed
/// shapes (`family[]`, `family[x]y`, bare `family` is handled earlier).
fn bracketed(name: &str, family: &str) -> Option<String> {
    let rest = name.strip_prefix(family)?.strip_prefix('[')?;
    let inner = rest.strip_suffix(']')?;
    if inner.is_empty() || inner.contains('[') || inner.contains(']') {
        return None;
    }
    Some(inner.to_string())
}

/// Splits a comma list, dropping empty entries (used where empties are
/// meaningful-absent rather than malformed: `fields` only).
fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_include(value: &str, into: &mut Vec<Vec<String>>) -> Result<()> {
    for path in value.split(',') {
        let segments: Vec<String> = path.split('.').map(str::to_string).collect();
        if segments.iter().any(String::is_empty) {
            return Err(AppError::bad_request(format!(
                "malformed include path {path:?}: segments must not be empty"
            )));
        }
        into.push(segments);
    }
    Ok(())
}

fn parse_sort(value: &str, into: &mut Vec<SortField>) -> Result<()> {
    for entry in value.split(',') {
        let (field, descending) = entry
            .strip_prefix('-')
            .map_or((entry, false), |rest| (rest, true));
        if field.is_empty() {
            return Err(AppError::bad_request(
                "malformed sort entry: field must not be empty",
            ));
        }
        into.push(SortField {
            field: field.to_string(),
            descending,
        });
    }
    Ok(())
}

fn parse_page(inner: &str, value: &str, page: &mut Page) -> Result<()> {
    let parsed: u64 = value
        .parse()
        .ok()
        .filter(|number| *number >= 1)
        .ok_or_else(|| {
            AppError::bad_request(format!("malformed page[{inner}]: expected an integer ≥ 1"))
        })?;
    match inner {
        "number" => page.number = parsed,
        "size" => {
            if parsed > MAX_PAGE_SIZE {
                return Err(AppError::bad_request(format!(
                    "malformed page[size]: expected 1–{MAX_PAGE_SIZE}"
                )));
            }
            page.size = parsed;
        }
        _ => {
            return Err(AppError::bad_request(format!(
                "unknown page parameter {inner:?}: expected number or size"
            )));
        }
    }
    Ok(())
}

/// Percent-encodes a query value: unreserved characters pass through,
/// everything else becomes `%XX` (uppercase hex).
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::query::encode;
///
/// assert_eq!(encode("a b&c"), "a%20b%26c");
/// assert_eq!(encode("users"), "users");
/// ```
pub fn encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

/// Runs filter → sort → paginate over loaded items, returning page indices
/// plus the filtered total.
///
/// Sort and filter fields must exist in every item's attributes, else 400
/// naming the field. Sort order is total: nulls first, then booleans,
/// numbers, strings, arrays, objects; numbers compare numerically (mixed
/// int/float via `f64`); the sort is stable. Filters are AND-combined
/// equalities: strings compare raw, other scalars compare canonical JSON
/// (`36`, `true`), so `filter[age]=36` matches the number `36`.
///
/// SQL-backed apps skip this and map the structured query instead:
/// `sort` → `order_by` / `order_by_desc`, `filters` → `where_eq`, `page`
/// → `limit` + `offset((number - 1) * size)`.
///
/// # Examples
///
/// ```rust
/// use lumos_jsonapi::{memory_page, ApiQuery, AttributeMap, Resource};
///
/// struct Item {
///     id: i64,
///     age: u32,
/// }
/// impl Resource for Item {
///     const TYPE: &'static str = "items";
///     fn resource_id(&self) -> String {
///         self.id.to_string()
///     }
///     fn attributes(&self) -> lumos_core::Result<AttributeMap> {
///         let mut map = AttributeMap::new();
///         map.insert("age".to_string(), self.age.into());
///         Ok(map)
///     }
///     fn relationships(&self) -> Vec<lumos_jsonapi::NamedRelationship<'_>> {
///         Vec::new()
///     }
/// }
///
/// let items = vec![Item { id: 1, age: 30 }, Item { id: 2, age: 20 }];
/// let query = ApiQuery::parse(&[("sort".to_string(), "age".to_string())], "/items").unwrap();
/// let (indices, total) = memory_page(&items, &query).unwrap();
/// assert_eq!((indices, total), (vec![1, 0], 2));
/// ```
pub fn memory_page<T: Resource>(items: &[T], query: &ApiQuery) -> Result<(Vec<usize>, u64)> {
    let mut selected: Vec<usize> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let attributes = item.attributes()?;
        let mut matches = true;
        for filter in &query.filters {
            match attributes.get(&filter.field) {
                Some(value) => {
                    if !filter_matches(value, &filter.value) {
                        matches = false;
                        break;
                    }
                }
                None => {
                    return Err(AppError::bad_request(format!(
                        "unknown filter field {:?} for type {:?}",
                        filter.field,
                        T::TYPE
                    )));
                }
            }
        }
        if matches {
            selected.push(index);
        }
    }
    if !query.sort.is_empty() {
        let mut keyed: Vec<(usize, Vec<Value>)> = Vec::with_capacity(selected.len());
        for index in selected {
            let attributes = items[index].attributes()?;
            let mut keys = Vec::with_capacity(query.sort.len());
            for entry in &query.sort {
                match attributes.get(&entry.field) {
                    Some(value) => keys.push(value.clone()),
                    None => {
                        return Err(AppError::bad_request(format!(
                            "unknown sort field {:?} for type {:?}",
                            entry.field,
                            T::TYPE
                        )));
                    }
                }
            }
            keyed.push((index, keys));
        }
        keyed.sort_by(|left, right| compare_keys(&left.1, &right.1, &query.sort));
        selected = keyed.into_iter().map(|(index, _)| index).collect();
    }
    let total = selected.len() as u64;
    let start = (query.page.number - 1).saturating_mul(query.page.size);
    let page = selected
        .into_iter()
        .skip(start as usize)
        .take(query.page.size as usize)
        .collect();
    Ok((page, total))
}

/// Equality of one attribute against a raw filter string.
fn filter_matches(value: &Value, raw: &str) -> bool {
    match value {
        Value::String(text) => text == raw,
        Value::Number(number) => number.to_string() == raw,
        Value::Bool(flag) => flag.to_string() == raw,
        Value::Null => raw == "null",
        Value::Array(_) | Value::Object(_) => {
            serde_json::to_string(value).is_ok_and(|json| json == raw)
        }
    }
}

/// Multi-key comparison honoring per-key direction.
fn compare_keys(left: &[Value], right: &[Value], sort: &[SortField]) -> Ordering {
    for ((left_value, right_value), entry) in left.iter().zip(right.iter()).zip(sort.iter()) {
        let ordering = compare_values(left_value, right_value);
        if ordering != Ordering::Equal {
            return if entry.descending {
                ordering.reverse()
            } else {
                ordering
            };
        }
    }
    Ordering::Equal
}

/// Total order across JSON values: null < bool < number < string < array <
/// object; numbers compare numerically.
fn compare_values(left: &Value, right: &Value) -> Ordering {
    let rank = |value: &Value| match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    };
    rank(left)
        .cmp(&rank(right))
        .then_with(|| match (left, right) {
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Number(a), Value::Number(b)) => compare_numbers(a, b),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            _ => Ordering::Equal,
        })
}

fn compare_numbers(left: &serde_json::Number, right: &serde_json::Number) -> Ordering {
    if let (Some(a), Some(b)) = (left.as_u64(), right.as_u64()) {
        return a.cmp(&b);
    }
    if let (Some(a), Some(b)) = (left.as_i64(), right.as_i64()) {
        return a.cmp(&b);
    }
    left.as_f64()
        .zip(right.as_f64())
        .map_or(Ordering::Equal, |(a, b)| a.total_cmp(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn parses_the_full_matrix() {
        let query = ApiQuery::parse(
            &[
                pair("include", "author,comments.author"),
                pair("fields[articles]", "title,body"),
                pair("fields[people]", ""),
                pair("sort", "title,-created_at"),
                pair("filter[status]", "open"),
                pair("page[number]", "2"),
                pair("page[size]", "10"),
                pair("app_param", "ignored"),
            ],
            "/articles",
        )
        .unwrap();
        assert_eq!(query.include.len(), 2);
        assert_eq!(
            query.include[1],
            vec!["comments".to_string(), "author".to_string()]
        );
        assert_eq!(
            query.fields["articles"],
            vec!["title".to_string(), "body".to_string()]
        );
        assert!(query.fields["people"].is_empty());
        assert_eq!(query.sort.len(), 2);
        assert!(query.sort[1].descending);
        assert_eq!(query.filters.len(), 1);
        assert_eq!(
            query.page,
            Page {
                number: 2,
                size: 10
            }
        );
    }

    #[test]
    fn malformed_parameters_are_400s() {
        for (name, value) in [
            ("include", "author,,x"),
            ("include", "author."),
            ("sort", "-"),
            ("sort", ""),
            ("fields", "title"),
            ("filter", "x"),
            ("fields[]", "title"),
            ("filter[]", "x"),
            ("page[number]", "0"),
            ("page[number]", "x"),
            ("page[size]", "101"),
            ("page[offset]", "1"),
        ] {
            let error = ApiQuery::parse(&[pair(name, value)], "/t").unwrap_err();
            assert_eq!(
                error.status_code(),
                lumos_core::StatusCode::BAD_REQUEST,
                "{name}={value}"
            );
        }
    }

    #[test]
    fn page_links_cover_edges() {
        let query = ApiQuery::parse(&[], "/t").unwrap();
        let one = query.page_links(0);
        assert!(one.prev.is_none() && one.next.is_none());
        assert!(one.last.contains("page[number]=1"));

        let query =
            ApiQuery::parse(&[pair("page[number]", "5"), pair("page[size]", "10")], "/t").unwrap();
        let overrun = query.page_links(12);
        assert!(overrun.prev.is_some() && overrun.next.is_none());
        assert!(overrun.last.contains("page[number]=2"));
    }

    #[test]
    fn memory_pipeline_sorts_filters_paginates() {
        struct Item {
            id: i64,
            age: u32,
            tag: &'static str,
        }
        impl Resource for Item {
            const TYPE: &'static str = "items";
            fn resource_id(&self) -> String {
                self.id.to_string()
            }
            fn attributes(&self) -> Result<AttributeMap> {
                let mut map = AttributeMap::new();
                map.insert("age".to_string(), self.age.into());
                map.insert("tag".to_string(), self.tag.into());
                Ok(map)
            }
            fn relationships(&self) -> Vec<crate::NamedRelationship<'_>> {
                Vec::new()
            }
        }

        let items = vec![
            Item {
                id: 1,
                age: 30,
                tag: "a",
            },
            Item {
                id: 2,
                age: 20,
                tag: "b",
            },
            Item {
                id: 3,
                age: 40,
                tag: "a",
            },
        ];
        let query = ApiQuery::parse(
            &[
                pair("filter[tag]", "a"),
                pair("sort", "-age"),
                pair("page[size]", "1"),
            ],
            "/items",
        )
        .unwrap();
        let (indices, total) = memory_page(&items, &query).unwrap();
        assert_eq!((indices, total), (vec![2], 2));

        let bad = ApiQuery::parse(&[pair("sort", "nope")], "/items").unwrap();
        assert!(memory_page(&items, &bad).is_err());
    }
}
