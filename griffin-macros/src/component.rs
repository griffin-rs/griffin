//! `#[component]`: makes a function with typed parameters what a capitalised tag in a
//! template calls (a PascalCase tag tells it from an HTML tag), by writing the `griffin_web::template::Component` a
//! Component author could write by hand (every macro lowers to a public API).

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Error, Expr, FnArg, ItemFn, Lifetime, Meta, Pat, Result, TypeReference};

/// The function, and beside it a struct of its name holding the attributes, with a
/// method to set each one and a `Component` implementation that calls the function.
pub fn expand(mut function: ItemFn) -> Result<TokenStream> {
    let signature = &function.sig;
    if !signature.ident.to_string().starts_with(char::is_uppercase) {
        let message = "a Component is called by a capitalised tag, so its name has to be \
                       capitalised too: `Badge`, not `badge`";
        return Err(Error::new_spanned(&signature.ident, message));
    }
    if !signature.generics.params.is_empty() {
        // No generic Components. Carry the function's generics onto the
        // struct and its impls when one is needed.
        return Err(Error::new_spanned(
            &signature.generics,
            "a Component cannot have generic parameters yet",
        ));
    }

    // The struct holds what the function borrows, so every lifetime the parameters
    // leave out gets this one name. It is only declared if one of them needs it. The
    // function gets it too: what it passes to a slot must be what the slot takes.
    let mut named = Named(false);
    let (mut fields, mut initials, mut setters, mut arguments, mut required) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut required_slots = Vec::new();
    let mut global = None;
    let (name, vis) = (signature.ident.clone(), function.vis.clone());
    for input in &mut function.sig.inputs {
        let FnArg::Typed(input) = input else {
            let message = "a Component is a plain function: it has no `self`";
            return Err(Error::new_spanned(input, message));
        };
        let Pat::Ident(pattern) = &*input.pat else {
            let message = "expected a name: each parameter of a Component is an attribute";
            return Err(Error::new_spanned(&input.pat, message));
        };
        let attribute = &pattern.ident;
        named.visit_type_mut(&mut input.ty);
        let ty = input.ty.clone();
        // `#[default]`, `#[default(expression)]`, `#[global]` and `#[slot]` are this
        // macro's own, and must be gone from the function it leaves behind.
        let (mut default, mut is_global, mut is_slot) = (None, false, false);
        for attr in std::mem::take(&mut input.attrs) {
            if attr.path().is_ident("default") {
                default = Some(default_value(&attr)?);
            } else if attr.path().is_ident("global") {
                is_global = true;
            } else if attr.path().is_ident("slot") {
                is_slot = true;
            } else {
                input.attrs.push(attr);
            }
        }
        if is_global {
            // The parameter that takes the extra HTML attributes is not an attribute.
            if global.replace(attribute.clone()).is_some() {
                let message = "a Component has one `#[global]` parameter";
                return Err(Error::new_spanned(attribute, message));
            }
            fields.push(quote!(#attribute: #ty));
            initials.push(quote!(#attribute: ::std::default::Default::default()));
            arguments.push(quote!(self.#attribute));
            continue;
        }

        let text = attribute.unraw().to_string();
        if is_slot {
            // A slot starts empty, and a tag adds an entry each time it gives it.
            let doc = format!("Gives the slot `{text}`: `add` pushes an entry onto it.");
            if default.is_none() {
                required_slots.push(text);
            }
            let initial = default.unwrap_or(quote!(::std::default::Default::default()));
            fields.push(quote!(#attribute: #ty));
            initials.push(quote!(#attribute: #initial));
            arguments.push(quote!(self.#attribute));
            setters.push(quote_spanned! {attribute.span()=>
                #[doc = #doc]
                #vis fn #attribute(mut self, add: impl ::std::ops::FnOnce(&mut #ty)) -> Self {
                    add(&mut self.#attribute);
                    self
                }
            });
            continue;
        }
        let doc = format!("Gives the attribute `{text}`.");
        // What the field holds and starts as, what the method stores in it and what
        // the function is passed. A required attribute has no value until the tag
        // gives it one.
        let (field, initial, given, argument) = match default {
            Some(default) => (
                quote!(#ty),
                default,
                quote!(#attribute),
                quote!(self.#attribute),
            ),
            None => {
                let missing = format!("the attribute `{text}` of `<{name}>` was not given");
                required.push(text);
                (
                    quote!(::std::option::Option<#ty>),
                    quote!(::std::option::Option::None),
                    quote!(::std::option::Option::Some(#attribute)),
                    quote!(self.#attribute.expect(#missing)),
                )
            }
        };
        fields.push(quote!(#attribute: #field));
        initials.push(quote!(#attribute: #initial));
        arguments.push(argument);
        // At the parameter, which is where rustc then says the type was declared.
        setters.push(quote_spanned! {attribute.span()=>
            #[doc = #doc]
            #vis fn #attribute(mut self, #attribute: #ty) -> Self {
                self.#attribute = #given;
                self
            }
        });
    }

    let lifetime = named.0.then(Named::lifetime);
    let generics = quote!(<#lifetime>);
    function.sig.generics = syn::parse_quote!(#generics);
    let global = global.map(|global| {
        quote_spanned! {global.span()=>
            impl #generics ::griffin_web::template::GlobalAttributes for #name #generics {
                fn attribute(
                    mut self,
                    name: &'static str,
                    value: impl ::std::convert::Into<::griffin_web::template::AttributeValue>,
                ) -> Self {
                    self.#global.push((name, value.into()));
                    self
                }
            }
        }
    });
    let doc = format!("The attributes of `<{name}>`, what the tag calls in a template.");
    Ok(quote! {
        #[allow(non_snake_case)]
        #function

        #[doc = #doc]
        #vis struct #name #generics {
            #(#fields,)*
        }

        impl #generics ::std::default::Default for #name #generics {
            fn default() -> Self {
                #name { #(#initials,)* }
            }
        }

        // An optional attribute that no tag gives is not dead code.
        #[allow(dead_code)]
        impl #generics #name #generics {
            #(#setters)*
        }

        impl #generics ::griffin_web::template::Component for #name #generics {
            const REQUIRED: &'static [&'static str] = &[#(#required),*];
            const REQUIRED_SLOTS: &'static [&'static str] = &[#(#required_slots),*];

            fn render(self) -> ::griffin_web::template::Rendered {
                #name(#(#arguments),*)
            }
        }

        #global
    })
}

/// The value an optional attribute has when the tag leaves it out: the expression in
/// `#[default(expression)]`, or for a bare `#[default]` its type's own default.
fn default_value(attr: &Attribute) -> Result<TokenStream> {
    Ok(match &attr.meta {
        Meta::Path(_) => quote_spanned!(attr.span()=> ::std::default::Default::default()),
        _ => attr.parse_args::<Expr>()?.into_token_stream(),
    })
}

/// Names the lifetimes a type leaves out, `&T` and `'_`, and says whether it did.
// One lifetime for all of them, also inside `dyn Fn(&T)`, where it means
// something narrower than the higher-ranked one Rust would take it for.
struct Named(bool);

impl Named {
    fn lifetime() -> Lifetime {
        Lifetime::new("'a", Span::call_site())
    }
}

impl VisitMut for Named {
    fn visit_type_reference_mut(&mut self, reference: &mut TypeReference) {
        if reference.lifetime.is_none() {
            (reference.lifetime, self.0) = (Some(Named::lifetime()), true);
        }
        visit_mut::visit_type_reference_mut(self, reference);
    }

    fn visit_lifetime_mut(&mut self, lifetime: &mut Lifetime) {
        if lifetime.ident == "_" {
            (*lifetime, self.0) = (Named::lifetime(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    //! A snapshot of the expansion, which pins that it is nothing but the documented
    //! `griffin_web::template::Component` API (every macro lowers to a public API).

    use super::expand;

    #[test]
    fn a_function_with_a_required_an_optional_and_a_global_parameter() {
        let function = syn::parse_str(
            "pub fn Badge(label: &str, #[default(0)] count: u32, #[global] rest: Attributes) \
             -> Rendered { body() }",
        );
        assert_eq!(
            expand(function.unwrap()).unwrap().to_string(),
            "# [allow (non_snake_case)] \
             pub fn Badge < 'a > (label : & 'a str , count : u32 , rest : Attributes) -> Rendered { body () } \
             # [doc = \"The attributes of `<Badge>`, what the tag calls in a template.\"] \
             pub struct Badge < 'a > { \
                label : :: std :: option :: Option < & 'a str > , \
                count : u32 , \
                rest : Attributes , \
             } \
             impl < 'a > :: std :: default :: Default for Badge < 'a > { \
                fn default () -> Self { \
                    Badge { \
                        label : :: std :: option :: Option :: None , \
                        count : 0 , \
                        rest : :: std :: default :: Default :: default () , \
                    } \
                } \
             } \
             # [allow (dead_code)] \
             impl < 'a > Badge < 'a > { \
                # [doc = \"Gives the attribute `label`.\"] \
                pub fn label (mut self , label : & 'a str) -> Self { \
                    self . label = :: std :: option :: Option :: Some (label) ; self \
                } \
                # [doc = \"Gives the attribute `count`.\"] \
                pub fn count (mut self , count : u32) -> Self { self . count = count ; self } \
             } \
             impl < 'a > :: griffin_web :: template :: Component for Badge < 'a > { \
                const REQUIRED : & 'static [& 'static str] = & [\"label\"] ; \
                const REQUIRED_SLOTS : & 'static [& 'static str] = & [] ; \
                fn render (self) -> :: griffin_web :: template :: Rendered { \
                    Badge (\
                        self . label . expect (\"the attribute `label` of `<Badge>` was not given\") , \
                        self . count , \
                        self . rest\
                    ) \
                } \
             } \
             impl < 'a > :: griffin_web :: template :: GlobalAttributes for Badge < 'a > { \
                fn attribute (\
                    mut self , \
                    name : & 'static str , \
                    value : impl :: std :: convert :: Into < :: griffin_web :: template :: AttributeValue > ,\
                ) -> Self { \
                    self . rest . push ((name , value . into ())) ; self \
                } \
             }"
        );
    }

    #[test]
    fn a_function_with_a_required_and_an_optional_slot() {
        let function = syn::parse_str(
            "fn Table(#[slot] col: SlotEntries<'_, &User, Col<'_>>, \
             #[slot] #[default] inner_block: SlotEntries<'_>) -> Rendered { body() }",
        );
        assert_eq!(
            expand(function.unwrap()).unwrap().to_string(),
            "# [allow (non_snake_case)] \
             fn Table < 'a > (\
                col : SlotEntries < 'a , & 'a User , Col < 'a > > , \
                inner_block : SlotEntries < 'a >\
             ) -> Rendered { body () } \
             # [doc = \"The attributes of `<Table>`, what the tag calls in a template.\"] \
             struct Table < 'a > { \
                col : SlotEntries < 'a , & 'a User , Col < 'a > > , \
                inner_block : SlotEntries < 'a > , \
             } \
             impl < 'a > :: std :: default :: Default for Table < 'a > { \
                fn default () -> Self { \
                    Table { \
                        col : :: std :: default :: Default :: default () , \
                        inner_block : :: std :: default :: Default :: default () , \
                    } \
                } \
             } \
             # [allow (dead_code)] \
             impl < 'a > Table < 'a > { \
                # [doc = \"Gives the slot `col`: `add` pushes an entry onto it.\"] \
                fn col (\
                    mut self , \
                    add : impl :: std :: ops :: FnOnce (& mut SlotEntries < 'a , & 'a User , Col < 'a > >)\
                ) -> Self { \
                    add (& mut self . col) ; self \
                } \
                # [doc = \"Gives the slot `inner_block`: `add` pushes an entry onto it.\"] \
                fn inner_block (mut self , add : impl :: std :: ops :: FnOnce (& mut SlotEntries < 'a >)) -> Self { \
                    add (& mut self . inner_block) ; self \
                } \
             } \
             impl < 'a > :: griffin_web :: template :: Component for Table < 'a > { \
                const REQUIRED : & 'static [& 'static str] = & [] ; \
                const REQUIRED_SLOTS : & 'static [& 'static str] = & [\"col\"] ; \
                fn render (self) -> :: griffin_web :: template :: Rendered { \
                    Table (self . col , self . inner_block) \
                } \
             }"
        );
    }
}
