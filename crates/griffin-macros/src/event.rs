//! `#[derive(Event)]`: maps an enum to the wire names and fields of a LiveView's
//! Events, so a misspelt event is a compile error, by writing the `griffin_web::live::Event` a developer could
//! write by hand (every macro lowers to a public API).

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Error, Field, Fields, GenericArgument, PathArguments, Result, Type};

/// The `Event` implementation: each variant is one Event, named on the wire by its
/// name in snake case, and each of its fields is sent under the field's own name.
pub fn expand(input: DeriveInput) -> Result<TokenStream> {
    let Data::Enum(data) = &input.data else {
        let message = "Event can only be derived for an enum: each variant is one Event";
        return Err(Error::new_spanned(&input.ident, message));
    };

    let (mut decodes, mut encodes) = (Vec::new(), Vec::new());
    let mut errors: Option<Error> = None;
    for variant in &data.variants {
        let fields = match &variant.fields {
            Fields::Named(fields) => fields.named.iter().collect(),
            Fields::Unit => Vec::new(),
            Fields::Unnamed(fields) => {
                let message = "the fields of an Event need names: \
                               each is sent as the attribute `phx-value-<name>`";
                let error = Error::new_spanned(fields, message);
                match &mut errors {
                    Some(errors) => errors.combine(error),
                    None => errors = Some(error),
                }
                continue;
            }
        };
        // A form arrives as one string, so beside it there is no field to read, and
        // two of them would be the same string.
        if let Some(first) = fields.iter().position(|field| is_form(field))
            && fields.len() > 1
        {
            let (offender, message) = match fields.iter().skip(first + 1).find(|f| is_form(f)) {
                Some(second) => (*second, "a variant takes one `#[form]` field at most"),
                None => (
                    *fields
                        .iter()
                        .find(|field| !is_form(field))
                        .expect("another field"),
                    "a variant with a `#[form]` field takes nothing else: the browser sends the \
                     form as one string, with no fields beside it",
                ),
            };
            let error = Error::new_spanned(offender, message);
            match &mut errors {
                Some(errors) => errors.combine(error),
                None => errors = Some(error),
            }
            continue;
        }
        let ident = &variant.ident;
        let name = snake_case(&ident.unraw().to_string());
        let idents: Vec<_> = fields.iter().flat_map(|field| &field.ident).collect();

        // The type is named, so that one with no parser is reported at the field.
        let reads = fields.iter().zip(&idents).map(|(field, ident)| {
            let (wire, ty) = (ident.unraw().to_string(), &field.ty);
            if is_form(field) {
                return quote!(#ident: __griffin_payload.form(#wire)?);
            }
            match optional(ty) {
                Some(ty) => quote!(#ident: __griffin_payload.optional_field::<#ty>(#wire)?),
                None => quote!(#ident: __griffin_payload.field::<#ty>(#wire)?),
            }
        });
        decodes.push(quote! {
            #name => ::core::result::Result::Ok(Self::#ident { #(#reads),* }),
        });

        // The params of a form are what the browser sends: nothing is written for them.
        let sent = fields
            .iter()
            .zip(&idents)
            .filter(|(field, _)| !is_form(field));
        let writes = sent.map(|(field, ident)| {
            let wire = ident.unraw().to_string();
            let push = quote_spanned! {field.ty.span()=>
                __griffin_values.push((#wire, ::std::string::ToString::to_string(#ident)));
            };
            match optional(&field.ty) {
                Some(_) => quote!(if let ::core::option::Option::Some(#ident) = #ident { #push }),
                None => push,
            }
        });
        let bindings = fields.iter().zip(&idents).map(|(field, ident)| {
            if is_form(field) {
                quote!(#ident: _)
            } else {
                quote!(#ident)
            }
        });
        encodes.push(quote! {
            Self::#ident { #(#bindings),* } => {
                #[allow(unused_mut)]
                let mut __griffin_values = ::std::vec::Vec::new();
                #(#writes)*
                (#name, __griffin_values)
            }
        });
    }
    if let Some(errors) = errors {
        return Err(errors);
    }

    // A reference is never without a value to rustc, so an enum with no variants is
    // matched by value.
    let this = if encodes.is_empty() {
        quote!(*self)
    } else {
        quote!(self)
    };
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::griffin_web::live::Event for #name #ty_generics #where_clause {
            fn decode(
                __griffin_name: &str,
                __griffin_payload: &::griffin_web::live::Payload<'_>,
            ) -> ::core::result::Result<Self, ::griffin_web::live::EventError> {
                match __griffin_name {
                    #(#decodes)*
                    _ => ::core::result::Result::Err(::griffin_web::live::EventError::Unknown),
                }
            }

            fn encode(
                &self,
            ) -> (&'static str, ::std::vec::Vec<(&'static str, ::std::string::String)>) {
                match #this {
                    #(#encodes)*
                }
            }
        }
    })
}

/// Whether a field is marked `#[form]`: it takes the params of a form Event.
fn is_form(field: &Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("form"))
}

/// What `Option<T>` is an option of, for a type written as `Option<T>`. An alias of
/// `Option` is not seen, as a macro only has the tokens.
fn optional(ty: &Type) -> Option<&Type> {
    generic_of(ty, "Option")
}

/// `T`, for a type written as `name<T>`, as `Vec<T>`.
pub fn generic_of<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return None;
    };
    match arguments.args.first()? {
        GenericArgument::Type(inner) if last.ident == name => Some(inner),
        _ => None,
    }
}

/// `DeleteUser` as `delete_user`: lowercase, with an underscore before each capital
/// but the first.
fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (index, char) in name.char_indices() {
        if char.is_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.extend(char.to_lowercase());
    }
    snake
}

#[cfg(test)]
mod tests {
    //! A snapshot of the expansion, which pins that it is nothing but the documented
    //! `griffin_web::live::Event` API (every macro lowers to a public API).

    use super::expand;

    #[test]
    fn an_enum_with_a_required_field_an_optional_field_and_no_fields() {
        let input = syn::parse_str(
            "enum TodoEvent { Rename { id: u32, note: Option<String> }, ClearDone }",
        );
        assert_eq!(
            expand(input.unwrap()).unwrap().to_string(),
            "impl :: griffin_web :: live :: Event for TodoEvent { \
                fn decode (\
                    __griffin_name : & str , \
                    __griffin_payload : & :: griffin_web :: live :: Payload < '_ > ,\
                ) -> :: core :: result :: Result < Self , :: griffin_web :: live :: EventError > { \
                    match __griffin_name { \
                        \"rename\" => :: core :: result :: Result :: Ok (Self :: Rename { \
                            id : __griffin_payload . field :: < u32 > (\"id\") ? , \
                            note : __griffin_payload . optional_field :: < String > (\"note\") ? \
                        }) , \
                        \"clear_done\" => :: core :: result :: Result :: Ok (Self :: ClearDone { }) , \
                        _ => :: core :: result :: Result :: Err (:: griffin_web :: live :: EventError :: Unknown) , \
                    } \
                } \
                fn encode (& self ,) -> (& 'static str , :: std :: vec :: Vec < (& 'static str , :: std :: string :: String) >) { \
                    match self { \
                        Self :: Rename { id , note } => { \
                            # [allow (unused_mut)] \
                            let mut __griffin_values = :: std :: vec :: Vec :: new () ; \
                            __griffin_values . push ((\"id\" , :: std :: string :: ToString :: to_string (id))) ; \
                            if let :: core :: option :: Option :: Some (note) = note { \
                                __griffin_values . push ((\"note\" , :: std :: string :: ToString :: to_string (note))) ; \
                            } \
                            (\"rename\" , __griffin_values) \
                        } \
                        Self :: ClearDone { } => { \
                            # [allow (unused_mut)] \
                            let mut __griffin_values = :: std :: vec :: Vec :: new () ; \
                            (\"clear_done\" , __griffin_values) \
                        } \
                    } \
                } \
            }"
        );
    }
}
