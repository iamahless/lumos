//! Bind values and the field encode/decode traits.
//!
//! [`BindValue`] is the only thing ever sent to the database: every field
//! type encodes into it ([`EncodeField`]), and every bind placeholder is
//! filled from it with its concrete type, so backend drivers always see
//! exact types, on every dialect. Decoding ([`DecodeField`]) goes the other
//! way, with small documented coercion chains where backends legitimately
//! differ (integer widths, boolean storage, timestamp representations).
//!
//! Two deliberate portability rules live here:
//!
//! - [`BindValue::Null`] is inlined as the `NULL` keyword, never bound, so
//!   no backend ever sees a mistyped NULL parameter.
//! - Timestamps and UUIDs bind as ISO strings and JSON binds as a JSON
//!   string; the query builder wraps those placeholders in dialect `CAST`s,
//!   so parsing happens server-side, uniformly.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::any::AnyRow;
use sqlx::{Row, ValueRef};

use crate::error::snip;
use crate::{Error, Result};

/// A single bound value, in backend-neutral form.
///
/// Variants mirror exactly the types sqlx's `Any` driver can encode, plus
/// [`BindValue::Null`] (inlined, never bound), [`BindValue::Time`] and
/// [`BindValue::Json`] (bound as strings under a dialect `CAST`).
///
/// # Examples
///
/// ```rust
/// use rusticate::BindValue;
///
/// assert!(BindValue::Null.is_null());
/// assert!(!BindValue::I64(1).is_null());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum BindValue {
    /// SQL `NULL`. Inlined as the keyword, never sent as a parameter.
    Null,
    /// Boolean.
    Bool(bool),
    /// Any integer (all int/uint field types widen here exactly, except
    /// `u64`/`i128`, which have no exact portable form and are unsupported).
    I64(i64),
    /// Any float (`f32` widens here exactly).
    F64(f64),
    /// Text.
    Text(String),
    /// Binary blob.
    Blob(Vec<u8>),
    /// Timestamp, bound as an ISO string under a dialect datetime `CAST`.
    Time(DateTime<Utc>),
    /// JSON document, bound as a compact string under a dialect JSON `CAST`
    /// (plain text on SQLite).
    Json(String),
}

impl BindValue {
    /// Returns `true` only for [`BindValue::Null`].
    ///
    /// # Examples
    ///
    /// ```rust
    /// use rusticate::BindValue;
    ///
    /// assert!(BindValue::Null.is_null());
    /// assert!(!BindValue::Bool(false).is_null());
    /// ```
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// Encodes a Rust field value into a [`BindValue`] for binding.
///
/// This is infallible by design: every supported type has an exact portable
/// form. Types without one (`u64`, `i128`, arbitrary `Serialize`) have no
/// impl. Attempting them is a compile error pointing here, not a runtime
/// surprise. (JSON casts use [`encode_json`], which can fail honestly.)
///
/// # Examples
///
/// ```rust
/// use rusticate::{BindValue, EncodeField};
///
/// assert_eq!(42i32.encode_field(), BindValue::I64(42));
/// assert_eq!(None::<String>.encode_field(), BindValue::Null);
/// ```
pub trait EncodeField {
    /// Encodes this value for binding.
    fn encode_field(&self) -> BindValue;
}

macro_rules! encode_int {
    ($($ty:ty),*) => {$(
        impl EncodeField for $ty {
            fn encode_field(&self) -> BindValue {
                BindValue::I64(*self as i64)
            }
        }
    )*};
}

encode_int!(i8, i16, i32, i64, u8, u16, u32);

impl EncodeField for usize {
    /// Encodes via `as i64`. Exact on every realistic platform (`usize`
    /// wider than `i64::MAX` does not occur outside theory).
    fn encode_field(&self) -> BindValue {
        BindValue::I64(*self as i64)
    }
}

impl EncodeField for isize {
    /// Encodes via `as i64` (exact on every realistic platform).
    fn encode_field(&self) -> BindValue {
        BindValue::I64(*self as i64)
    }
}

impl EncodeField for bool {
    fn encode_field(&self) -> BindValue {
        BindValue::Bool(*self)
    }
}

impl EncodeField for f32 {
    fn encode_field(&self) -> BindValue {
        BindValue::F64(f64::from(*self))
    }
}

impl EncodeField for f64 {
    fn encode_field(&self) -> BindValue {
        BindValue::F64(*self)
    }
}

impl EncodeField for String {
    fn encode_field(&self) -> BindValue {
        BindValue::Text(self.clone())
    }
}

impl EncodeField for str {
    fn encode_field(&self) -> BindValue {
        BindValue::Text(self.to_owned())
    }
}

/// Transparent references: `&str`, `&String`, `&i64`, … all encode as their
/// target (so string literals and borrows work anywhere a value does).
impl<T: EncodeField + ?Sized> EncodeField for &T {
    fn encode_field(&self) -> BindValue {
        (*self).encode_field()
    }
}

impl EncodeField for Vec<u8> {
    fn encode_field(&self) -> BindValue {
        BindValue::Blob(self.clone())
    }
}

impl EncodeField for [u8] {
    fn encode_field(&self) -> BindValue {
        BindValue::Blob(self.to_vec())
    }
}

impl EncodeField for DateTime<Utc> {
    fn encode_field(&self) -> BindValue {
        BindValue::Time(*self)
    }
}

impl EncodeField for uuid::Uuid {
    fn encode_field(&self) -> BindValue {
        BindValue::Text(self.hyphenated().to_string())
    }
}

impl EncodeField for serde_json::Value {
    /// Renders compact JSON. Serializing a `Value` is total; the fallback
    /// is unreachable defense, never a silent corruption path (a `Value`
    /// always round-trips).
    fn encode_field(&self) -> BindValue {
        BindValue::Json(serde_json::to_string(self).unwrap_or_else(|_| "null".to_string()))
    }
}

impl<T: EncodeField> EncodeField for Option<T> {
    fn encode_field(&self) -> BindValue {
        match self {
            Some(value) => value.encode_field(),
            None => BindValue::Null,
        }
    }
}

impl EncodeField for BindValue {
    /// Identity: already-encoded values (primary keys, re-applied changes)
    /// pass through untouched.
    fn encode_field(&self) -> BindValue {
        self.clone()
    }
}

/// Converts a [`BindValue`] back into a field type.
///
/// Used by generated `apply()` methods (and therefore [`Model::update`]).
/// Conversions mirror [`DecodeField`] but start from values instead of rows:
/// integers range-check, `0`/`1` become booleans, timestamps accept `Time`
/// values and ISO text, UUIDs parse from text.
///
/// # Examples
///
/// ```rust
/// use rusticate::{BindValue, FromBind};
///
/// assert_eq!(i64::from_bind(&BindValue::I64(3)).unwrap(), 3);
/// assert!(String::from_bind(&BindValue::I64(3)).is_err());
/// ```
pub trait FromBind: Sized {
    /// Converts a bound value into this field type.
    fn from_bind(value: &BindValue) -> Result<Self>;
}

macro_rules! from_bind_int {
    ($($ty:ty),*) => {$(
        impl FromBind for $ty {
            fn from_bind(value: &BindValue) -> Result<Self> {
                match value {
                    BindValue::I64(int) => <$ty>::try_from(*int).map_err(|_| {
                        Error::Decode(format!("integer {int} out of range"))
                    }),
                    other => Err(Error::Decode(format!("expected integer, got {other:?}"))),
                }
            }
        }
    )*};
}

from_bind_int!(i8, i16, i32, i64, u8, u16, u32);

impl FromBind for usize {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::I64(int) => usize::try_from(*int)
                .map_err(|_| Error::Decode(format!("integer {int} out of range"))),
            other => Err(Error::Decode(format!("expected integer, got {other:?}"))),
        }
    }
}

impl FromBind for isize {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::I64(int) => isize::try_from(*int)
                .map_err(|_| Error::Decode(format!("integer {int} out of range"))),
            other => Err(Error::Decode(format!("expected integer, got {other:?}"))),
        }
    }
}

impl FromBind for bool {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Bool(flag) => Ok(*flag),
            BindValue::I64(0) => Ok(false),
            BindValue::I64(1) => Ok(true),
            other => Err(Error::Decode(format!(
                "expected boolean or 0/1, got {other:?}"
            ))),
        }
    }
}

impl FromBind for f64 {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::F64(float) => Ok(*float),
            BindValue::I64(int) => Ok(*int as f64),
            other => Err(Error::Decode(format!("expected float, got {other:?}"))),
        }
    }
}

impl FromBind for f32 {
    fn from_bind(value: &BindValue) -> Result<Self> {
        f64::from_bind(value).map(|float| float as f32)
    }
}

impl FromBind for String {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Text(text) => Ok(text.clone()),
            other => Err(Error::Decode(format!("expected text, got {other:?}"))),
        }
    }
}

impl FromBind for Vec<u8> {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Blob(bytes) => Ok(bytes.clone()),
            other => Err(Error::Decode(format!("expected binary, got {other:?}"))),
        }
    }
}

impl FromBind for DateTime<Utc> {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Time(moment) => Ok(*moment),
            BindValue::Text(text) => parse_datetime(text).map_err(Error::Decode),
            other => Err(Error::Decode(format!("expected timestamp, got {other:?}"))),
        }
    }
}

impl FromBind for uuid::Uuid {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Text(text) => text
                .parse()
                .map_err(|_| Error::Decode(format!("invalid UUID {}", snip(text)))),
            other => Err(Error::Decode(format!("expected UUID text, got {other:?}"))),
        }
    }
}

impl FromBind for serde_json::Value {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Json(text) | BindValue::Text(text) => serde_json::from_str(text)
                .map_err(|error| Error::Decode(format!("invalid JSON: {error}"))),
            other => Err(Error::Decode(format!("expected JSON, got {other:?}"))),
        }
    }
}

impl<T: FromBind> FromBind for Option<T> {
    fn from_bind(value: &BindValue) -> Result<Self> {
        match value {
            BindValue::Null => Ok(None),
            present => T::from_bind(present).map(Some),
        }
    }
}

/// Converts a bind value into a `casts = "json"` field: `NULL` deserializes
/// from JSON `null` (so `Option` fields become `None` uniformly), JSON/text
/// parses, anything else errors.
///
/// # Examples
///
/// ```rust
/// use rusticate::{parse_json, BindValue};
///
/// let tags: Vec<String> = parse_json(&BindValue::Json(r#"["a"]"#.to_string())).unwrap();
/// assert_eq!(tags, vec!["a"]);
/// let missing: Option<Vec<String>> = parse_json(&BindValue::Null).unwrap();
/// assert_eq!(missing, None);
/// ```
pub fn parse_json<T: DeserializeOwned>(value: &BindValue) -> Result<T> {
    match value {
        BindValue::Null => serde_json::from_value(serde_json::Value::Null)
            .map_err(|error| Error::Decode(format!("invalid JSON cast: {error}"))),
        BindValue::Json(text) | BindValue::Text(text) => serde_json::from_str(text)
            .map_err(|error| Error::Decode(format!("invalid JSON cast: {error}"))),
        other => Err(Error::Decode(format!(
            "expected JSON for cast field, got {other:?}"
        ))),
    }
}

/// Converts a bind value into a `casts = "string"` field via `FromStr`.
///
/// # Examples
///
/// ```rust
/// use rusticate::{parse_display, BindValue};
///
/// assert_eq!(parse_display::<String>(&BindValue::Text("x".to_string())).unwrap(), "x");
/// assert!(parse_display::<u32>(&BindValue::Null).is_err());
/// ```
pub fn parse_display<T>(value: &BindValue) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match value {
        BindValue::Text(text) => text.parse::<T>().map_err(|error| {
            Error::Decode(format!(
                "cannot parse {} as target type: {error}",
                snip(text)
            ))
        }),
        other => Err(Error::Decode(format!(
            "expected text for cast field, got {other:?}"
        ))),
    }
}

/// Converts a bind value into an optional `casts = "string"` field:
/// `NULL` becomes `None`, text parses.
///
/// # Examples
///
/// ```rust
/// use rusticate::{parse_display_opt, BindValue};
///
/// assert_eq!(parse_display_opt::<String>(&BindValue::Null).unwrap(), None);
/// ```
pub fn parse_display_opt<T>(value: &BindValue) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match value {
        BindValue::Null => Ok(None),
        present => parse_display(present).map(Some),
    }
}

/// Encodes any serializable value as a [`BindValue::Json`] document.
///
/// Used by `casts = "json"` fields. Fails honestly when serialization fails.
///
/// # Examples
///
/// ```rust
/// use rusticate::{encode_json, BindValue};
///
/// let value = encode_json(&serde_json::json!({ "a": 1 })).unwrap();
/// assert_eq!(value, BindValue::Json(r#"{"a":1}"#.to_string()));
/// ```
pub fn encode_json<T: Serialize>(value: &T) -> Result<BindValue> {
    serde_json::to_string(value)
        .map(BindValue::Json)
        .map_err(|error| Error::Encode(format!("failed to encode JSON cast field: {error}")))
}

/// Encodes a displayable value as [`BindValue::Text`].
///
/// Used by `casts = "string"` fields (string-mapped enums).
///
/// # Examples
///
/// ```rust
/// use rusticate::{encode_display, BindValue};
///
/// assert_eq!(encode_display(&"admin"), BindValue::Text("admin".to_string()));
/// ```
pub fn encode_display<T: std::fmt::Display>(value: &T) -> BindValue {
    BindValue::Text(value.to_string())
}

/// Decodes a field value from a result row by column name.
///
/// Decoding is name-based, so column order does not matter, and strict by default,
/// with narrow documented coercions where backends genuinely differ.
/// Integers arrive in one attempt: the `Any` driver widens every integer
/// width (`INT2`/`INT4`/`INT8`, `TINYINT` through `BIGINT`, SQLite's dynamic
/// `INTEGER`) into `i64`, and narrower field types range-check from there.
/// `0`/`1` decode to `bool`; timestamps, UUIDs, and JSON decode from their
/// text forms (the query builder `CAST`s those columns to text on read;
/// the `Any` driver cannot return them natively).
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{DecodeField, Result};
/// use sqlx::any::AnyRow;
///
/// fn name(row: &AnyRow) -> Result<String> {
///     String::decode_field(row, "name")
/// }
/// ```
pub trait DecodeField: Sized {
    /// Decodes this field from `column` of `row`.
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self>;
}

/// Decodes any integer width: the `Any` driver widens `INT2`/`INT4`/`INT8`
/// (and every backend's equivalents) into `i64` with range checks.
fn decode_i64(row: &AnyRow, column: &str) -> Result<i64> {
    row.try_get::<i64, _>(column)
        .map_err(|_| Error::decode(column, "expected an integer"))
}

macro_rules! decode_int {
    ($($ty:ty),*) => {$(
        impl DecodeField for $ty {
            fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
                let value = decode_i64(row, column)?;
                <$ty>::try_from(value).map_err(|_| {
                    Error::decode(column, format!("integer {value} out of range"))
                })
            }
        }
    )*};
}

decode_int!(i8, i16, i32, i64, u8, u16, u32);

impl DecodeField for usize {
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        let value = decode_i64(row, column)?;
        usize::try_from(value)
            .map_err(|_| Error::decode(column, format!("integer {value} out of range")))
    }
}

impl DecodeField for isize {
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        let value = decode_i64(row, column)?;
        isize::try_from(value)
            .map_err(|_| Error::decode(column, format!("integer {value} out of range")))
    }
}

impl DecodeField for bool {
    /// Decodes native booleans, plus `0`/`1` integers (SQLite `INTEGER` and
    /// MySQL's `CAST(... AS SIGNED)` boolean reads). Anything else errors.
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(value) = row.try_get::<bool, _>(column) {
            return Ok(value);
        }
        match decode_i64(row, column)? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(Error::decode(
                column,
                format!("expected boolean or 0/1, got {other}"),
            )),
        }
    }
}

impl DecodeField for f64 {
    /// Decodes floats (`REAL` widens to `DOUBLE` in the driver), plus
    /// integers (SQLite `REAL` columns may hold integer values; large
    /// integers may lose precision; this is documented rather than silent).
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(value) = row.try_get::<f64, _>(column) {
            return Ok(value);
        }
        decode_i64(row, column).map(|value| value as f64)
    }
}

impl DecodeField for f32 {
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(value) = row.try_get::<f32, _>(column) {
            return Ok(value);
        }
        let value = f64::decode_field(row, column)?;
        Ok(value as f32)
    }
}

impl DecodeField for String {
    /// Decodes text columns strictly: no silent number-to-string coercion.
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        row.try_get::<String, _>(column)
            .map_err(|_| Error::decode(column, "expected text"))
    }
}

impl DecodeField for Vec<u8> {
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        row.try_get::<Vec<u8>, _>(column)
            .map_err(|_| Error::decode(column, "expected binary"))
    }
}

impl DecodeField for DateTime<Utc> {
    /// Decodes timestamp text in several layouts (see [`parse_datetime`]),
    /// plus unix-second integers. The `Any` driver cannot return timestamps
    /// natively, so these columns are always `CAST` to text on read.
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(text) = row.try_get::<String, _>(column) {
            return parse_datetime(&text).map_err(|detail| Error::decode(column, detail));
        }
        if let Ok(seconds) = row.try_get::<i64, _>(column) {
            return DateTime::from_timestamp(seconds, 0).ok_or_else(|| {
                Error::decode(column, format!("unix timestamp {seconds} out of range"))
            });
        }
        Err(Error::decode(column, "expected a timestamp"))
    }
}

impl DecodeField for uuid::Uuid {
    /// Decodes hyphenated UUID strings (timestamp-style `CAST` columns),
    /// plus 16-byte blobs (for schemas storing UUIDs as binary).
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(text) = row.try_get::<String, _>(column) {
            return text
                .parse()
                .map_err(|_| Error::decode(column, format!("invalid UUID {}", snip(&text))));
        }
        if let Ok(bytes) = row.try_get::<Vec<u8>, _>(column) {
            return uuid::Uuid::from_slice(&bytes)
                .map_err(|_| Error::decode(column, "invalid 16-byte UUID blob"));
        }
        Err(Error::decode(column, "expected a UUID"))
    }
}

impl DecodeField for serde_json::Value {
    /// Decodes JSON text (the `Any` driver cannot return JSON natively, so
    /// these columns are always `CAST` to text on read).
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if let Ok(text) = row.try_get::<String, _>(column) {
            return serde_json::from_str(&text)
                .map_err(|error| Error::decode(column, format!("invalid JSON: {error}")));
        }
        Err(Error::decode(column, "expected JSON"))
    }
}

impl<T: DecodeField> DecodeField for Option<T> {
    /// `None` for SQL `NULL` (detected without decoding, so any inner type
    /// works); otherwise decodes `T`.
    fn decode_field(row: &AnyRow, column: &str) -> Result<Self> {
        if is_null_cell(row, column)? {
            return Ok(None);
        }
        T::decode_field(row, column).map(Some)
    }
}

/// Decodes a `casts = "json"` field: `NULL` deserializes from JSON `null`
/// (so `Option` fields become `None` uniformly), JSON text parses into `T`.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{decode_json, Result};
/// use sqlx::any::AnyRow;
///
/// fn tags(row: &AnyRow) -> Result<Vec<String>> {
///     decode_json(row, "tags")
/// }
/// ```
pub fn decode_json<T: DeserializeOwned>(row: &AnyRow, column: &str) -> Result<T> {
    if is_null_cell(row, column)? {
        return serde_json::from_value(serde_json::Value::Null)
            .map_err(|error| Error::decode(column, format!("invalid JSON cast: {error}")));
    }
    if let Ok(text) = row.try_get::<String, _>(column) {
        return serde_json::from_str(&text)
            .map_err(|error| Error::decode(column, format!("invalid JSON cast: {error}")));
    }
    Err(Error::decode(column, "expected JSON for cast field"))
}

/// Decodes a `casts = "string"` field: text parsed with `FromStr`
/// (string-mapped enums).
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{decode_display, Result};
/// use sqlx::any::AnyRow;
///
/// fn role(row: &AnyRow) -> Result<String> {
///     decode_display(row, "role")
/// }
/// ```
pub fn decode_display<T>(row: &AnyRow, column: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let text = String::decode_field(row, column)?;
    text.parse::<T>().map_err(|error| {
        Error::decode(
            column,
            format!("cannot parse {} as target type: {error}", snip(&text)),
        )
    })
}

/// Decodes an optional `casts = "string"` field: `NULL` becomes `None`,
/// text parses.
///
/// # Examples
///
/// ```rust,no_run
/// use rusticate::{decode_display_opt, Result};
/// use sqlx::any::AnyRow;
///
/// fn role(row: &AnyRow) -> Result<Option<String>> {
///     decode_display_opt(row, "role")
/// }
/// ```
pub fn decode_display_opt<T>(row: &AnyRow, column: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    if is_null_cell(row, column)? {
        return Ok(None);
    }
    decode_display(row, column).map(Some)
}

/// Returns `true` when the cell is SQL `NULL`, without decoding it.
fn is_null_cell(row: &AnyRow, column: &str) -> Result<bool> {
    row.try_get_raw(column)
        .map(|raw| raw.is_null())
        .map_err(|_| Error::decode(column, "column missing from row"))
}

/// Parses the timestamp layouts Rusticate writes and backends return:
/// RFC 3339, Postgres `CAST AS TEXT` output (`YYYY-MM-DD HH:MM:SS[.f]+00`),
/// naive `YYYY-MM-DD HH:MM:SS[.f]` (space or `T` separator, assumed UTC),
/// bare dates (midnight UTC), and unix-second strings.
///
/// # Examples
///
/// ```rust
/// use rusticate::parse_datetime;
///
/// assert!(parse_datetime("2026-09-30T12:00:00Z").is_ok());
/// assert!(parse_datetime("2026-09-30 12:00:00").is_ok());
/// assert!(parse_datetime("2026-09-30").is_ok());
/// assert!(parse_datetime("not a time").is_err());
/// ```
pub fn parse_datetime(text: &str) -> std::result::Result<DateTime<Utc>, String> {
    if let Ok(with_zone) = text.parse::<DateTime<Utc>>() {
        return Ok(with_zone);
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%d %H:%M:%S%#z"] {
        if let Ok(with_zone) = DateTime::parse_from_str(text, format) {
            return Ok(with_zone.with_timezone(&Utc));
        }
    }
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(naive.and_utc());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        if let Some(midnight) = date.and_hms_opt(0, 0, 0) {
            return Ok(midnight.and_utc());
        }
    }
    if let Ok(seconds) = text.parse::<i64>() {
        if let Some(moment) = DateTime::from_timestamp(seconds, 0) {
            return Ok(moment);
        }
    }
    Err(format!("invalid timestamp {}", snip(text)))
}

/// Validates a SQL identifier (`column`, `table.column`): letters, digits,
/// underscores, one optional dot. Everything interpolated into SQL goes
/// through here; values never interpolate (they bind).
///
/// # Examples
///
/// ```rust
/// use rusticate::validate_identifier;
///
/// assert!(validate_identifier("email").is_ok());
/// assert!(validate_identifier("users.email").is_ok());
/// assert!(validate_identifier("email; DROP TABLE users").is_err());
/// ```
pub fn validate_identifier(name: &str) -> Result<()> {
    fn plain(part: &str) -> bool {
        let mut chars = part.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() || first == '_' => (),
            _ => return false,
        }
        chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }

    let valid = match name.split_once('.') {
        None => plain(name),
        Some((table, column)) => plain(table) && plain(column),
    };
    if valid {
        Ok(())
    } else {
        Err(Error::invalid_query(format!("invalid identifier {name:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_widen_exactly() {
        assert_eq!(7i8.encode_field(), BindValue::I64(7));
        assert_eq!(7u32.encode_field(), BindValue::I64(7));
        assert_eq!(7usize.encode_field(), BindValue::I64(7));
    }

    #[test]
    fn options_encode_none_as_null() {
        assert_eq!(None::<i64>.encode_field(), BindValue::Null);
        assert_eq!(Some(1i64).encode_field(), BindValue::I64(1));
    }

    #[test]
    fn uuid_encodes_hyphenated() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            id.encode_field(),
            BindValue::Text("00000000-0000-0000-0000-000000000000".to_string())
        );
    }

    #[test]
    fn datetime_layouts_parse() {
        use chrono::TimeZone;
        assert_eq!(
            parse_datetime("2026-09-30T12:00:00Z").unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
        );
        assert_eq!(
            parse_datetime("2026-09-30 12:00:00").unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
        );
        assert_eq!(
            parse_datetime("2026-09-30").unwrap(),
            Utc.with_ymd_and_hms(2026, 9, 30, 0, 0, 0).unwrap()
        );
        assert!(parse_datetime("not a time").is_err());
    }

    #[test]
    fn identifiers_reject_injection() {
        assert!(validate_identifier("email").is_ok());
        assert!(validate_identifier("_x1").is_ok());
        assert!(validate_identifier("users.email").is_ok());
        for bad in [
            "",
            "1x",
            "a-b",
            "a b",
            "a;b",
            "a.b.c",
            ".a",
            "a.",
            "x\" OR \"1\"=\"1",
        ] {
            assert!(validate_identifier(bad).is_err(), "for {bad:?}");
        }
    }
}
