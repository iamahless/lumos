//! `#[derive(Model)]` implementation.
//!
//! Generated paths point at `::rusticate` unconditionally: derives accept no
//! arguments, and rusticate is standalone by design, so every model owner
//! depends on it directly (documented in the crate docs).

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, ToTokens};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Expr, Fields, Lit, Meta, Type};

/// Framework root for all generated paths.
fn rusticate() -> TokenStream2 {
    quote!(::rusticate)
}

/// Container-level `#[model(...)]` options.
#[derive(Default)]
struct ContainerOptions {
    table: Option<(String, Span)>,
    primary_key: Option<(String, Span)>,
    timestamps: bool,
    soft_deletes: bool,
    appends: Vec<String>,
}

/// Field-level `#[model(...)]` options.
#[derive(Default)]
struct FieldOptions {
    id: bool,
    auto_increment: bool,
    hidden: bool,
    casts: Option<(String, Span)>,
    has_many: Option<Type>,
    belongs_to: Option<Type>,
    foreign_key: Option<String>,
    local_key: Option<String>,
    created_at: bool,
    updated_at: bool,
    deleted_at: bool,
}

/// One parsed field plus its classification.
struct Field<'a> {
    ident: &'a syn::Ident,
    ty: &'a Type,
    options: FieldOptions,
    /// Target model type for `HasMany<T>` / `BelongsTo<T>` typed fields (attributed or not).
    relation_target: Option<Type>,
}

/// A system-managed timestamp field (owned for codegen).
struct StampField {
    ident: syn::Ident,
    optional: bool,
}

/// Common resolved relation metadata.
struct RelationBase {
    field: syn::Ident,
    name: String,
    target: Type,
}

/// A fully-resolved relation (after inference + overrides).
enum Relation {
    HasMany {
        base: RelationBase,
        /// Column on the child (HasMany) used for fetching/grouping.
        fk_column: String,
        /// Field read for the foreign key (child field for HasMany, own field for BelongsTo).
        fk_field: syn::Ident,
        /// Own field holding the local key (HasMany; always the PK field unless overridden).
        local_field: syn::Ident,
        /// `create_{relation}` method name (HasMany only).
        create_ident: syn::Ident,
    },
    BelongsTo {
        base: RelationBase,
        fk_column: String,
        fk_field: syn::Ident,
    },
}

impl Relation {
    fn base(&self) -> &RelationBase {
        match self {
            Self::HasMany { base, .. } | Self::BelongsTo { base, .. } => base,
        }
    }

    fn field(&self) -> &syn::Ident {
        &self.base().field
    }
    fn name(&self) -> &str {
        &self.base().name
    }
    fn target(&self) -> &Type {
        &self.base().target
    }
    fn many(&self) -> bool {
        matches!(self, Self::HasMany { .. })
    }
    fn fk_column(&self) -> &str {
        match self {
            Self::HasMany { fk_column, .. } | Self::BelongsTo { fk_column, .. } => fk_column,
        }
    }
    fn fk_field(&self) -> &syn::Ident {
        match self {
            Self::HasMany { fk_field, .. } | Self::BelongsTo { fk_field, .. } => fk_field,
        }
    }
    fn local_field(&self) -> Option<&syn::Ident> {
        match self {
            Self::HasMany { local_field, .. } => Some(local_field),
            Self::BelongsTo { .. } => None,
        }
    }
    fn create_ident(&self) -> Option<&syn::Ident> {
        match self {
            Self::HasMany { create_ident, .. } => Some(create_ident),
            Self::BelongsTo { .. } => None,
        }
    }
}

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    input.ident.span(),
                    "#[derive(Model)] requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "#[derive(Model)] only supports structs",
            ))
        }
    };

    let container = parse_container(input)?;
    let table = container
        .table
        .as_ref()
        .map(|(name, _)| name.clone())
        .ok_or_else(|| {
            syn::Error::new(
                input.ident.span(),
                "missing #[model(table = \"...\")] on the struct",
            )
        })?;
    let primary_key = container
        .primary_key
        .map(|(name, _)| name)
        .unwrap_or_else(|| "id".to_string());

    // Parse + classify fields.
    let mut parsed: Vec<Field> = Vec::with_capacity(fields.len());
    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new(field.span(), "Model fields must be named"))?;
        let options = parse_field(field)?;
        let relation_target = relation_target_of(&field.ty);
        if relation_target.is_some() && options.has_many.is_none() && options.belongs_to.is_none() {
            return Err(syn::Error::new(
                field.ty.span(),
                format!(
                    "relation-typed field `{ident}` needs #[model(has_many = \"...\")] or #[model(belongs_to = \"...\")]"
                ),
            ));
        }
        if options.has_many.is_some() && options.belongs_to.is_some() {
            return Err(syn::Error::new(
                ident.span(),
                format!("field `{ident}` cannot be both has_many and belongs_to"),
            ));
        }
        if relation_target.is_none() && (options.has_many.is_some() || options.belongs_to.is_some())
        {
            return Err(syn::Error::new(
                field.ty.span(),
                format!("field `{ident}` declares a relation but is not typed HasMany<T> or BelongsTo<T>"),
            ));
        }
        if let Some((casts, span)) = &options.casts {
            if !matches!(casts.as_str(), "json" | "string" | "bool" | "datetime") {
                return Err(syn::Error::new(
                    *span,
                    format!(
                        "unknown casts = \"{casts}\": expected json, string, bool, or datetime"
                    ),
                ));
            }
            if relation_target.is_some() {
                return Err(syn::Error::new(
                    *span,
                    "casts do not apply to relation fields",
                ));
            }
        }
        if options.id && relation_target.is_some() {
            return Err(syn::Error::new(
                ident.span(),
                "the primary key cannot be a relation field",
            ));
        }
        if options.local_key.is_some() && options.belongs_to.is_some() {
            return Err(syn::Error::new(
                ident.span(),
                "local_key applies to has_many relations only",
            ));
        }
        parsed.push(Field {
            ident,
            ty: &field.ty,
            options,
            relation_target,
        });
    }
    let has_field = |name: &str| parsed.iter().any(|field| field.ident == name);

    // Primary key: marked `id`, else the field named by `primary_key`.
    let id_marked: Vec<&Field> = parsed.iter().filter(|field| field.options.id).collect();
    if id_marked.len() > 1 {
        return Err(syn::Error::new(
            input.ident.span(),
            "only one field may carry #[model(id)]",
        ));
    }
    let pk_field = match id_marked.first() {
        Some(field) => *field,
        None => parsed
            .iter()
            .find(|field| field.ident == primary_key.as_str())
            .ok_or_else(|| {
                syn::Error::new(
                    input.ident.span(),
                    format!(
                        "no primary key: mark a field #[model(id)] or add a `{primary_key}` field"
                    ),
                )
            })?,
    };
    if !pk_field.options.id && parsed.iter().any(|field| field.options.auto_increment) {
        return Err(syn::Error::new(
            input.ident.span(),
            "auto_increment requires the field to also carry #[model(id)]",
        ));
    }
    let auto_increment = pk_field.options.auto_increment;

    // Timestamp / soft-delete fields: markers win; flags assume conventional names.
    let created_field = resolve_marker(
        &parsed,
        "created_at",
        container.timestamps,
        "timestamps = true",
    )?;
    let updated_field = resolve_marker(
        &parsed,
        "updated_at",
        container.timestamps,
        "timestamps = true",
    )?;
    let deleted_field = resolve_marker(
        &parsed,
        "deleted_at",
        container.soft_deletes,
        "soft_deletes = true",
    )?;
    if let Some(field) = created_field {
        require_datetime(field, "created_at")?;
    }
    if let Some(field) = updated_field {
        require_datetime(field, "updated_at")?;
    }
    if let Some(field) = deleted_field {
        match option_inner(field.ty) {
            Some(_) => (),
            None => {
                return Err(syn::Error::new(
                    field.ty.span(),
                    "deleted_at must be Option<DateTime<Utc>>",
                ))
            }
        }
    }
    let stamp_of = |field: Option<&Field>| {
        field.map(|found| StampField {
            ident: (*found.ident).clone(),
            optional: option_inner(found.ty).is_some(),
        })
    };
    let created = stamp_of(created_field);
    let updated = stamp_of(updated_field);
    let deleted = stamp_of(deleted_field);
    let uses_timestamps = container.timestamps || created.is_some() || updated.is_some();
    let uses_soft_deletes = container.soft_deletes || deleted.is_some();

    // Relations with inference + overrides.
    let parent_snake = snake_case(&input.ident.to_string());
    let mut relations: Vec<Relation> = Vec::new();
    for field in parsed
        .iter()
        .filter(|field| field.relation_target.is_some())
    {
        let many = field.options.has_many.is_some();
        let target = field
            .relation_target
            .clone()
            .ok_or_else(|| syn::Error::new(field.ty.span(), "unreachable"))?;
        if many {
            let fk_column = field
                .options
                .foreign_key
                .clone()
                .unwrap_or_else(|| format!("{parent_snake}_id"));
            let fk_field = ident_parse(&fk_column, field.ident.span())?;
            let local_field = match &field.options.local_key {
                Some(name) => {
                    if !has_field(name) {
                        return Err(syn::Error::new(
                            field.ident.span(),
                            format!("local_key = \"{name}\" names no field of {}", input.ident),
                        ));
                    }
                    ident_parse(name, field.ident.span())?
                }
                None => pk_field.ident.clone(),
            };
            let create_ident = ident_parse(&format!("create_{}", field.ident), field.ident.span())?;
            relations.push(Relation::HasMany {
                base: RelationBase {
                    field: field.ident.clone(),
                    name: field.ident.to_string(),
                    target,
                },
                fk_column,
                fk_field,
                local_field,
                create_ident,
            });
        } else {
            let fk_name = field
                .options
                .foreign_key
                .clone()
                .unwrap_or_else(|| format!("{}_id", field.ident));
            if !has_field(&fk_name) {
                return Err(syn::Error::new(
                    field.ident.span(),
                    format!(
                        "belongs_to `{}` needs a `{fk_name}` field (or foreign_key = \"...\")",
                        field.ident
                    ),
                ));
            }
            relations.push(Relation::BelongsTo {
                base: RelationBase {
                    field: field.ident.clone(),
                    name: field.ident.to_string(),
                    target,
                },
                fk_column: fk_name.clone(),
                fk_field: ident_parse(&fk_name, field.ident.span())?,
            });
        }
    }

    // Appends must be callable method names (validated once, reused by codegen).
    let mut appends: Vec<syn::Ident> = Vec::with_capacity(container.appends.len());
    for name in &container.appends {
        match syn::parse_str::<syn::Ident>(name) {
            Ok(ident) => appends.push(ident),
            Err(_) => {
                return Err(syn::Error::new(
                    input.ident.span(),
                    format!("appends entry {name:?} is not a valid method name"),
                ))
            }
        }
    }
    let append_names: Vec<String> = container.appends.clone();

    Ok(emit(
        input,
        &parsed,
        pk_field.ident.clone(),
        pk_field.ty.clone(),
        auto_increment,
        created,
        updated,
        deleted,
        uses_timestamps,
        uses_soft_deletes,
        &relations,
        &table,
        &primary_key,
        &appends,
        &append_names,
    ))
}

/// Emits `impl Model` plus the inherent relation-method impl.
#[allow(clippy::too_many_arguments)]
fn emit(
    input: &DeriveInput,
    parsed: &[Field],
    pk_ident: syn::Ident,
    pk_ty: Type,
    auto_increment: bool,
    created: Option<StampField>,
    updated: Option<StampField>,
    deleted: Option<StampField>,
    uses_timestamps: bool,
    uses_soft_deletes: bool,
    relations: &[Relation],
    table: &str,
    primary_key: &str,
    appends: &[syn::Ident],
    append_names: &[String],
) -> TokenStream2 {
    let mp = rusticate();
    let name = &input.ident;

    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param.bounds.push(syn::parse_quote!(Send));
        param.bounds.push(syn::parse_quote!(Sync));
        param.bounds.push(syn::parse_quote!('static));
    }
    for lifetime in generics.lifetimes_mut() {
        lifetime
            .bounds
            .push(syn::Lifetime::new("'static", Span::call_site()));
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let columns: Vec<String> = parsed
        .iter()
        .filter(|field| field.relation_target.is_none())
        .map(|field| field.ident.to_string())
        .collect();
    let hidden: Vec<String> = parsed
        .iter()
        .filter(|field| field.options.hidden)
        .map(|field| field.ident.to_string())
        .collect();
    let relation_names: Vec<String> = relations
        .iter()
        .map(|relation| relation.name().to_string())
        .collect();
    // Columns the `Any` driver cannot return natively (cast to text on read)
    // and boolean columns (cast to signed on MySQL): derived from field
    // types plus explicit `casts` markers.
    let mut text_casts: Vec<String> = Vec::new();
    let mut bool_casts: Vec<String> = Vec::new();
    for field in parsed
        .iter()
        .filter(|field| field.relation_target.is_none())
    {
        let name = field.ident.to_string();
        let casts = field.options.casts.as_ref().map(|(kind, _)| kind.as_str());
        let inner = option_inner(field.ty).unwrap_or(field.ty);
        if matches!(casts, Some("json" | "datetime"))
            || is_datetime(inner)
            || is_uuid(inner)
            || is_json_value(inner)
        {
            text_casts.push(name.clone());
        }
        if matches!(casts, Some("bool")) || is_bool(inner) {
            bool_casts.push(name);
        }
    }

    let created_name = created.as_ref().map(|stamp| stamp.ident.to_string());
    let updated_name = updated.as_ref().map(|stamp| stamp.ident.to_string());
    let deleted_name = deleted.as_ref().map(|stamp| stamp.ident.to_string());
    let created_opt = option_literal(created_name.as_deref());
    let updated_opt = option_literal(updated_name.as_deref());
    let deleted_opt = option_literal(deleted_name.as_deref());

    let from_row_fields = parsed.iter().map(|field| {
        let ident = field.ident;
        let ty = field.ty;
        let column = ident.to_string();
        if field.relation_target.is_some() {
            return quote!(#ident: ::std::default::Default::default());
        }
        match field.options.casts.as_ref().map(|(kind, _)| kind.as_str()) {
            Some("json") => quote!(#ident: #mp::decode_json::<#ty>(row, #column)?),
            Some("string") => match option_inner(ty) {
                Some(inner) => quote!(#ident: #mp::decode_display_opt::<#inner>(row, #column)?),
                None => quote!(#ident: #mp::decode_display::<#ty>(row, #column)?),
            },
            Some(_) | None => {
                quote!(#ident: <#ty as #mp::DecodeField>::decode_field(row, #column)?)
            }
        }
    });

    // `mut` only when puts exist, so pk-only models compile warning-free.
    let to_changeset_puts: Vec<TokenStream2> = parsed
        .iter()
        .filter(|field| field.relation_target.is_none() && field.ident != &pk_ident)
        .map(|field| {
            let ident = field.ident;
            let column = ident.to_string();
            match field.options.casts.as_ref().map(|(kind, _)| kind.as_str()) {
                Some("json") => quote!(changeset.put(#column, #mp::encode_json(&self.#ident)?);),
                Some("string") => match option_inner(field.ty) {
                    Some(_) => quote! {
                        match &self.#ident {
                            None => changeset.put(#column, #mp::BindValue::Null),
                            Some(value) => changeset.put(#column, #mp::encode_display(value)),
                        };
                    },
                    None => quote!(changeset.put(#column, #mp::encode_display(&self.#ident));),
                },
                Some(_) | None => {
                    quote!(changeset.put(#column, #mp::EncodeField::encode_field(&self.#ident));)
                }
            }
        })
        .collect();
    let changeset_mut = if to_changeset_puts.is_empty() {
        quote! {}
    } else {
        quote! { mut }
    };

    let set_pk = if auto_increment {
        quote! {
            <#pk_ty as ::std::convert::TryFrom<i64>>::try_from(id)
                .map(|value| {
                    self.#pk_ident = value;
                })
                .map_err(|_| #mp::Error::Decode(format!("inserted id {id} out of range for primary key")))
        }
    } else {
        quote!({
            let _ = id;
            Ok(())
        })
    };

    let apply_arms = parsed.iter().filter(|field| field.relation_target.is_none()).map(|field| {
        let ident = field.ident;
        let column = ident.to_string();
        let ty = field.ty;
        if ident == &pk_ident {
            return quote!(#column => {
                return Err(#mp::Error::invalid_query("primary key cannot be changed via update"))
            });
        }
        match field.options.casts.as_ref().map(|(kind, _)| kind.as_str()) {
            Some("json") => quote!(#column => {
                self.#ident = #mp::parse_json::<#ty>(value)?;
            }),
            Some("string") => match option_inner(ty) {
                Some(inner) => quote!(#column => {
                    self.#ident = #mp::parse_display_opt::<#inner>(value)?;
                }),
                None => quote!(#column => {
                    self.#ident = #mp::parse_display::<#ty>(value)?;
                }),
            },
            Some(_) | None => quote!(#column => {
                self.#ident = <#ty as #mp::FromBind>::from_bind(value)?;
            }),
        }
    });

    let from_changeset_fields = parsed.iter().map(|field| {
        let ident = field.ident;
        let column = ident.to_string();
        let ty = field.ty;
        if field.relation_target.is_some() {
            return quote!(#ident: ::std::default::Default::default());
        }
        if ident == &pk_ident {
            if auto_increment {
                return quote!(#ident: ::std::default::Default::default());
            }
            return quote!(#ident: match changeset.get(#column) {
                Some(value) => <#ty as #mp::FromBind>::from_bind(value)?,
                // Non-incrementing keys default when absent (typically stamped
                // by `creating` hooks, e.g. UUIDs).
                None => ::std::default::Default::default(),
            });
        }
        let is_created = created_name.as_deref() == Some(column.as_str());
        let is_updated = updated_name.as_deref() == Some(column.as_str());
        let is_deleted = deleted_name.as_deref() == Some(column.as_str());
        if is_created || is_updated {
            // Timestamps: supplied values win (data imports), otherwise now.
            let now = match option_inner(ty) {
                Some(_) => quote!(Some(#mp::chrono::Utc::now())),
                None => quote!(#mp::chrono::Utc::now()),
            };
            return quote!(#ident: match changeset.get(#column) {
                Some(value) => <#ty as #mp::FromBind>::from_bind(value)?,
                None => #now,
            });
        }
        if is_deleted {
            return quote!(#ident: match changeset.get(#column) {
                Some(value) => <#ty as #mp::FromBind>::from_bind(value)?,
                None => None,
            });
        }
        match option_inner(ty) {
            Some(_) => quote!(#ident: match changeset.get(#column) {
                Some(value) => <#ty as #mp::FromBind>::from_bind(value)?,
                None => None,
            }),
            None => {
                let decode = match field.options.casts.as_ref().map(|(kind, _)| kind.as_str()) {
                    Some("json") => quote!(#mp::parse_json::<#ty>(value)?),
                    Some("string") => quote!(#mp::parse_display::<#ty>(value)?),
                    Some(_) | None => quote!(<#ty as #mp::FromBind>::from_bind(value)?),
                };
                quote!(#ident: match changeset.get(#column) {
                    Some(value) => #decode,
                    None => {
                        return Err(#mp::Error::invalid_query(format!(
                            "missing column `{}` for create",
                            #column
                        )))
                    }
                })
            }
        }
    });

    let stamp_update = updated.as_ref().map(|stamp| {
        let ident = &stamp.ident;
        if stamp.optional {
            quote!(self.#ident = Some(#mp::chrono::Utc::now());)
        } else {
            quote!(self.#ident = #mp::chrono::Utc::now();)
        }
    });
    let updated_at_value = updated
        .as_ref()
        .map(|stamp| {
            let ident = &stamp.ident;
            quote!(Some(#mp::EncodeField::encode_field(&self.#ident)))
        })
        .unwrap_or_else(|| quote!(None));
    let set_deleted = deleted
        .as_ref()
        .map(|stamp| {
            let ident = &stamp.ident;
            quote!(self.#ident = deleted;)
        })
        .unwrap_or_else(|| quote!(let _ = deleted;));
    let trashed = deleted
        .as_ref()
        .map(|stamp| {
            let ident = &stamp.ident;
            quote!(self.#ident.is_some())
        })
        .unwrap_or_else(|| quote!(false));

    // `mut` only when inserts exist, so fully-hidden models compile warning-free.
    let to_value_columns: Vec<TokenStream2> = parsed
        .iter()
        .filter(|field| field.relation_target.is_none() && !field.options.hidden)
        .map(|field| {
            let ident = field.ident;
            let column = ident.to_string();
            quote! {
                map.insert(
                    #column.to_string(),
                    #mp::serde_json::to_value(&self.#ident).map_err(|error| {
                        #mp::Error::Encode(format!("cannot serialize `{}`: {error}", #column))
                    })?,
                );
            }
        })
        .collect();
    let to_value_relations: Vec<TokenStream2> = relations
        .iter()
        .map(|relation| {
            let field = relation.field();
            let name = relation.name();
            if relation.many() {
                quote! {
                    if let Ok(items) = self.#field.get() {
                        let nested = items
                            .iter()
                            .map(<_ as #mp::Model>::to_value)
                            .collect::<#mp::Result<Vec<_>>>()?;
                        map.insert(#name.to_string(), #mp::serde_json::Value::Array(nested));
                    }
                }
            } else {
                quote! {
                    match self.#field.get() {
                        Ok(Some(one)) => {
                            map.insert(#name.to_string(), <_ as #mp::Model>::to_value(one)?);
                        }
                        Ok(None) => {
                            map.insert(#name.to_string(), #mp::serde_json::Value::Null);
                        }
                        Err(_) => {}
                    }
                }
            }
        })
        .collect();
    let to_value_appends: Vec<TokenStream2> = appends
        .iter()
        .zip(append_names.iter())
        .map(|(method, name)| {
            quote! {
                map.insert(
                    #name.to_string(),
                    #mp::serde_json::to_value(&self.#method()).map_err(|error| {
                        #mp::Error::Encode(format!("cannot serialize `{}`: {error}", #name))
                    })?,
                );
            }
        })
        .collect();
    let map_mut = if to_value_columns.is_empty()
        && to_value_relations.is_empty()
        && to_value_appends.is_empty()
    {
        quote! {}
    } else {
        quote! { mut }
    };

    let load_arms = relations.iter().map(|relation| {
        let name = relation.name();
        let target = relation.target();
        if relation.many() {
            let field = relation.field();
            let fk_column = relation.fk_column();
            let fk_field = relation.fk_field();
            let local = relation.local_field().expect("has-many relation");
            quote!(#name => {
                let keys: Vec<#mp::BindValue> = models
                    .iter()
                    .map(|m| #mp::EncodeField::encode_field(&m.#local))
                    .collect();
                let mut children =
                    #mp::relations::fetch_many_by_keys::<#target>(target, #fk_column, keys).await?;
                if !rest.is_empty() {
                    <#target as #mp::Model>::load_relation(target, &mut children, rest).await?;
                }
                let locals: Vec<#mp::BindValue> = models
                    .iter()
                    .map(|m| #mp::EncodeField::encode_field(&m.#local))
                    .collect();
                let mut buckets: Vec<Vec<#target>> =
                    locals.iter().map(|_| Vec::new()).collect();
                for child in children {
                    let key = #mp::EncodeField::encode_field(&child.#fk_field);
                    if let Some(index) = locals.iter().position(|local| *local == key) {
                        buckets[index].push(child);
                    }
                }
                for (model, mine) in models.iter_mut().zip(buckets) {
                    model.#field = #mp::HasMany::loaded(mine);
                }
            })
        } else {
            let field = relation.field();
            let fk_field = relation.fk_field();
            quote!(#name => {
                let keys: Vec<#mp::BindValue> = models
                    .iter()
                    .map(|m| #mp::EncodeField::encode_field(&m.#fk_field))
                    .collect();
                let mut owners = #mp::relations::fetch_many_by_keys::<#target>(
                    target,
                    <#target as #mp::Model>::primary_key(),
                    keys,
                )
                .await?;
                if !rest.is_empty() {
                    <#target as #mp::Model>::load_relation(target, &mut owners, rest).await?;
                }
                for m in models.iter_mut() {
                    let key = #mp::EncodeField::encode_field(&m.#fk_field);
                    let found = owners
                        .iter()
                        .position(|o| <_ as #mp::Model>::pk_value(o) == key)
                        .map(|index| owners.remove(index));
                    m.#field = #mp::BelongsTo::loaded(found);
                }
            })
        }
    });

    let load_arms: Vec<TokenStream2> = load_arms.collect();
    // Relation-less models skip the match: a single catch-all arm that
    // always returns would leave the trailing `Ok(())` unreachable.
    let load_body = if load_arms.is_empty() {
        quote! {
            Box::pin(async move {
                let Some((head, _)) = path.split_first() else {
                    return Err(#mp::Error::invalid_query("empty relation path"));
                };
                Err(#mp::Error::RelationNotFound(format!(
                    "relation `{head}` on {} (this model declares no relations)",
                    Self::table(),
                )))
            })
        }
    } else {
        quote! {
            Box::pin(async move {
                let Some((head, rest)) = path.split_first() else {
                    return Err(#mp::Error::invalid_query("empty relation path"));
                };
                match head.as_str() {
                    #(#load_arms,)*
                    other => {
                        return Err(#mp::Error::RelationNotFound(format!(
                            "relation `{other}` on {} (valid: [{}])",
                            Self::table(),
                            Self::relation_names().join(", ")
                        )))
                    }
                }
                Ok(())
            })
        }
    };

    let relation_methods = relations.iter().map(|relation| {
        let field = relation.field();
        let target = relation.target();
        if relation.many() {
            let fk_column = relation.fk_column();
            let local = relation.local_field().expect("has-many relation");
            let create_name = relation.create_ident().expect("has-many relation");
            quote!(
                /// Lazy to-many query, pre-filtered to this instance.
                pub fn #field(&self, target: impl Into<#mp::Target>) -> #mp::Query<#target> {
                    <#target as #mp::Model>::query(target)
                        .where_eq(#fk_column, #mp::EncodeField::encode_field(&self.#local))
                }

                /// Creates a related row with the foreign key stamped automatically.
                pub async fn #create_name(
                    &self,
                    target: impl Into<#mp::Target>,
                    mut changeset: #mp::Changeset,
                ) -> #mp::Result<#target> {
                    changeset.put(#fk_column, #mp::EncodeField::encode_field(&self.#local));
                    <#target as #mp::Model>::create(target, changeset).await
                }
            )
        } else {
            let fk_field = relation.fk_field();
            quote!(
                /// Lazy to-one query, pre-filtered to this instance's foreign key.
                pub fn #field(&self, target: impl Into<#mp::Target>) -> #mp::Query<#target> {
                    <#target as #mp::Model>::query(target).where_eq(
                        <#target as #mp::Model>::primary_key(),
                        #mp::EncodeField::encode_field(&self.#fk_field),
                    )
                }
            )
        }
    });

    quote! {
        impl #impl_generics #mp::Model for #name #type_generics #where_clause {
            fn table() -> &'static str {
                #table
            }

            fn primary_key() -> &'static str {
                #primary_key
            }

            fn columns() -> &'static [&'static str] {
                &[#(#columns),*]
            }

            fn auto_increment_pk() -> bool {
                #auto_increment
            }

            fn uses_timestamps() -> bool {
                #uses_timestamps
            }

            fn created_at_column() -> Option<&'static str> {
                #created_opt
            }

            fn updated_at_column() -> Option<&'static str> {
                #updated_opt
            }

            fn soft_deletes() -> bool {
                #uses_soft_deletes
            }

            fn deleted_at_column() -> Option<&'static str> {
                #deleted_opt
            }

            fn hidden_columns() -> &'static [&'static str] {
                &[#(#hidden),*]
            }

            fn relation_names() -> &'static [&'static str] {
                &[#(#relation_names),*]
            }

            fn text_cast_columns() -> &'static [&'static str] {
                &[#(#text_casts),*]
            }

            fn bool_columns() -> &'static [&'static str] {
                &[#(#bool_casts),*]
            }

            fn from_row(row: &#mp::AnyRow) -> #mp::Result<Self> {
                Ok(Self {
                    #(#from_row_fields,)*
                })
            }

            fn to_changeset(&self) -> #mp::Result<#mp::Changeset> {
                let #changeset_mut changeset = #mp::Changeset::new();
                #(#to_changeset_puts)*
                Ok(changeset)
            }

            fn pk_value(&self) -> #mp::BindValue {
                #mp::EncodeField::encode_field(&self.#pk_ident)
            }

            fn set_pk_from_i64(&mut self, id: i64) -> #mp::Result<()> {
                #set_pk
            }

            fn apply(&mut self, changeset: &#mp::Changeset) -> #mp::Result<()> {
                for (column, value) in changeset.iter() {
                    match column.as_str() {
                        #(#apply_arms,)*
                        other => {
                            return Err(#mp::Error::invalid_query(format!(
                                "unknown column `{other}` for {}",
                                Self::table()
                            )))
                        }
                    }
                }
                Ok(())
            }

            fn from_changeset(changeset: &#mp::Changeset) -> #mp::Result<Self> {
                for (column, _) in changeset.iter() {
                    match column.as_str() {
                        #(#columns)|* => {}
                        other => {
                            return Err(#mp::Error::invalid_query(format!(
                                "unknown column `{other}` for {}",
                                Self::table()
                            )))
                        }
                    }
                }
                Ok(Self {
                    #(#from_changeset_fields,)*
                })
            }

            fn stamp_update(&mut self) {
                #stamp_update
            }

            fn updated_at_value(&self) -> Option<#mp::BindValue> {
                #updated_at_value
            }

            fn set_deleted_at(&mut self, deleted: Option<#mp::chrono::DateTime<#mp::chrono::Utc>>) {
                #set_deleted
            }

            fn trashed(&self) -> bool {
                #trashed
            }

            fn to_value(&self) -> #mp::Result<#mp::serde_json::Value> {
                let #map_mut map = #mp::serde_json::Map::new();
                #(#to_value_columns)*
                #(#to_value_relations)*
                #(#to_value_appends)*
                Ok(#mp::serde_json::Value::Object(map))
            }

            fn load_relation<'a>(
                target: &'a #mp::Target,
                models: &'a mut [Self],
                path: &'a [String],
            ) -> ::std::pin::Pin<
                Box<dyn ::std::future::Future<Output = #mp::Result<()>> + Send + 'a>,
            > {
                #load_body
            }
        }

        impl #impl_generics #name #type_generics #where_clause {
            #(#relation_methods)*
        }
    }
}

/// Parses struct-level `#[model(...)]` options (multiple attributes merge).
fn parse_container(input: &DeriveInput) -> syn::Result<ContainerOptions> {
    let mut options = ContainerOptions::default();
    for attr in input
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("model"))
    {
        let items = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;
        for item in items {
            match item {
                Meta::Path(path) if path.is_ident("timestamps") => options.timestamps = true,
                Meta::Path(path) if path.is_ident("soft_deletes") => options.soft_deletes = true,
                Meta::NameValue(pair) if pair.path.is_ident("table") => {
                    options.table = Some((string_value(&pair.value)?, pair.path.span()));
                }
                Meta::NameValue(pair) if pair.path.is_ident("primary_key") => {
                    let name = string_value(&pair.value)?;
                    validate_ident(&name, pair.path.span())?;
                    options.primary_key = Some((name, pair.path.span()));
                }
                Meta::NameValue(pair) if pair.path.is_ident("timestamps") => {
                    options.timestamps = bool_value(&pair.value)?;
                }
                Meta::NameValue(pair) if pair.path.is_ident("soft_deletes") => {
                    options.soft_deletes = bool_value(&pair.value)?;
                }
                Meta::NameValue(pair) if pair.path.is_ident("appends") => {
                    options.appends = string_list(&pair.value)?;
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unknown #[model(...)] option: expected table, primary_key, timestamps, soft_deletes, appends",
                    ))
                }
            }
        }
    }
    if let Some((table, span)) = &options.table {
        validate_ident(table, *span)?;
    }
    Ok(options)
}

/// Parses field-level `#[model(...)]` options (multiple attributes merge).
fn parse_field(field: &syn::Field) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in field
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("model"))
    {
        let items = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;
        for item in items {
            match item {
                Meta::Path(path) if path.is_ident("id") => options.id = true,
                Meta::Path(path) if path.is_ident("auto_increment") => options.auto_increment = true,
                Meta::Path(path) if path.is_ident("hidden") => options.hidden = true,
                Meta::Path(path) if path.is_ident("created_at") => options.created_at = true,
                Meta::Path(path) if path.is_ident("updated_at") => options.updated_at = true,
                Meta::Path(path) if path.is_ident("deleted_at") => options.deleted_at = true,
                Meta::NameValue(pair) if pair.path.is_ident("casts") => {
                    options.casts = Some((string_value(&pair.value)?, pair.path.span()));
                }
                Meta::NameValue(pair) if pair.path.is_ident("has_many") => {
                    options.has_many = Some(type_value(&pair.value)?);
                }
                Meta::NameValue(pair) if pair.path.is_ident("belongs_to") => {
                    options.belongs_to = Some(type_value(&pair.value)?);
                }
                Meta::NameValue(pair) if pair.path.is_ident("foreign_key") => {
                    let name = string_value(&pair.value)?;
                    validate_ident(&name, pair.path.span())?;
                    options.foreign_key = Some(name);
                }
                Meta::NameValue(pair) if pair.path.is_ident("local_key") => {
                    let name = string_value(&pair.value)?;
                    validate_ident(&name, pair.path.span())?;
                    options.local_key = Some(name);
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unknown #[model(...)] option: expected id, auto_increment, hidden, casts, has_many, belongs_to, foreign_key, local_key, created_at, updated_at, deleted_at",
                    ))
                }
            }
        }
    }
    Ok(options)
}

/// Extracts a string literal's value.
fn string_value(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(text) => Ok(text.value()),
            _ => Err(syn::Error::new(expr.span(), "expected a string literal")),
        },
        _ => Err(syn::Error::new(expr.span(), "expected a string literal")),
    }
}

/// Extracts a boolean literal's value.
fn bool_value(expr: &Expr) -> syn::Result<bool> {
    match expr {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Bool(flag) => Ok(flag.value),
            _ => Err(syn::Error::new(expr.span(), "expected `true` or `false`")),
        },
        _ => Err(syn::Error::new(expr.span(), "expected `true` or `false`")),
    }
}

/// Extracts `["a", "b"]` string lists.
fn string_list(expr: &Expr) -> syn::Result<Vec<String>> {
    match expr {
        Expr::Array(array) => array.elems.iter().map(string_value).collect(),
        _ => Err(syn::Error::new(
            expr.span(),
            "expected a string list like [\"a\", \"b\"]",
        )),
    }
}

/// Extracts a model type from `"Post"` or bare `Post`.
fn type_value(expr: &Expr) -> syn::Result<Type> {
    if let Expr::Lit(literal) = expr {
        if let Lit::Str(text) = &literal.lit {
            return syn::parse_str(&text.value());
        }
    }
    syn::parse2(expr.to_token_stream())
}

/// Validates an identifier (mirrors `rusticate::validate_identifier`, minus dots).
fn validate_ident(name: &str, span: Span) -> syn::Result<()> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            span,
            format!("invalid identifier {name:?}"),
        ))
    }
}

/// Parses a pre-validated name into an identifier. It is fallible and does not panic.
fn ident_parse(name: &str, span: Span) -> syn::Result<syn::Ident> {
    syn::parse_str(name).map_err(|_| syn::Error::new(span, format!("invalid identifier {name:?}")))
}

/// Extracts the target model type from `HasMany<T>` / `BelongsTo<T>` field types.
fn relation_target_of(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    match segment.ident.to_string().as_str() {
        "HasMany" | "BelongsTo" => (),
        _ => return None,
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match &args.args[0] {
        syn::GenericArgument::Type(target) => Some(target.clone()),
        _ => None,
    }
}

/// Extracts `T` from `Option<T>`.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match &args.args[0] {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

/// Checks for `DateTime<...>` by final segment (chrono's type, however imported).
fn is_datetime(ty: &Type) -> bool {
    last_segment(ty).is_some_and(|name| name == "DateTime")
}

/// Checks for `Uuid` by final segment (the `uuid` crate's type, however imported).
fn is_uuid(ty: &Type) -> bool {
    last_segment(ty).is_some_and(|name| name == "Uuid")
}

/// Checks for `serde_json::Value` by final segment. A custom `Value` type
/// would also match. This is harmless because casting a text column to text is a no-op.
fn is_json_value(ty: &Type) -> bool {
    last_segment(ty).is_some_and(|name| name == "Value")
}

/// Checks for `bool` by final segment.
fn is_bool(ty: &Type) -> bool {
    last_segment(ty).is_some_and(|name| name == "bool")
}

/// Returns the final path segment of a type, if it is a path.
/// (`Option<DateTime<Utc>>` yields `"Option"`; unwrap with
/// [`option_inner`] first when classifying the inner type.)
fn last_segment(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

/// Requires a timestamp field to be `DateTime<Utc>` or `Option<DateTime<Utc>>`.
fn require_datetime(field: &Field, role: &str) -> syn::Result<()> {
    let valid = is_datetime(field.ty) || option_inner(field.ty).is_some_and(is_datetime);
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            field.ty.span(),
            format!(
                "`{role}` field `{}` must be DateTime<Utc> or Option<DateTime<Utc>>",
                field.ident
            ),
        ))
    }
}

/// Resolves a timestamp marker: explicit marker wins; otherwise the flag
/// assumes the conventional field name (missing field = explicit error).
fn resolve_marker<'a, 'b>(
    parsed: &'a [Field<'b>],
    name: &str,
    flag: bool,
    flag_name: &str,
) -> syn::Result<Option<&'a Field<'b>>> {
    let marked = parsed.iter().find(|field| match name {
        "created_at" => field.options.created_at,
        "updated_at" => field.options.updated_at,
        "deleted_at" => field.options.deleted_at,
        _ => false,
    });
    if let Some(field) = marked {
        return Ok(Some(field));
    }
    if flag {
        match parsed.iter().find(|field| field.ident == name) {
            Some(field) => Ok(Some(field)),
            None => Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "{flag_name} needs a `{name}` field (or an explicit #[model({name})] marker)"
                ),
            )),
        }
    } else {
        Ok(None)
    }
}

/// Converts snake_case for foreign-key inference (`UserProfile` → `user_profile`).
fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Renders `Some("...")` / `None` for column-name options.
fn option_literal(name: Option<&str>) -> TokenStream2 {
    match name {
        Some(column) => quote!(Some(#column)),
        None => quote!(None),
    }
}
