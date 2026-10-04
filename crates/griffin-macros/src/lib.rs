//! Procedural macros of the Griffin web framework: <https://github.com/griffin-rs/griffin>.
//!
//! Use them through the crate that re-exports them (`griffin-domain` for the
//! `Changeset` derive, `griffin-web` for `html!`, `#[component]`, `routes!` and the
//! `Event` derive); every macro lowers to a public API documented there.

mod component;
mod event;
mod routes;
mod template;

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Error, Fields, ItemFn, parse_macro_input};

/// Implements `griffin_domain::Cast` for a struct with named fields: each
/// field is parsed from the param of the same name by its type's `FromStr`.
#[proc_macro_derive(Changeset)]
pub fn derive_changeset(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let Data::Struct(syn::DataStruct {
        fields: Fields::Named(fields),
        ..
    }) = &input.data
    else {
        return Error::new_spanned(
            &input.ident,
            "Changeset can only be derived for a struct with named fields",
        )
        .into_compile_error()
        .into();
    };

    let idents: Vec<_> = fields.named.iter().flat_map(|field| &field.ident).collect();
    // Spanned at the field so that an unsupported field type is reported there.
    let casts = fields.named.iter().zip(&idents).map(|(field, ident)| {
        let (ty, name) = (&field.ty, ident.unraw().to_string());
        // A field written as `Vec<T>` is a list: `tags[]` in a form.
        match event::generic_of(ty, "Vec") {
            Some(item) => {
                quote_spanned!(ident.span()=> let #ident = __griffin_input.list::<#item>(#name);)
            }
            None => {
                quote_spanned!(ident.span()=> let #ident = __griffin_input.field::<#ty>(#name);)
            }
        }
    });
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        impl #impl_generics ::griffin_domain::Cast for #name #ty_generics #where_clause {
            fn cast(
                __griffin_input: &mut ::griffin_domain::Input<'_>,
            ) -> ::core::option::Option<Self> {
                #(#casts)*
                ::core::option::Option::Some(Self { #(#idents: #idents?),* })
            }
        }
    }
    .into()
}

/// Implements `griffin_web::live::Event` for an enum: each variant is one Event of a
/// LiveView. It is documented on that trait.
#[proc_macro_derive(Event, attributes(form))]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    event::expand(input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Expands to a `griffin_web::template::Rendered`. The syntax is documented where
/// `griffin-web` re-exports this macro.
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    template::expand(input.into()).into()
}

/// Expands to a `griffin_web::template::Rendered`, as `html!` does, for a template
/// file. It is documented where `griffin-web` re-exports this macro.
#[proc_macro]
pub fn html_file(input: TokenStream) -> TokenStream {
    template::expand_file(input.into()).into()
}

/// Expands to the function that builds an application's router, and to its Path
/// helpers. The table is documented where `griffin-web` re-exports this macro.
#[proc_macro]
pub fn routes(input: TokenStream) -> TokenStream {
    routes::expand(input.into())
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Makes a function a Component, which a capitalised tag in a template calls. It is
/// documented where `griffin-web` re-exports this macro.
#[proc_macro_attribute]
pub fn component(arguments: TokenStream, function: TokenStream) -> TokenStream {
    if let Some(argument) = arguments.into_iter().next() {
        return Error::new(argument.span().into(), "`#[component]` takes no arguments")
            .into_compile_error()
            .into();
    }
    let function = parse_macro_input!(function as ItemFn);
    component::expand(function)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
