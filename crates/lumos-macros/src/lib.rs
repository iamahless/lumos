//! Compile-time codegen for Lumos.
//!
//! All framework "magic" lives here, at compile time: there is no runtime
//! reflection and no global state anywhere in Lumos.
//!
//! Phase 1 ships [`controller`]; Phase 2 adds [`Model`] and [`scopes`].
//! `Validate`, `FormRequest`, `Migration`, and `JsonApiResource` arrive with
//! their phases.

mod model;
mod scopes;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, spanned::Spanned, FnArg, ImplItem, Item};

/// Marks a controller struct (dependency injection) or impl block (routing).
///
/// Applied to a **struct** with named fields, generates a `from_container`
/// constructor that resolves every field from the [`Container`](::lumos::Container).
/// Applied to an **impl block**, generates a `routes` constructor mapping
/// convention methods to RESTful routes:
///
/// | Method    | Verb      | Path   |
/// |-----------|-----------|--------|
/// | `index`   | `GET`     | `/`    |
/// | `store`   | `POST`    | `/`    |
/// | `show`    | `GET`     | `/{id}`|
/// | `update`  | `PUT+PATCH`| `/{id}`|
/// | `destroy` | `DELETE`  | `/{id}`|
///
/// Only `pub` convention methods become routes, and only for methods that
/// exist — a controller with just `index` and `show` gets exactly two
/// routes. Actions must take `&self`, must not be generic, and may be
/// `async` or sync; their parameters are forwarded as axum extractors and
/// their return values must implement `IntoResponse` (typically
/// `Result<Response, AppError>`).
///
/// Generated paths point at `::lumos` by default; crates using `lumos-core`
/// directly pass `#[controller(crate = "lumos_core")]`.
///
/// # Examples
///
/// ```ignore
/// use lumos::{controller, Container, Json, Path, Query, Response, Result};
///
/// #[controller]
/// pub struct UserController {
///     users: UserService, // resolved from the container; must be Clone
/// }
///
/// #[controller]
/// impl UserController {
///     pub async fn index(&self, Query(page): Query<u32>) -> Result<Response> {
///         // ...
///         # Ok(lumos::ok(&[] as &[u8]))
///     }
///
///     pub async fn show(&self, Path(id): Path<i64>) -> Result<Response> {
///         // ...
///         # Ok(lumos::ok(&id))
///     }
/// }
///
/// # fn mount(container: &Container) -> Result<lumos::Router> {
/// let controller = std::sync::Arc::new(UserController::from_container(container)?);
/// let router = lumos::routes! { resource("/users", UserController, controller) };
/// # Ok(router)
/// # }
/// ```
#[proc_macro_attribute]
pub fn controller(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let crate_path = match parse_crate_path(attribute) {
        Ok(path) => path,
        Err(error) => return error.to_compile_error().into(),
    };
    match parse_macro_input!(item as Item) {
        Item::Struct(input) => expand_struct(&crate_path, input),
        Item::Impl(input) => expand_impl(&crate_path, input),
        other => syn::Error::new(
            other.span(),
            "#[controller] goes on a struct (dependency injection) or an impl block (route dispatch)",
        )
        .to_compile_error()
        .into(),
    }
}

/// Resolves the framework path: `::lumos` by default, overridable via
/// `#[controller(crate = "some_path")]` for direct `lumos-core` users.
fn parse_crate_path(attribute: TokenStream) -> syn::Result<syn::Path> {
    if attribute.is_empty() {
        return syn::parse_str("::lumos");
    }
    let meta: syn::Meta = syn::parse(attribute)?;
    let syn::Meta::NameValue(pair) = meta else {
        return Err(syn::Error::new(
            meta.span(),
            "expected #[controller] or #[controller(crate = \"...\")]",
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
    syn::parse_str(&path.value())
}

/// Struct form: generates `from_container`, resolving each named field.
fn expand_struct(crate_path: &syn::Path, input: syn::ItemStruct) -> TokenStream {
    let syn::Fields::Named(fields) = &input.fields else {
        return syn::Error::new(
            input.ident.span(),
            "#[controller] on a struct requires named fields; each field is resolved from the container",
        )
        .to_compile_error()
        .into();
    };

    let mut initializers = Vec::with_capacity(fields.named.len());
    for field in &fields.named {
        let Some(name) = field.ident.as_ref() else {
            return syn::Error::new(field.span(), "#[controller] requires named fields")
                .to_compile_error()
                .into();
        };
        let ty = &field.ty;
        initializers.push(quote! { #name: container.resolve_value::<#ty>()? });
    }

    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        #input

        impl #impl_generics #name #type_generics #where_clause {
            #[doc = "Builds this controller by resolving every field from the container."]
            #[doc = ""]
            #[doc = "Each field type must be registered (see `Container::bind` /"]
            #[doc = "`Container::singleton`) and implement `Clone`. For services that"]
            #[doc = "are not `Clone`, hold an `Arc<T>` field and resolve it explicitly."]
            pub fn from_container(container: &#crate_path::Container) -> #crate_path::Result<Self> {
                Ok(Self {
                    #(#initializers,)*
                })
            }
        }
    }
    .into()
}

/// One convention action: method name, HTTP verbs, and relative path.
struct Action {
    method: &'static str,
    verbs: &'static [&'static str],
    path: &'static str,
}

/// The RESTful convention table. Controllers mount under a prefix via
/// `routes!` / `nest`, so these paths stay relative.
const ACTIONS: &[Action] = &[
    Action {
        method: "index",
        verbs: &["get"],
        path: "/",
    },
    Action {
        method: "store",
        verbs: &["post"],
        path: "/",
    },
    Action {
        method: "show",
        verbs: &["get"],
        path: "/{id}",
    },
    Action {
        method: "update",
        verbs: &["put", "patch"],
        path: "/{id}",
    },
    Action {
        method: "destroy",
        verbs: &["delete"],
        path: "/{id}",
    },
];

/// Impl-block form: generates `routes`, mounting each `pub` convention
/// method that exists. Non-convention and private methods are untouched.
fn expand_impl(crate_path: &syn::Path, input: syn::ItemImpl) -> TokenStream {
    if input.trait_.is_some() {
        return syn::Error::new(
            input.span(),
            "#[controller] on a trait impl is not supported; use it on the inherent impl block",
        )
        .to_compile_error()
        .into();
    }

    let mut mounts = Vec::new();
    for action in ACTIONS {
        let Some(found) = find_action(&input, action.method) else {
            continue;
        };
        let (argument_types, is_async) = match check_action(found, action.method) {
            Ok(valid) => valid,
            Err(error) => return error.to_compile_error().into(),
        };
        for verb in action.verbs {
            mounts.push(mount_route(
                crate_path,
                action,
                verb,
                &argument_types,
                is_async,
            ));
        }
    }

    let (impl_generics, _, where_clause) = input.generics.split_for_impl();
    let self_ty = &input.self_ty;
    // `mut` only when mounts exist, so controllers without convention
    // methods (helpers + hand-written routes) compile warning-free.
    let mutability = if mounts.is_empty() {
        quote! {}
    } else {
        quote! { mut }
    };
    quote! {
        #input

        impl #impl_generics #self_ty #where_clause {
            #[doc = "Builds this controller's RESTful routes (relative paths)."]
            #[doc = ""]
            #[doc = "Mount under a prefix with `routes!` or `Router::nest`. Only the"]
            #[doc = "`pub` convention methods present on the impl become routes; see"]
            #[doc = "the `controller` macro documentation for the method table."]
            pub fn routes<__LumosState>(
                ctrl: ::std::sync::Arc<Self>,
            ) -> #crate_path::Router<__LumosState>
            where
                __LumosState: Clone + Send + Sync + 'static,
            {
                let _ = &ctrl;
                let #mutability __router = #crate_path::Router::new();
                #(#mounts)*
                __router
            }
        }
    }
    .into()
}

/// Finds a `pub` inherent method by name. Private methods are helpers, not
/// actions, and are deliberately ignored.
fn find_action<'a>(input: &'a syn::ItemImpl, name: &str) -> Option<&'a syn::ImplItemFn> {
    input.items.iter().find_map(|item| match item {
        ImplItem::Fn(found)
            if found.sig.ident == name && matches!(found.vis, syn::Visibility::Public(_)) =>
        {
            Some(found)
        }
        _ => None,
    })
}

/// Validates an action signature, returning its extractor types and whether
/// the method is `async`.
fn check_action(method: &syn::ImplItemFn, name: &str) -> syn::Result<(Vec<syn::Type>, bool)> {
    if !method.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            format!("controller action `{name}` must not be generic"),
        ));
    }
    let mut inputs = method.sig.inputs.iter();
    let receiver_ok =
        matches!(inputs.next(), Some(FnArg::Receiver(receiver)) if receiver.reference.is_some());
    if !receiver_ok {
        return Err(syn::Error::new(
            method.sig.ident.span(),
            format!("controller action `{name}` must take `&self`"),
        ));
    }
    let mut argument_types = Vec::with_capacity(inputs.len());
    for input in inputs {
        let FnArg::Typed(typed) = input else {
            return Err(syn::Error::new(
                input.span(),
                format!("controller action `{name}` has an unsupported parameter"),
            ));
        };
        argument_types.push((*typed.ty).clone());
    }
    Ok((argument_types, method.sig.asyncness.is_some()))
}

/// Generates one `.route(path, verb(handler))` mount. The handler clones the
/// `Arc` per request so the closure stays `Fn + Clone` as axum requires
/// (an `async move` capturing the `Arc` directly would be `FnOnce`).
fn mount_route(
    crate_path: &syn::Path,
    action: &Action,
    verb: &str,
    argument_types: &[syn::Type],
    is_async: bool,
) -> proc_macro2::TokenStream {
    let method = format_ident!("{}", action.method);
    let verb = format_ident!("{verb}");
    let path = action.path;
    let parameters: Vec<proc_macro2::TokenStream> = argument_types
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            let name = format_ident!("__arg{index}");
            quote! { #name: #ty }
        })
        .collect();
    let arguments: Vec<proc_macro2::TokenStream> = argument_types
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let name = format_ident!("__arg{index}");
            quote! { #name }
        })
        .collect();
    // Both forms produce an `async` handler: axum requires handlers to
    // return futures, so sync actions are wrapped (their return values must
    // still implement `IntoResponse`, exactly like async ones).
    let call = if is_async {
        quote! {
            move |#(#parameters),*| {
                let __ctrl = ::std::sync::Arc::clone(&__ctrl);
                async move { __ctrl.#method(#(#arguments),*).await }
            }
        }
    } else {
        quote! {
            move |#(#parameters),*| {
                let __ctrl = ::std::sync::Arc::clone(&__ctrl);
                async move { __ctrl.#method(#(#arguments),*) }
            }
        }
    };
    quote! {
        {
            let __ctrl = ::std::sync::Arc::clone(&ctrl);
            let __handler = #call;
            __router = __router.route(#path, #crate_path::#verb(__handler));
        }
    }
}

/// Derives the [`Model`](::rusticate::Model) trait: metadata, row hydration,
/// changesets, timestamps, relations, and eager loading.
///
/// Container options (`#[model(...)]` on the struct): `table` (required),
/// `primary_key` (default `"id"`), `timestamps` / `soft_deletes` (bool or
/// bare flag), `appends = ["method", ...]` (extra `to_value` entries).
///
/// Field options: `id`, `auto_increment`, `hidden`, `casts = "json" |
/// "string" | "bool" | "datetime"`, `has_many = "Post"` / `belongs_to =
/// "Team"` (with `foreign_key` / `local_key` overrides), and the
/// `created_at` / `updated_at` / `deleted_at` markers (timestamps and soft
/// deletes assume conventionally-named fields when markers are absent).
///
/// Every model owner depends on `rusticate` directly (it is standalone by
/// design), so generated paths use `::rusticate` unconditionally.
///
/// # Examples
///
/// ```ignore
/// use rusticate::{BelongsTo, HasMany, Model};
///
/// #[derive(Model)]
/// #[model(table = "users", timestamps = true)]
/// pub struct User {
///     #[model(id, auto_increment)]
///     pub id: i64,
///     pub name: String,
///     #[model(has_many = "Post")]
///     pub posts: HasMany<Post>,
///     #[model(created_at)]
///     pub created_at: chrono::DateTime<chrono::Utc>,
///     #[model(updated_at)]
///     pub updated_at: chrono::DateTime<chrono::Utc>,
/// }
/// ```
#[proc_macro_derive(Model, attributes(model))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as syn::DeriveInput);
    model::expand(&parsed)
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}

/// Turns `scope_*` associated functions into chainable query methods.
///
/// For each `pub` scope in the impl block, generates a `{Model}Scopes` trait
/// (import it to use): `scope_active` becomes `.active()`. Scopes must be
/// associated functions taking and returning a `Query`; names colliding with
/// `Query` methods are compile errors. Plain `Model::scope_x(query)` calls
/// work with or without this macro.
///
/// Generated paths point at `::rusticate` by default; override with
/// `#[scopes(crate = "...")]` in exotic layouts.
///
/// # Examples
///
/// ```ignore
/// use rusticate::{scopes, Model, Query};
///
/// #[scopes]
/// impl User {
///     pub fn scope_active(query: Query<User>) -> Query<User> {
///         query.where_eq("active", true)
///     }
/// }
///
/// // The generated `UserScopes` trait lives beside the impl: import it
/// // (`use crate::UserScopes;`) and chain `.active()` on any `Query<User>`.
/// ```
#[proc_macro_attribute]
pub fn scopes(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(item as syn::ItemImpl);
    scopes::expand(attribute, parsed)
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}
