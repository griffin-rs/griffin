//! `routes!`: the route table (one `routes!` table), lowered to the `Scope` builder of
//! `griffin_web::router` that a developer could call by hand (every macro lowers to a public API).

use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned};
use std::collections::HashMap;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Attribute, Error, Expr, Ident, LitStr, Result, Signature, Token, Type, Visibility, braced,
    bracketed,
};

/// The verbs of a route line, each the name in capitals of a function of
/// `axum::routing`.
const VERBS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// A route table: the function it becomes, then what the root Scope holds.
struct Table {
    /// The attributes of the function, which its doc comment is one of.
    attributes: Vec<Attribute>,
    visibility: Visibility,
    signature: Signature,
    items: Vec<Item>,
}

/// One thing a Scope of the table holds.
enum Item {
    PipeThrough(Vec<Expr>),
    Layout(Expr),
    Route(Route),
    Scope(LitStr, Vec<Item>),
    /// `live_session admin { .. }`: routes with no prefix of their own, whose
    /// LiveViews are in one live session.
    LiveSession(Ident, Vec<Item>),
}

/// A route line: `GET "/users/{id: u32}" => show_user as user;`.
struct Route {
    /// A verb, or `LIVE`.
    verb: Ident,
    path: LitStr,
    /// The handler, or the LiveView of a `LIVE` route.
    target: syn::Path,
    name: Option<Ident>,
}

impl Parse for Table {
    fn parse(input: ParseStream) -> Result<Table> {
        let attributes = input.call(Attribute::parse_outer)?;
        let visibility = input.parse()?;
        if !input.peek(Token![fn]) {
            let message = "a route table starts with the function it becomes, \
                           as in `pub fn router() -> Router<AppState>;`";
            return Err(input.error(message));
        }
        let signature = input.parse()?;
        input.parse::<Token![;]>()?;
        let items = items(input)?;
        Ok(Table {
            attributes,
            visibility,
            signature,
            items,
        })
    }
}

/// What is between the braces of a Scope, or the whole table after its function.
fn items(input: ParseStream) -> Result<Vec<Item>> {
    let expected = |found: &dyn std::fmt::Display| {
        format!(
            "expected a route, a `scope`, a `live_session`, a `pipe_through` or a `layout`, \
             found `{found}`: \
             a route starts with `LIVE` or a verb in capitals, one of {}",
            VERBS.join(", ")
        )
    };
    let mut items = Vec::new();
    while !input.is_empty() {
        let Ok(word) = input.parse::<Ident>() else {
            let found: proc_macro2::TokenTree = input.parse()?;
            return Err(Error::new(found.span(), expected(&found)));
        };
        items.push(match word.to_string().as_str() {
            "scope" => {
                let prefix = input.parse()?;
                let body;
                braced!(body in input);
                Item::Scope(prefix, self::items(&body)?)
            }
            "live_session" => {
                let name = input.parse().map_err(|error: Error| {
                    let message =
                        "a live session is given a name, as in `live_session admin { .. }`";
                    Error::new(error.span(), message)
                })?;
                let body;
                braced!(body in input);
                Item::LiveSession(name, self::items(&body)?)
            }
            "pipe_through" => {
                let list;
                bracketed!(list in input);
                let pipelines = list.parse_terminated(Expr::parse, Token![,])?;
                input.parse::<Token![;]>()?;
                Item::PipeThrough(pipelines.into_iter().collect())
            }
            "layout" => {
                let layout = input.parse()?;
                input.parse::<Token![;]>()?;
                Item::Layout(layout)
            }
            verb if verb == "LIVE" || VERBS.contains(&verb) => {
                let path = input.parse()?;
                input.parse::<Token![=>]>()?;
                let target = input.parse().map_err(|error: Error| {
                    let message = "expected the path of a handler function or of a LiveView, \
                                   as in `users::show`";
                    Error::new(error.span(), message)
                })?;
                let name = match input.parse::<Option<Token![as]>>()? {
                    Some(_) => Some(input.parse()?),
                    None => None,
                };
                input.parse::<Token![;]>()?;
                Item::Route(Route {
                    verb: word,
                    path,
                    target,
                    name,
                })
            }
            _ => return Err(Error::new(word.span(), expected(&word))),
        });
    }
    Ok(items)
}

/// The function, the Path helpers and the listing a table becomes.
pub fn expand(input: TokenStream) -> Result<TokenStream> {
    let table: Table = syn::parse2(input)?;
    let mut lowering = Lowering::default();
    let root = lowering.scope(&quote!("/"), &table.items, &Place::default());
    if let Some(error) = lowering.errors.into_iter().reduce(|mut errors, error| {
        errors.combine(error);
        errors
    }) {
        return Err(error);
    }

    let router = quote!(::griffin_web::axum::Router::new().merge(#root));
    // The socket is given the pages with every Pipeline already on them.
    let body = if lowering.live {
        quote! {
            let pages = #router;
            pages.clone().route("/live/websocket", ::griffin_web::live::live_socket(pages))
        }
    } else {
        router
    };
    let (visibility, signature) = (&table.visibility, &table.signature);
    let attributes = &table.attributes;
    let (helpers, list) = (&lowering.helpers, &lowering.list);
    Ok(quote! {
        #(#attributes)*
        #visibility #signature {
            #body
        }

        /// The route table: its listing, and a Path helper for each route it names.
        #visibility struct Routes;

        impl Routes {
            /// Every route of the table, in the order written.
            pub const LIST: &'static [::griffin_web::router::Route] = &[#(#list),*];

            #(#helpers)*
        }
    })
}

/// A parameter of a path: `{id: u32}`, or `{*rest: String}` for the rest of the path.
#[derive(Clone)]
struct Parameter {
    name: Ident,
    ty: Type,
    rest: bool,
}

/// A path of the table as axum takes it, which is without the types, and its
/// parameters. Their tokens have the span of the path: no stable span is inside it.
fn path(literal: &LitStr) -> Result<(String, Vec<Parameter>)> {
    let (text, span) = (literal.value(), literal.span());
    if !text.starts_with('/') {
        return Err(Error::new(span, "a path starts with `/`"));
    }
    let (mut path, mut parameters) = (String::new(), Vec::new());
    let mut left = text.as_str();
    while let Some(open) = left.find('{') {
        let close = left[open..].find('}').map_or(left.len(), |at| open + at);
        let written = &left[(open + 1).min(close)..close];
        let (rest, written) = match written.strip_prefix('*') {
            Some(written) => (true, written),
            None => (false, written),
        };
        let parameter = written.split_once(':').and_then(|(name, ty)| {
            let name = syn::parse_str::<Ident>(name).ok()?;
            let ty = syn::parse2(respan(ty.parse().ok()?, span)).ok()?;
            let name = Ident::new(&name.to_string(), span);
            Some(Parameter { name, ty, rest })
        });
        let Some(parameter) = parameter.filter(|_| close < left.len()) else {
            let message = format!(
                "a path parameter is written with its type, as in `{{id: u32}}`, \
                 which `{}` is not",
                &left[open..(close + 1).min(left.len())]
            );
            return Err(Error::new(span, message));
        };
        let star = if rest { "*" } else { "" };
        path.push_str(&format!("{}{{{star}{}}}", &left[..open], parameter.name));
        parameters.push(parameter);
        left = &left[close + 1..];
    }
    path.push_str(left);
    Ok((path, parameters))
}

/// The tokens, each with the span given.
fn respan(tokens: TokenStream, span: Span) -> TokenStream {
    let respan = |token| match token {
        TokenTree::Group(group) => {
            let mut inner = Group::new(group.delimiter(), respan(group.stream(), span));
            inner.set_span(span);
            TokenTree::Group(inner)
        }
        mut token => {
            token.set_span(span);
            token
        }
    };
    tokens.into_iter().map(respan).collect()
}

/// The path as axum takes it, with `parameter` in the place of each parameter and
/// `rest` in the place of one that takes the rest of the path.
fn placeholders(path: &str, parameter: &str, rest: &str) -> String {
    let mut pieces = path.split('{');
    let mut filled = pieces.next().unwrap_or_default().to_owned();
    for piece in pieces {
        let (name, after) = piece.split_once('}').unwrap_or((piece, ""));
        filled.push_str(if name.starts_with('*') {
            rest
        } else {
            parameter
        });
        filled.push_str(after);
    }
    filled
}

/// Whether the type is written `String`. An alias of it is not seen, as a macro only
/// has the tokens.
fn is_string(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    (path.path.segments.last()).is_some_and(|last| last.ident == "String")
}

/// Where a Scope or a route is in the table: under the prefixes of the Scopes around
/// it, with their parameters.
#[derive(Default)]
struct Place {
    /// As axum takes it, and with no `/` at its end.
    prefix: String,
    parameters: Vec<Parameter>,
}

impl Place {
    /// The place of what is at `path` in this one, and that path as axum takes it.
    fn at(&self, path: &LitStr) -> Result<(LitStr, Place)> {
        let (own, added) = self::path(path)?;
        let prefix = format!("{}{}", self.prefix, own.trim_end_matches('/'));
        let mut parameters = self.parameters.clone();
        for parameter in added {
            if parameters.iter().any(|other| other.name == parameter.name) {
                let message = format!(
                    "the path parameter `{}` is already one of this path, \
                     or of the Scopes around it",
                    parameter.name
                );
                return Err(Error::new(path.span(), message));
            }
            parameters.push(parameter);
        }
        Ok((LitStr::new(&own, path.span()), Place { prefix, parameters }))
    }
}

/// What is learnt of a table while its Scopes are lowered.
#[derive(Default)]
struct Lowering {
    errors: Vec<Error>,
    /// Whether a route is a LiveView's, which needs the socket.
    live: bool,
    /// The Path helper of each named route.
    helpers: Vec<TokenStream>,
    /// The `Route` that lists each route.
    list: Vec<TokenStream>,
    /// What each method and path is routed to. The path is without the names of its
    /// parameters, which do not tell two routes apart.
    targets: HashMap<(String, String), String>,
    /// The route of each name.
    names: HashMap<String, String>,
    /// The live sessions seen, and whether the Scope being lowered is in one.
    live_sessions: std::collections::HashSet<String>,
    in_live_session: bool,
}

impl Lowering {
    /// The builder calls that make the Scope at `place`, whose own prefix is `prefix`,
    /// holding `items`.
    fn scope(&mut self, prefix: &TokenStream, items: &[Item], place: &Place) -> TokenStream {
        let (mut calls, mut has_layout) = (Vec::new(), false);
        for item in items {
            let call = match item {
                Item::PipeThrough(pipelines) => Ok(quote!(#(.pipe_through(#pipelines))*)),
                Item::Layout(layout) if has_layout => {
                    let message = "a Scope has one layout, and this is its second";
                    Err(Error::new_spanned(layout, message))
                }
                Item::Layout(layout) => {
                    has_layout = true;
                    Ok(quote!(.layout(#layout)))
                }
                Item::Route(route) => self.route(route, place),
                Item::Scope(prefix, items) => place.at(prefix).map(|(prefix, place)| {
                    let scope = self.scope(&quote!(#prefix), items, &place);
                    quote!(.scope(#scope))
                }),
                Item::LiveSession(name, _) if self.in_live_session => {
                    let message = "a live session cannot be inside another: a live \
                                   session is a group of routes, and groups are side by side";
                    Err(Error::new(name.span(), message))
                }
                Item::LiveSession(name, _) if !self.live_sessions.insert(name.to_string()) => {
                    let message = format!(
                        "the live session `{name}` is already in this table: its routes \
                         go in one block, or the two blocks are one live session by accident"
                    );
                    Err(Error::new(name.span(), message))
                }
                Item::LiveSession(name, items) => {
                    self.in_live_session = true;
                    let scope = self.scope(&quote!("/"), items, place);
                    self.in_live_session = false;
                    let name = name.to_string();
                    Ok(quote!(.scope(#scope.live_session(#name))))
                }
            };
            match call {
                Ok(call) => calls.push(call),
                Err(error) => self.errors.push(error),
            }
        }
        quote!(::griffin_web::router::Scope::new(#prefix) #(#calls)*)
    }

    /// The `.route(..)` call of a route line in the Scope at `place`.
    fn route(&mut self, route: &Route, place: &Place) -> Result<TokenStream> {
        let Route {
            verb, path, target, ..
        } = route;
        let written = path;
        let (path, place) = place.at(path)?;
        let full = match place.prefix.as_str() {
            "" => "/",
            prefix => prefix,
        };
        let live = verb == "LIVE";
        let target_name = quote!(#target).to_string().replace(' ', "");

        // A LiveView is reached by a GET.
        let method = if live { "GET" } else { &verb.to_string() };
        let route_name = format!("{method} {full}");
        let key = (method.to_owned(), placeholders(full, "{}", "{*}"));
        if let Some(first) = self.targets.get(&key) {
            let message = format!("duplicate route: `{route_name}` is already routed to `{first}`");
            return Err(Error::new_spanned(quote!(#verb #written), message));
        }
        self.targets.insert(key, target_name.clone());

        let name = match &route.name {
            Some(name) => {
                if let Some(first) = self.names.get(&name.to_string()) {
                    let message =
                        format!("duplicate route name: `{name}` is already the name of `{first}`");
                    return Err(Error::new(name.span(), message));
                }
                self.names.insert(name.to_string(), route_name);
                let helper = helper(name, verb, full, &place.parameters);
                self.helpers.push(helper);
                let name = name.to_string();
                quote!(::core::option::Option::Some(#name))
            }
            None => quote!(::core::option::Option::None),
        };
        let method = verb.to_string();
        self.list.push(quote! {
            ::griffin_web::router::Route {
                method: #method,
                path: #full,
                name: #name,
                target: #target_name,
            }
        });

        // The types of the path's parameters, as a handler takes them.
        let types = place.parameters.iter().map(|parameter| &parameter.ty);
        let parameters = match place.parameters.len() {
            1 => quote!(#(#types)*),
            _ => quote!((#(#types),*)),
        };
        // Spanned at the target, where rustc then reports one that does not take them.
        let handlers = if live {
            self.live = true;
            quote_spanned! {target.span()=>
                ::griffin_web::router::live_takes_path::<#target, #parameters, _>()
            }
        } else {
            let function = format_ident!("{}", verb.to_string().to_lowercase(), span = verb.span());
            let handler = if place.parameters.is_empty() {
                quote!(#target)
            } else {
                quote_spanned! {target.span()=>
                    ::griffin_web::router::takes_path::<#parameters, _, _>(#target)
                }
            };
            quote!(::griffin_web::axum::routing::#function(#handler))
        };
        Ok(quote!(.route(#path, #handlers)))
    }
}

/// The Path helper `name` of the route `verb path`.
fn helper(name: &Ident, verb: &Ident, path: &str, parameters: &[Parameter]) -> TokenStream {
    let arguments = parameters.iter().map(|Parameter { name, ty, .. }| {
        if is_string(ty) {
            quote_spanned!(ty.span()=> #name: &str)
        } else {
            quote!(#name: #ty)
        }
    });
    let body = if parameters.is_empty() {
        quote!(::std::string::String::from(#path))
    } else {
        let format = placeholders(path, "{}", "{}");
        let values = parameters.iter().map(|Parameter { name, rest, .. }| {
            let encode = if *rest {
                quote!(encode_path)
            } else {
                quote!(encode_segment)
            };
            quote!(::griffin_web::router::#encode(&#name))
        });
        quote!(::std::format!(#format, #(#values),*))
    };
    let doc = format!("The path of `{verb} {path}`.");
    quote! {
        #[doc = #doc]
        pub fn #name(#(#arguments),*) -> ::std::string::String {
            #body
        }
    }
}

#[cfg(test)]
mod tests {
    //! A snapshot of the expansion, which pins that it is nothing but the documented
    //! builder API of `griffin_web::router` and `griffin_web::live` (every macro lowers to a public API).

    use super::expand;

    #[test]
    fn a_table_with_nested_scopes_a_pipeline_layouts_verb_routes_and_a_live_route() {
        let table = r#"
            /// The router.
            pub fn router(session: SessionLayer) -> Router<AppState>;

            scope "/" {
                pipe_through [browser(session), log()];
                layout site;

                GET "/" => home as home;
                LIVE "/counter" => Counter;

                scope "/users/{user: u32}" {
                    layout |page, _flash| page;

                    GET "/" => users::show as user;
                    PUT "/" => users::rename;
                    GET "/posts/{slug: String}" => posts::show as user_post;
                    LIVE "/files/{*path: String}" => Files as user_file;
                }
            }
        "#;
        assert_eq!(
            expand(table.parse().unwrap()).unwrap().to_string(),
            "# [doc = \" The router.\"] \
            pub fn router (session : SessionLayer) -> Router < AppState > { \
                let pages = :: griffin_web :: axum :: Router :: new () . merge (\
                    :: griffin_web :: router :: Scope :: new (\"/\") \
                    . scope (\
                        :: griffin_web :: router :: Scope :: new (\"/\") \
                        . pipe_through (browser (session)) \
                        . pipe_through (log ()) \
                        . layout (site) \
                        . route (\"/\" , :: griffin_web :: axum :: routing :: get (home)) \
                        . route (\"/counter\" , :: griffin_web :: router :: live_takes_path :: < Counter , () , _ > ()) \
                        . scope (\
                            :: griffin_web :: router :: Scope :: new (\"/users/{user}\") \
                            . layout (| page , _flash | page) \
                            . route (\"/\" , :: griffin_web :: axum :: routing :: get (\
                                :: griffin_web :: router :: takes_path :: < u32 , _ , _ > (users :: show))) \
                            . route (\"/\" , :: griffin_web :: axum :: routing :: put (\
                                :: griffin_web :: router :: takes_path :: < u32 , _ , _ > (users :: rename))) \
                            . route (\"/posts/{slug}\" , :: griffin_web :: axum :: routing :: get (\
                                :: griffin_web :: router :: takes_path :: < (u32 , String) , _ , _ > (posts :: show))) \
                            . route (\"/files/{*path}\" , \
                                :: griffin_web :: router :: live_takes_path :: < Files , (u32 , String) , _ > ())))) ; \
                pages . clone () . route (\"/live/websocket\" , :: griffin_web :: live :: live_socket (pages)) \
            } \
            # [doc = r\" The route table: its listing, and a Path helper for each route it names.\"] \
            pub struct Routes ; \
            impl Routes { \
                # [doc = r\" Every route of the table, in the order written.\"] \
                pub const LIST : & 'static [:: griffin_web :: router :: Route] = & [\
                    :: griffin_web :: router :: Route { \
                        method : \"GET\" , path : \"/\" , \
                        name : :: core :: option :: Option :: Some (\"home\") , target : \"home\" , } , \
                    :: griffin_web :: router :: Route { \
                        method : \"LIVE\" , path : \"/counter\" , \
                        name : :: core :: option :: Option :: None , target : \"Counter\" , } , \
                    :: griffin_web :: router :: Route { \
                        method : \"GET\" , path : \"/users/{user}\" , \
                        name : :: core :: option :: Option :: Some (\"user\") , target : \"users::show\" , } , \
                    :: griffin_web :: router :: Route { \
                        method : \"PUT\" , path : \"/users/{user}\" , \
                        name : :: core :: option :: Option :: None , target : \"users::rename\" , } , \
                    :: griffin_web :: router :: Route { \
                        method : \"GET\" , path : \"/users/{user}/posts/{slug}\" , \
                        name : :: core :: option :: Option :: Some (\"user_post\") , target : \"posts::show\" , } , \
                    :: griffin_web :: router :: Route { \
                        method : \"LIVE\" , path : \"/users/{user}/files/{*path}\" , \
                        name : :: core :: option :: Option :: Some (\"user_file\") , target : \"Files\" , }] ; \
                # [doc = \"The path of `GET /`.\"] \
                pub fn home () -> :: std :: string :: String { \
                    :: std :: string :: String :: from (\"/\") \
                } \
                # [doc = \"The path of `GET /users/{user}`.\"] \
                pub fn user (user : u32) -> :: std :: string :: String { \
                    :: std :: format ! (\"/users/{}\" , :: griffin_web :: router :: encode_segment (& user)) \
                } \
                # [doc = \"The path of `GET /users/{user}/posts/{slug}`.\"] \
                pub fn user_post (user : u32 , slug : & str) -> :: std :: string :: String { \
                    :: std :: format ! (\"/users/{}/posts/{}\" , \
                        :: griffin_web :: router :: encode_segment (& user) , \
                        :: griffin_web :: router :: encode_segment (& slug)) \
                } \
                # [doc = \"The path of `LIVE /users/{user}/files/{*path}`.\"] \
                pub fn user_file (user : u32 , path : & str) -> :: std :: string :: String { \
                    :: std :: format ! (\"/users/{}/files/{}\" , \
                        :: griffin_web :: router :: encode_segment (& user) , \
                        :: griffin_web :: router :: encode_path (& path)) \
                } \
            }"
        );
    }

    #[test]
    fn a_table_without_a_live_route_has_no_socket() {
        let table = r#"fn router() -> Router; GET "/health" => health;"#;
        assert_eq!(
            expand(table.parse().unwrap()).unwrap().to_string(),
            "fn router () -> Router { \
                :: griffin_web :: axum :: Router :: new () . merge (\
                    :: griffin_web :: router :: Scope :: new (\"/\") \
                    . route (\"/health\" , :: griffin_web :: axum :: routing :: get (health))) \
            } \
            # [doc = r\" The route table: its listing, and a Path helper for each route it names.\"] \
            struct Routes ; \
            impl Routes { \
                # [doc = r\" Every route of the table, in the order written.\"] \
                pub const LIST : & 'static [:: griffin_web :: router :: Route] = & [\
                    :: griffin_web :: router :: Route { \
                        method : \"GET\" , path : \"/health\" , \
                        name : :: core :: option :: Option :: None , target : \"health\" , }] ; \
            }"
        );
    }
}
