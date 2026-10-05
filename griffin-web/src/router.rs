//! Scopes and layouts: the glue Griffin adds to axum's router.
//!
//! The router, the handlers and the extractors are axum's own (Griffin does not duplicate upstream API surface). A
//! [`Scope`] groups routes under a path prefix, a [`Pipeline`] and a layout, and
//! becomes part of an axum [`Router`] with [`Router::merge`]. This is the builder the
//! route table of [`routes!`](crate::routes) lowers to (every macro lowers to a public API), with what its Path
//! helpers and its listing are made of: [`encode_segment`] and [`Route`].
//!
//! A handler returns a template and gets it rendered inside the layout of its Scope.
//! Anything else it returns, such as JSON, is sent as it is.
//!
//! ```
//! use griffin_web::axum::routing::get;
//! use griffin_web::axum::{Json, Router};
//! use griffin_web::router::Scope;
//! use griffin_web::template::{Rendered, SlotEntries};
//! use griffin_web::{component, html};
//!
//! /// A layout is a Component with a default slot.
//! #[component]
//! fn Site(#[slot] inner_block: SlotEntries<'_>) -> Rendered {
//!     html! { <html><body>{inner_block}</body></html> }
//! }
//!
//! async fn home() -> Rendered {
//!     html! { <h1>Welcome</h1> }
//! }
//!
//! async fn health() -> Json<&'static str> {
//!     Json("ok")
//! }
//!
//! let site = Scope::new("/")
//!     .layout(|page, _flash| html! { <Site>{page}</Site> })
//!     .route("/", get(home));
//! let api = Scope::new("/api").route("/health", get(health));
//! let app: Router = Router::new().merge(site).merge(api);
//! ```
//!
//! # Outcomes and errors
//!
//! A handler that can end in several ways returns an enum of its outcomes, and the
//! enum says what response each one is. A test calls the handler and compares the
//! outcome, with nothing rendered. A page is put in the layout whatever its status.
//!
//! ```
//! use griffin_web::axum::http::StatusCode;
//! use griffin_web::axum::response::{IntoResponse, Response};
//! use griffin_web::html;
//! use griffin_web::router::Path;
//!
//! #[derive(Debug, PartialEq)]
//! enum ShowUser {
//!     Found { name: &'static str },
//!     NotFound,
//! }
//!
//! impl IntoResponse for ShowUser {
//!     fn into_response(self) -> Response {
//!         match self {
//!             ShowUser::Found { name } => html! { <h1>{name}</h1> }.into_response(),
//!             ShowUser::NotFound => {
//!                 (StatusCode::NOT_FOUND, html! { <h1>No such user</h1> }).into_response()
//!             }
//!         }
//!     }
//! }
//!
//! async fn show_user(Path(id): Path<u32>) -> ShowUser {
//!     match id {
//!         7 => ShowUser::Found { name: "Ann" },
//!         _ => ShowUser::NotFound,
//!     }
//! }
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! assert_eq!(show_user(Path(8)).await, ShowUser::NotFound);
//! # });
//! ```
//!
//! Errors go the same way. The application has one error type of its own, with a
//! `From` for each error of its Contexts so that a handler can end them with `?`, and
//! its `IntoResponse` is the one place where an error becomes a response (one place maps errors to responses).
//! An error it did not provide for becomes an [`Unexpected`](crate::error::Unexpected),
//! which [`ErrorPages`](crate::error::ErrorPages) gives a page.

use crate::live::{LiveSession, LiveView, live};
use crate::pipeline::Pipeline;
use crate::session::{Flash, Session};
use crate::template::Rendered;
use crate::token::SigningKey;
use axum::Router;
use axum::body::Body;
use axum::extract::{FromRef, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::{Next, from_fn, map_request};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::MethodRouter;
use serde::de::DeserializeOwned;
use std::fmt::Display;

/// A group of routes that share a path prefix, a [`Pipeline`] and a layout.
///
/// `Router::new().merge(scope)` adds its routes to an axum [`Router`]. See the
/// [module's example](self).
#[must_use = "a Scope serves nothing until it is merged into a Router"]
pub struct Scope<S = ()> {
    prefix: String,
    routes: Vec<(String, MethodRouter<S>)>,
    scopes: Vec<Scope<S>>,
    layout: Option<Layout>,
    live_session: Option<String>,
    pipeline: Pipeline<S>,
}

/// Puts a page into a layout. See [`Scope::layout`].
type Layout = fn(Rendered, &Flash) -> Rendered;

impl<S: Clone + Send + Sync + 'static> Scope<S> {
    /// A Scope whose routes all start with `prefix`: `"/admin"`, or `"/"` for none.
    pub fn new(prefix: &str) -> Scope<S> {
        Scope {
            prefix: prefix.trim_end_matches('/').to_owned(),
            routes: Vec::new(),
            scopes: Vec::new(),
            layout: None,
            live_session: None,
            pipeline: Pipeline::new(),
        }
    }

    /// Puts the LiveViews of this Scope, and of the Scopes inside it, in the live
    /// session `name`. A live navigation ([`Socket::navigate`](crate::live::Socket::navigate),
    /// a `navigate` link) goes over the open connection only between LiveViews of one
    /// live session: to a route in another, the browser loads the page instead, and
    /// the route's own Pipeline gives it its session and its checks. A LiveView is
    /// never moved from one to the other with the state it had.
    ///
    /// So routes with different requirements, such as the ones that need a signed-in
    /// user and the ones that do not, go in live sessions of their own. The routes in
    /// no live session are in one together.
    ///
    /// ```
    /// use griffin_web::live::{LiveView, Socket, live};
    /// use griffin_web::router::Scope;
    /// use griffin_web::template::Rendered;
    /// use griffin_web::token::SigningKey;
    /// use griffin_web::{axum::Router, html};
    /// use std::convert::Infallible;
    ///
    /// struct Page;
    /// impl LiveView for Page {
    ///     type Params = ();
    ///     type Event = Infallible;
    ///     type Message = Infallible;
    ///     type Error = std::convert::Infallible;
    ///     async fn mount(_: (), _: &mut Socket) -> Page { Page }
    ///     fn render(&self) -> Rendered { html! { <p>Page</p> } }
    /// }
    ///
    /// let admin = Scope::new("/admin").live_session("admin").route("/", live::<Page, _>());
    /// let public = Scope::new("/").route("/", live::<Page, _>());
    /// let app: Router<SigningKey> = Router::new().merge(admin).merge(public);
    /// ```
    pub fn live_session(mut self, name: &str) -> Scope<S> {
        self.live_session = Some(name.to_owned());
        self
    }

    /// Sends every request of this Scope through the Layers of `pipeline`, top to
    /// bottom, before it reaches a handler. Called again, it adds the Layers of the
    /// next [`Pipeline`] after those of the one before.
    ///
    /// The Layers are outside the layout: what they see of a response is the page
    /// already in it.
    pub fn pipe_through(mut self, pipeline: Pipeline<S>) -> Scope<S> {
        self.pipeline.extend(pipeline);
        self
    }

    /// Wraps every template a handler of this Scope returns in a layout, and the
    /// Dead render of every LiveView in it.
    ///
    /// A layout is a [`Component`](crate::template::Component) with a default slot,
    /// and `layout` puts the page in it: `|page, _flash| html! { <Site>{page}</Site> }`.
    /// A Scope without one sends the page as the handler returned it. A response that
    /// is not a template is never wrapped.
    ///
    /// The second argument is the [`Flash`] to show with the page, taken out of the
    /// session so that it is shown once: `<Site flash={flash}>{page}</Site>`. It is
    /// empty unless the Scope is piped through a
    /// [`SessionLayer`](crate::session::SessionLayer).
    pub fn layout(mut self, layout: fn(Rendered, &Flash) -> Rendered) -> Scope<S> {
        self.layout = Some(layout);
        self
    }

    /// Adds a route, as [`Router::route`] does, at the Scope's prefix followed by
    /// `path`. The path `"/"` is the prefix itself. A LiveView is a route like any
    /// other: `.route("/clock", live::<Clock, _>())`, with [`live`](crate::live::live).
    pub fn route(mut self, path: &str, handlers: MethodRouter<S>) -> Scope<S> {
        self.routes.push((path.to_owned(), handlers));
        self
    }

    /// Puts a Scope inside this one. Its prefix comes after this Scope's, its requests
    /// pass this Scope's Pipelines before its own, and its pages get this Scope's
    /// layout unless it has one itself.
    ///
    /// ```
    /// use griffin_web::axum::routing::get;
    /// use griffin_web::router::Scope;
    ///
    /// // Serves `/admin` and `/admin/users/{id}`.
    /// let admin: Scope = Scope::new("/admin")
    ///     .route("/", get(|| async { "Dashboard" }))
    ///     .scope(Scope::new("/users").route("/{id}", get(|| async { "A user" })));
    /// ```
    pub fn scope(mut self, inner: Scope<S>) -> Scope<S> {
        self.scopes.push(inner);
        self
    }

    /// The router of this Scope, inside a Scope with the prefix and layout given.
    fn into_router(
        self,
        outer_prefix: &str,
        outer_layout: Option<Layout>,
        outer_live_session: Option<String>,
    ) -> Router<S> {
        let prefix = format!("{outer_prefix}{}", self.prefix);
        let layout = self.layout.or(outer_layout);
        let live_session = self.live_session.or(outer_live_session);
        let mut routes = Router::new();
        for (path, handlers) in self.routes {
            let path = match (prefix.as_str(), path.as_str()) {
                ("", path) => path.to_owned(),
                (prefix, "/") => prefix.to_owned(),
                (prefix, path) => format!("{prefix}{path}"),
            };
            routes = routes.route(&path, handlers);
        }
        if let Some(layout) = layout {
            let wrap = move |request: Request, next: Next| async move {
                let session = request.extensions().get::<Session>().cloned();
                wrap(layout, session, next.run(request).await)
            };
            routes = routes.layer(from_fn(wrap));
        }
        // What the LiveViews of the routes find out which live session they are in by.
        if let Some(name) = live_session.clone() {
            routes = routes.layer(map_request(move |mut request: Request| {
                request.extensions_mut().insert(LiveSession(name.clone()));
                std::future::ready(request)
            }));
        }
        // After the layout layer, which is for the routes of this Scope alone: an
        // inner Scope brings its own.
        for scope in self.scopes {
            routes = routes.merge(scope.into_router(&prefix, layout, live_session.clone()));
        }
        // After the layout, so outside it: the layout replaces the body, and a Layer
        // that reads the body must not sit between it and the handler.
        self.pipeline.apply(routes)
    }
}

impl<S: Clone + Send + Sync + 'static> From<Scope<S>> for Router<S> {
    fn from(scope: Scope<S>) -> Router<S> {
        scope.into_router("", None, None)
    }
}

/// The path parameters of a route, typed: axum's [`Path`](axum::extract::Path), except
/// that a request whose parameters do not parse as `T` is answered 404 and not 400. A
/// path that names no `u32` is not the path of a route that takes one.
///
/// ```
/// use griffin_web::router::Path;
///
/// async fn show_user(Path(id): Path<u32>) -> String {
///     format!("user {id}")
/// }
/// ```
#[derive(Debug)]
pub struct Path<T>(pub T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = PathRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Path<T>, PathRejection> {
        match axum::extract::Path::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(parameters)) => Ok(Path(parameters)),
            Err(rejection) => Err(PathRejection(rejection)),
        }
    }
}

/// Why a [`Path`] could not be extracted: axum's own reason. As a response it is 404
/// when the parameters do not parse, and axum's response for a route and a `T` that
/// do not fit each other, which is a bug in the application.
#[derive(Debug)]
pub struct PathRejection(pub axum::extract::rejection::PathRejection);

impl IntoResponse for PathRejection {
    fn into_response(self) -> Response {
        if self.0.status() == StatusCode::BAD_REQUEST {
            return StatusCode::NOT_FOUND.into_response();
        }
        self.0.into_response()
    }
}

/// A handler that takes the path parameters `P` of its route, as a [`Path<P>`](Path)
/// among its arguments. `M` tells where among them, and is inferred.
///
/// It is what [`takes_path`] checks. No type implements it but functions and closures.
#[diagnostic::on_unimplemented(
    message = "this handler does not take the path parameters of its route, which are `{P}`",
    label = "needs an argument of type `griffin_web::router::Path<{P}>`",
    note = "the types of the parameters are those written in the path of the route, in order: \
            one is itself, as in `Path<u32>`, and several are a tuple, as in `Path<(u32, String)>`",
    note = "it is `griffin_web::router::Path` that a handler takes, which answers 404 when a \
            parameter does not parse, where `axum::extract::Path` answers 400"
)]
pub trait TakesPath<P, M> {}

/// Gives `handler` back. It compiles only if the handler takes the path parameters
/// `P`, so a route table puts every handler of a route with parameters through it:
/// `get(takes_path::<(u32, String), _, _>(show_post))`.
///
/// ```compile_fail
/// use griffin_web::router::{Path, takes_path};
///
/// async fn show_user(Path(id): Path<u32>) -> String {
///     format!("user {id}")
/// }
///
/// takes_path::<String, _, _>(show_user);
/// ```
///
/// The check is of the types and their order, not of the names. A handler has to take
/// the parameters to pass it, even if it has no use for them, and as that one `Path`:
/// a struct that names them is not seen to match. A route with no parameters is not
/// checked, as nothing can say that a handler takes no `Path`.
pub fn takes_path<P, M, H: TakesPath<P, M>>(handler: H) -> H {
    handler
}

/// Implements [`TakesPath`] for the functions that take a `Path` at each place among
/// as many arguments as there are names in the second list.
macro_rules! takes_path {
    ([$($before:ident)*] []) => {};
    ([$($before:ident)*] [$here:ident $($after:ident)*]) => {
        impl<F, R, P, $($before,)* $($after,)*>
            TakesPath<P, (($($before,)*), ($($after,)*))> for F
        where
            F: FnOnce($($before,)* Path<P>, $($after,)*) -> R,
        {
        }
        takes_path!([$($before)* $here] [$($after)*]);
    };
}

/// For every number of arguments up to that of the names given, which is the number
/// axum's handlers stop at.
macro_rules! takes_path_at_every_place {
    () => {};
    ($first:ident $($others:ident)*) => {
        takes_path!([] [$first $($others)*]);
        takes_path_at_every_place!($($others)*);
    };
}

takes_path_at_every_place!(T1 T2 T3 T4 T5 T6 T7 T8 T9 T10 T11 T12 T13 T14 T15 T16);

/// [`live`], for a route whose path parameters are `P`: none is `()`, one is itself
/// and several are a tuple. It compiles only if they are the
/// [`Params`](LiveView::Params) the LiveView is mounted with, so a route table serves
/// every LiveView through it: `live_takes_path::<Counter, i32, _>()`.
///
/// rustc reports the other case in its own words, as a type mismatch between the two.
pub fn live_takes_path<V, P, S>() -> MethodRouter<S>
where
    V: LiveView<Params = P>,
    S: Clone + Send + Sync + 'static,
    SigningKey: FromRef<S>,
{
    live::<V, S>()
}

/// One route of a route table, as a listing of the table shows it. A table lists its
/// routes in `Routes::LIST`, in the order written: neither axum's router nor a
/// [`Scope`] can be asked what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// The verb as the table has it: `"GET"`, or `"LIVE"` for a LiveView, which a
    /// GET reaches.
    pub method: &'static str,
    /// The whole path as axum takes it, under the prefix of every Scope around the
    /// route: `"/admin/users/{id}"`.
    pub path: &'static str,
    /// The name of its Path helper, if it has one.
    pub name: Option<&'static str>,
    /// The handler or the LiveView, as the table names it: `"users::show"`.
    pub target: &'static str,
}

/// The listing `cargo griffin routes` prints: one line per route, in the order written,
/// with the method, the path, the name (`-` if it has none) and the target in columns.
///
/// ```
/// use griffin_web::router::{Route, format_routes};
///
/// let list = [Route { method: "GET", path: "/", name: Some("home"), target: "pages::home" }];
/// assert_eq!(
///     format_routes(&list),
///     "METHOD  PATH  NAME  TARGET\nGET     /     home  pages::home\n"
/// );
/// ```
pub fn format_routes(routes: &[Route]) -> String {
    let rows =
        std::iter::once(["METHOD", "PATH", "NAME", "TARGET"]).chain(routes.iter().map(|route| {
            [
                route.method,
                route.path,
                route.name.unwrap_or("-"),
                route.target,
            ]
        }));
    let rows: Vec<_> = rows.collect();
    let width = |column: usize| rows.iter().map(|row| row[column].len()).max().unwrap_or(0);
    let widths = [width(0), width(1), width(2)];
    let mut table = String::new();
    for row in rows {
        for (column, cell) in row[..3].iter().enumerate() {
            table.push_str(&format!("{cell:<0$}  ", widths[column]));
        }
        table.push_str(row[3]);
        table.push('\n');
    }
    table
}

/// A value as one segment of a path, percent-encoded: everything but ASCII letters,
/// digits, `-`, `.`, `_` and `~` is, so a `/`, a `?`, a `#` or a space in the value
/// cannot end the segment. A Path helper puts each of its parameters through this.
///
/// ```
/// use griffin_web::router::encode_segment;
///
/// assert_eq!(encode_segment("a/b?c d"), "a%2Fb%3Fc%20d");
/// assert_eq!(format!("/users/{}", encode_segment(7)), "/users/7");
/// ```
///
/// Two kinds of value no encoding can keep: an empty one is no segment, and a browser
/// takes `.` and `..` as the directory and its parent however they are written. A
/// parameter that may hold any text has to be checked for them.
pub fn encode_segment(value: impl Display) -> String {
    encode(&value.to_string(), false)
}

/// A value as the rest of a path, for a parameter written `{*rest}`: encoded as
/// [`encode_segment`] does, except that each `/` stays.
pub fn encode_path(value: impl Display) -> String {
    encode(&value.to_string(), true)
}

fn encode(value: &str, keep_slashes: bool) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            b'/' if keep_slashes => encoded.push('/'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The template a response was rendered from, kept beside the HTML in the response's
/// extensions for the layout of the Scope to wrap.
#[derive(Clone)]
pub(crate) struct Page(pub(crate) Rendered);

/// A template is a response: its HTML, with status 200.
impl IntoResponse for Rendered {
    fn into_response(self) -> Response {
        // A page in a Scope with a layout is rendered twice, here and inside
        // the layout. Render in the Scope alone if it shows up in a profile.
        let mut response = Html(self.to_html()).into_response();
        response.extensions_mut().insert(Page(self));
        response
    }
}

/// The response with its page inside the layout, if it was rendered from a template.
/// The flash goes with the page: any other response leaves it in the session.
fn wrap(layout: Layout, session: Option<Session>, mut response: Response) -> Response {
    let Some(Page(page)) = response.extensions_mut().remove() else {
        return response;
    };
    let flash = session.map(|session| session.take_flash());
    let page = layout(page, &flash.unwrap_or_default());
    // No `Content-Length` to correct: axum sets it after the layers of a route.
    *response.body_mut() = Body::from(page.to_html());
    response
}
