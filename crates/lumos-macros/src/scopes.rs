//! `#[scopes]` implementation: turns `scope_*` associated functions into
//! chainable query methods.
//!
//! Query scopes are ordinary associated functions taking and returning a
//! `Query`. `User::scope_active(User::query(&db))` always works with zero
//! machinery. This macro adds the Eloquent-style sugar: for each `pub fn
//! scope_active(...)` in the impl block, it generates a `UserScopes` trait
//! with an `.active()` method, so `User::query(&db).active().get()` reads
//! naturally.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::spanned::Spanned;
use syn::{ImplItem, ItemImpl, Visibility};

/// [`Query`](::rusticate::Query) method names a scope must not shadow: trait
/// methods lose to inherent ones silently, so collisions are compile errors.
const RESERVED: &[&str] = &[
    "new",
    "on",
    "to_sql",
    "where_eq",
    "where_like",
    "where_in",
    "where_null",
    "where_not_null",
    "order_by",
    "order_by_desc",
    "limit",
    "offset",
    "with",
    "with_trashed",
    "only_trashed",
    "get",
    "first",
    "first_or_fail",
    "count",
    "avg",
    "exists",
    "paginate",
    "chunk",
];

pub(crate) fn expand(
    attribute: proc_macro::TokenStream,
    input: ItemImpl,
) -> syn::Result<TokenStream2> {
    if input.trait_.is_some() {
        return Err(syn::Error::new(
            input.span(),
            "#[scopes] goes on the inherent impl block, not a trait impl",
        ));
    }
    let self_name = match input.self_ty.as_ref() {
        syn::Type::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
            path.path.segments[0].ident.clone()
        }
        _ => {
            return Err(syn::Error::new(
                input.self_ty.span(),
                "#[scopes] supports inherent impls on plain type names only",
            ))
        }
    };
    let mp = match crate_path(attribute)? {
        Some(path) => path.to_token_stream(),
        None => quote!(::rusticate),
    };

    let mut methods = Vec::new();
    for item in &input.items {
        let ImplItem::Fn(found) = item else {
            continue;
        };
        if !matches!(found.vis, Visibility::Public(_)) {
            continue;
        }
        let name = found.sig.ident.to_string();
        let Some(short) = name.strip_prefix("scope_") else {
            continue;
        };
        if short.is_empty() {
            return Err(syn::Error::new(
                found.sig.ident.span(),
                "scope name must not be empty (`scope_` + name)",
            ));
        }
        if RESERVED.contains(&short) {
            return Err(syn::Error::new(
                found.sig.ident.span(),
                format!("scope `{short}` would shadow Query::{short}; rename it"),
            ));
        }
        if found.sig.receiver().is_some() {
            return Err(syn::Error::new(
                found.sig.ident.span(),
                format!("scope `{name}` must be an associated function (no self), taking and returning a Query"),
            ));
        }
        let short_ident: syn::Ident = syn::parse_str(short).map_err(|_| {
            syn::Error::new(
                found.sig.ident.span(),
                format!("invalid scope name `{short}`"),
            )
        })?;
        methods.push((short_ident, found.sig.ident.clone()));
    }

    let trait_name: syn::Ident = syn::parse_str(&format!("{self_name}Scopes"))
        .map_err(|_| syn::Error::new(input.self_ty.span(), "cannot derive a scopes trait name"))?;
    let declarations = methods.iter().map(|(name, _)| {
        quote! {
            #[doc = "Chainable scope (see the `scopes` macro)."]
            fn #name(self) -> #mp::Query<#self_name>;
        }
    });
    let definitions = methods.iter().map(|(name, scope)| {
        quote! {
            fn #name(self) -> #mp::Query<#self_name> {
                #self_name::#scope(self)
            }
        }
    });
    Ok(quote! {
        #input

        #[doc = "Chainable query scopes (see the `scopes` macro). Import to use."]
        pub trait #trait_name {
            #(#declarations)*
        }

        impl #trait_name for #mp::Query<#self_name> {
            #(#definitions)*
        }
    })
}

/// Parses the optional `crate = "..."` override (default `::rusticate`).
fn crate_path(attribute: proc_macro::TokenStream) -> syn::Result<Option<syn::Path>> {
    if attribute.is_empty() {
        return Ok(None);
    }
    let meta: syn::Meta = syn::parse(attribute)?;
    let syn::Meta::NameValue(pair) = meta else {
        return Err(syn::Error::new(
            meta.span(),
            "expected #[scopes] or #[scopes(crate = \"...\")]",
        ));
    };
    if !pair.path.is_ident("crate") {
        return Err(syn::Error::new(
            pair.path.span(),
            "expected `crate = \"...\"`",
        ));
    }
    let syn::Expr::Lit(literal) = pair.value else {
        return Err(syn::Error::new(
            pair.value.span(),
            "expected `crate = \"...\"` with a string literal",
        ));
    };
    let syn::Lit::Str(path) = literal.lit else {
        return Err(syn::Error::new(
            literal.lit.span(),
            "expected `crate = \"...\"` with a string literal",
        ));
    };
    Ok(Some(syn::parse_str(&path.value())?))
}
