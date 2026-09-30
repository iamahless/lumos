//! `#[derive(JsonApiResource)]` implementation.
//!
//! Generated paths point at `::lumos::lumos_jsonapi` unconditionally: the
//! facade is what apps depend on, and it re-exports `lumos_jsonapi` under
//! either `jsonapi` flag (documented in the crate docs).

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Expr, Fields, Lit, Meta, Type};

/// Container-level `#[resource(...)]` options.
#[derive(Default)]
struct ContainerOptions {
    resource_type: Option<(String, Span)>,
}

/// Field-level `#[resource(...)]` options.
#[derive(Default)]
struct FieldOptions {
    id: bool,
    hidden: bool,
    relation: bool,
    rename: Option<(String, Span)>,
}

/// Relation field shape, sniffed from the field type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RelationKind {
    ToOne,
    ToMany,
    OptionToOne,
}

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    input.ident.span(),
                    "#[derive(JsonApiResource)] requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "#[derive(JsonApiResource)] only supports structs",
            ))
        }
    };

    let container = parse_container(&input.attrs)?;
    let Some((resource_type, _)) = container.resource_type else {
        return Err(syn::Error::new(
            input.ident.span(),
            "#[derive(JsonApiResource)] requires #[resource(type = \"...\")]",
        ));
    };

    let mut id_field: Option<&syn::Ident> = None;
    let mut attributes = Vec::new();
    let mut relations = Vec::new();
    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new(field.span(), "JsonApiResource fields must be named"))?;
        let options = parse_field(&field.attrs)?;
        if options.relation {
            if options.id {
                return Err(syn::Error::new(
                    field.span(),
                    "the id field cannot be a relation",
                ));
            }
            if options.hidden {
                return Err(syn::Error::new(
                    field.span(),
                    "hidden applies to attributes only; relations render linkage, never values",
                ));
            }
            let kind = relation_kind(&field.ty)?;
            let name = rename_or(&options.rename, &ident.to_string());
            relations.push((ident, name, kind));
            continue;
        }
        if options.id {
            if options.hidden {
                return Err(syn::Error::new(
                    field.span(),
                    "hidden applies to attributes only; the id is structural, never an attribute",
                ));
            }
            if id_field.is_some() {
                return Err(syn::Error::new(
                    field.span(),
                    "JsonApiResource allows exactly one #[resource(id)] field",
                ));
            }
            id_field = Some(ident);
            continue;
        }
        if options.hidden {
            continue;
        }
        let key = rename_or(&options.rename, &ident.to_string());
        attributes.push((ident, key));
    }

    let Some(id_field) = id_field else {
        return Err(syn::Error::new(
            input.ident.span(),
            "#[derive(JsonApiResource)] requires exactly one #[resource(id)] field",
        ));
    };

    let ident = &input.ident;
    let attribute_inserts = attributes.iter().map(|(field, key)| {
        quote! {
            map.insert(
                #key.to_string(),
                ::lumos::lumos_jsonapi::to_attribute(#key, &self.#field)?,
            );
        }
    });
    let relation_entries = relations.iter().map(|(field, name, kind)| {
        let linkage = match kind {
            RelationKind::ToOne => {
                quote! {
                    ::lumos::lumos_jsonapi::Relationship::ToOne(
                        Some(self.#field.as_target())
                    )
                }
            }
            RelationKind::OptionToOne => {
                quote! {
                    ::lumos::lumos_jsonapi::Relationship::ToOne(
                        self.#field.as_ref().map(|link| link.as_target())
                    )
                }
            }
            RelationKind::ToMany => {
                quote! {
                    ::lumos::lumos_jsonapi::Relationship::ToMany(
                        self.#field.targets()
                    )
                }
            }
        };
        quote! {
            ::lumos::lumos_jsonapi::NamedRelationship {
                name: #name,
                relation: #linkage,
            }
        }
    });

    Ok(quote! {
        impl ::lumos::lumos_jsonapi::Resource for #ident {
            const TYPE: &'static str = #resource_type;

            fn resource_id(&self) -> String {
                self.#id_field.to_string()
            }

            fn attributes(
                &self,
            ) -> ::lumos::lumos_jsonapi::Result<::lumos::lumos_jsonapi::AttributeMap> {
                let mut map = ::lumos::lumos_jsonapi::AttributeMap::new();
                #(#attribute_inserts)*
                Ok(map)
            }

            fn relationships(
                &self,
            ) -> Vec<::lumos::lumos_jsonapi::NamedRelationship<'_>> {
                vec![#(#relation_entries),*]
            }
        }
    })
}

/// Parses struct-level `#[resource(type = "...")]`. Custom parser (not
/// `Meta`) because `type` is a strict keyword and never parses as a path.
fn parse_container(attrs: &[syn::Attribute]) -> syn::Result<ContainerOptions> {
    let mut options = ContainerOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("resource")) {
        attr.parse_args_with(|input: syn::parse::ParseStream| {
            input
                .parse::<syn::token::Type>()
                .map_err(|_| expected_container_error(input.span()))?;
            input
                .parse::<syn::Token![=]>()
                .map_err(|_| expected_container_error(input.span()))?;
            let lit: syn::LitStr = input
                .parse()
                .map_err(|_| expected_container_error(input.span()))?;
            if input.is_empty() {
                options.resource_type = Some((lit.value(), lit.span()));
                Ok(())
            } else {
                Err(expected_container_error(input.span()))
            }
        })?;
    }
    Ok(options)
}

fn expected_container_error(span: Span) -> syn::Error {
    syn::Error::new(span, "expected #[resource(type = \"...\")]")
}

/// Parses field-level `#[resource(...)]` options.
fn parse_field(attrs: &[syn::Attribute]) -> syn::Result<FieldOptions> {
    let mut options = FieldOptions::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("resource")) {
        let metas = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;
        for meta in metas {
            match meta {
                Meta::Path(path) if path.is_ident("id") => options.id = true,
                Meta::Path(path) if path.is_ident("hidden") => options.hidden = true,
                Meta::Path(path) if path.is_ident("relation") => options.relation = true,
                Meta::NameValue(pair) if pair.path.is_ident("rename") => {
                    let value = string_value(&pair.value)?;
                    options.rename = Some((value, pair.path.span()));
                }
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "unknown resource option; expected id, hidden, relation, or rename = \"...\"",
                    ));
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

/// The rename, or the field name when no rename was given.
fn rename_or(rename: &Option<(String, Span)>, field: &str) -> String {
    rename
        .as_ref()
        .map_or_else(|| field.to_string(), |(name, _)| name.clone())
}

/// Sniffs `ToOne<T>` / `ToMany<T>` / `Option<ToOne<T>>` from a field type.
/// Only the final path segment is inspected, so both imported and
/// fully-qualified spellings work.
fn relation_kind(ty: &Type) -> syn::Result<RelationKind> {
    let error = || {
        syn::Error::new(
            ty.span(),
            "relation fields must be ToOne<T>, ToMany<T>, or Option<ToOne<T>>",
        )
    };
    let Type::Path(path) = ty else {
        return Err(error());
    };
    let segment = path.path.segments.last().ok_or_else(error)?;
    match segment.ident.to_string().as_str() {
        "ToOne" => {
            require_one_argument(segment)?;
            Ok(RelationKind::ToOne)
        }
        "ToMany" => {
            require_one_argument(segment)?;
            Ok(RelationKind::ToMany)
        }
        "Option" => {
            let [arg] = require_one_argument(segment)?;
            let Type::Path(inner) = arg else {
                return Err(error());
            };
            let inner_segment = inner.path.segments.last().ok_or_else(error)?;
            if inner_segment.ident == "ToOne" {
                require_one_argument(inner_segment)?;
                Ok(RelationKind::OptionToOne)
            } else {
                Err(error())
            }
        }
        _ => Err(error()),
    }
}

/// Extracts the single generic argument of `Name<T>`.
fn require_one_argument(segment: &syn::PathSegment) -> syn::Result<[&Type; 1]> {
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return Err(syn::Error::new(
            segment.span(),
            "relation fields must be ToOne<T>, ToMany<T>, or Option<ToOne<T>>",
        ));
    };
    if args.args.len() != 1 {
        return Err(syn::Error::new(
            segment.span(),
            "relation fields must be ToOne<T>, ToMany<T>, or Option<ToOne<T>>",
        ));
    };
    let syn::GenericArgument::Type(ty) = &args.args[0] else {
        return Err(syn::Error::new(
            segment.span(),
            "relation fields must be ToOne<T>, ToMany<T>, or Option<ToOne<T>>",
        ));
    };
    Ok([ty])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand_str(source: &str) -> Result<String, String> {
        let input: DeriveInput = syn::parse_str(source).expect("fixture parses");
        expand(&input)
            .map(|tokens| tokens.to_string())
            .map_err(|error| error.to_string())
    }

    #[test]
    fn generates_resource_impl() {
        let tokens = expand_str(
            r#"
            pub struct Article {
                #[resource(id)]
                pub id: i64,
                pub title: String,
                #[resource(hidden)]
                pub internal: String,
                #[resource(rename = "body")]
                pub content: String,
                #[resource(relation)]
                pub author: ToOne<User>,
                #[resource(relation)]
                pub tags: ToMany<Tag>,
                #[resource(relation)]
                pub editor: Option<ToOne<User>>,
            }
            "#,
        );
        // No container attribute in this fixture: type is required.
        assert!(tokens.unwrap_err().contains("type"), "type is required");

        let tokens = expand_str(
            r#"
            #[resource(type = "articles")]
            pub struct Article {
                #[resource(id)]
                pub id: i64,
                pub title: String,
            }
            "#,
        )
        .unwrap();
        assert!(tokens.contains("articles"), "{tokens}");
        assert!(tokens.contains("self . id . to_string ()"), "{tokens}");
        assert!(tokens.contains("title"), "{tokens}");
    }

    #[test]
    fn id_rules_are_enforced() {
        let missing = expand_str(
            r#"
            #[resource(type = "t")]
            pub struct NoId {
                pub name: String,
            }
            "#,
        )
        .unwrap_err();
        assert!(missing.contains("exactly one"), "{missing}");

        let double = expand_str(
            r#"
            #[resource(type = "t")]
            pub struct TwoIds {
                #[resource(id)]
                pub a: i64,
                #[resource(id)]
                pub b: i64,
            }
            "#,
        )
        .unwrap_err();
        assert!(double.contains("exactly one"), "{double}");
    }

    #[test]
    fn relation_shapes_are_validated() {
        let bad_shape = expand_str(
            r#"
            #[resource(type = "t")]
            pub struct Bad {
                #[resource(id)]
                pub id: i64,
                #[resource(relation)]
                pub author: Vec<User>,
            }
            "#,
        )
        .unwrap_err();
        assert!(bad_shape.contains("ToOne"), "{bad_shape}");

        let hidden_relation = expand_str(
            r#"
            #[resource(type = "t")]
            pub struct Bad {
                #[resource(id)]
                pub id: i64,
                #[resource(relation, hidden)]
                pub author: ToOne<User>,
            }
            "#,
        )
        .unwrap_err();
        assert!(hidden_relation.contains("hidden"), "{hidden_relation}");

        let unknown = expand_str(
            r#"
            #[resource(type = "t")]
            pub struct Bad {
                #[resource(id)]
                pub id: i64,
                #[resource(frobnicate)]
                pub name: String,
            }
            "#,
        )
        .unwrap_err();
        assert!(unknown.contains("unknown resource option"), "{unknown}");
    }
}
