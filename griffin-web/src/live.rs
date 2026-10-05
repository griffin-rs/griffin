//! LiveView: a page whose state lives on the server.
//!
//! A LiveView is rendered twice. The Dead render is the plain HTTP response that
//! shows the page before any script runs. The Connected render follows when the
//! Phoenix client opens the socket and joins: the LiveView is mounted again, in a task
//! of its own that keeps the state, handles the browser's [`Event`]s and the server
//! side's Messages (sent through a [`Handle`]) one at a time, and sends the browser
//! what changed on the page after each.
//!
//! ```
//! use griffin_web::axum::Router;
//! use griffin_web::html;
//! use griffin_web::live::{Event, LiveView, Socket, live, live_socket};
//! use griffin_web::pipeline::Pipeline;
//! use griffin_web::router::Scope;
//! use griffin_web::session::SessionLayer;
//! use griffin_web::template::Rendered;
//! use griffin_web::token::SigningKey;
//! use std::convert::Infallible;
//!
//! struct Counter {
//!     count: i32,
//! }
//!
//! #[derive(Event)]
//! enum CounterEvent {
//!     Add { by: i32 },
//!     Reset,
//! }
//!
//! impl LiveView for Counter {
//!     type Params = i32;
//!     type Event = CounterEvent;
//!     type Message = Infallible;
//!     type Error = Infallible;
//!
//!     async fn mount(start: i32, _socket: &mut Socket) -> Counter {
//!         Counter { count: start }
//!     }
//!
//!     async fn handle_event(
//!         &mut self,
//!         event: CounterEvent,
//!         _socket: &mut Socket,
//!     ) -> Result<(), Self::Error> {
//!         match event {
//!             CounterEvent::Add { by } => self.count += by,
//!             CounterEvent::Reset => self.count = 0,
//!         }
//!         Ok(())
//!     }
//!
//!     fn render(&self) -> Rendered {
//!         html! {
//!             <p>
//!                 Count: {@count}
//!                 <button phx-click={CounterEvent::Add { by: 1 }}>One more</button>
//!                 <button phx-click={CounterEvent::Reset}>Start over</button>
//!             </p>
//!         }
//!     }
//! }
//!
//! # let secret = "read this from configuration, not from the source";
//! // The socket is only opened with the CSRF token of a session, so the LiveViews
//! // are behind the browser Pipeline, which has the session in it.
//! let site = Scope::new("/")
//!     .pipe_through(Pipeline::browser(SessionLayer::new(secret)?))
//!     .route("/counter/{start}", live::<Counter, _>());
//! let pages = Router::new().merge(site);
//! let app: Router = pages
//!     .clone()
//!     .route("/live/websocket", live_socket(pages))
//!     .with_state(SigningKey::new(secret)?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The page's script opens the socket with that token: see [`SocketChecks`].
//!
//! # In the browser
//!
//! The client is Phoenix's, unmodified (the wire protocol is Phoenix's, unchanged), so its bindings are attributes in
//! a template and work as its documentation says: `phx-click`, `phx-keydown` and
//! `phx-keyup` with `phx-key`, their `phx-window-` forms, `phx-focus`, `phx-blur`,
//! `phx-click-away`, and `phx-debounce` and `phx-throttle` beside any of them. An
//! Event whose fields the browser supplies, as it does `key` for a key binding and
//! `value` for an input, is bound by its name.
//!
//! [`Js`] builds client commands: what a binding does in the browser itself.
//!
//! # Navigation
//!
//! A LiveView follows its URL in [`LiveView::handle_params`], which runs after mount in
//! both renders. [`Socket::patch`] changes the URL to another path of the same route
//! and handles the new parameters with the state kept; [`Socket::navigate`] goes to
//! another LiveView in the same [live session](crate::router::Scope::live_session) over
//! the open connection; [`Socket::redirect`] leaves for any page, with the flash of
//! [`Socket::put_flash`]. A template links to them with `<a patch={path}>` and
//! `<a navigate={path}>`, which take what a Path helper of [`routes!`](crate::routes)
//! gives (see [`Link`]). A target is a [local path](BadTarget), whatever it is made of.
//! [`Socket::set_title`] sets the page title, which the diff carries under the key `t`.
//!
//! A join that follows a live navigation is told apart from a page's by `redirect` in
//! place of `url`. It carries the tokens of the page that was left, which name the live
//! session that page was in, and mounts only a route of that live session: the
//! route's own Pipeline gives it its session and its checks, as for any join, and the
//! CSRF token of the socket is held against that session. A route in another live
//! session, or one that is no LiveView, is refused as `unauthorized`, and the client
//! loads the page.
//!
//! A client hook is JavaScript of the application's, given to the client under a
//! name and attached by it: `phx-hook="Name"` on an element with an `id`. A hook
//! pushes an Event with `this.pushEvent(name, value, reply => ..)`, which the handler
//! answers with [`Socket::reply`], and listens with `this.handleEvent(name, ..)` to
//! what [`Socket::push_event`] pushes.

mod form;
mod js;

pub use form::FormParams;
pub(crate) use form::MAX_BYTES as FORM_MAX_BYTES;
pub use js::Js;

use crate::session::Session;
use crate::template::{AttributeValue, DiffState, Rendered};
use crate::token::{SigningKey, TokenError};
use crate::transport::{self, Message};
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRef, Path, State};
use axum::http::header::{COOKIE, HOST, ORIGIN};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};
use axum::middleware::map_request;
use axum::response::{Html, IntoResponse as _, Redirect, Response};
use axum::routing::{MethodRouter, get};
use axum::{Extension, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tower_service::Service as _;

/// A LiveView: the implementing struct is its state.
///
/// # Failure
///
/// A handler says no to the user by changing the state, so that the page shows it,
/// and returning `Ok`: a rejection the application provides for is not an error here.
/// An error is for what it did not provide for. A handler that returns one, or that
/// panics, ends this LiveView and no other. The failure is logged, the state is
/// dropped, and the client is told that its channel crashed. It joins again, which
/// mounts a fresh LiveView.
pub trait LiveView: Sized + Send + 'static {
    /// The route's path parameters, as [`axum::extract::Path`] takes them: one value,
    /// a tuple, or a struct with a field per parameter. `()` for a route with none.
    type Params: DeserializeOwned + Send + 'static;

    /// What the browser can ask of this LiveView: typically an enum with
    /// [`#[derive(Event)]`](macro@Event). [`Infallible`] for a LiveView that takes none.
    type Event: Event + Send;

    /// What the server side can tell this LiveView through a [`Handle`]: a timer's
    /// tick, the result of work done in another task. Any type, typically an enum.
    /// [`Infallible`] for a LiveView that is told nothing.
    type Message: Send + 'static;

    /// What a handler fails with: the application's own error type, so that the
    /// signature says what can go wrong. [`Infallible`] for a LiveView whose handlers
    /// do not fail. The failure is logged as the `Display` of the error and of each
    /// [source](std::error::Error::source) under it, never as `Debug`. Those messages
    /// are logged as the application wrote them, so they must not hold secrets or
    /// Event payloads.
    type Error: std::error::Error + Send + 'static;

    /// Builds the initial state. It runs once for the Dead render and again when the
    /// browser connects; [`Socket::connected`] tells which. The second time it runs in
    /// the LiveView's own task, as the handlers after it do.
    ///
    /// Write it as `async fn mount(..) -> Self`.
    fn mount(params: Self::Params, socket: &mut Socket) -> impl Future<Output = Self> + Send;

    /// Handles the route's parameters, which are `params`: right after [`mount`](Self::mount)
    /// in both renders, and again, with the new ones, whenever the URL is patched
    /// ([`Socket::patch`], or a `data-phx-link="patch"` link) to another path of this
    /// route. The state is kept: this is where a LiveView follows its URL. What the
    /// query string holds is [`Socket::query`].
    ///
    /// It may [`patch`](Socket::patch), [`navigate`](Socket::navigate) or
    /// [`redirect`](Socket::redirect), as a handler does: in the Dead render the
    /// browser is then redirected. A patch that goes on and on, 20 times
    /// in a row, ends this LiveView, as a failure does.
    ///
    /// Does nothing unless overridden. Write it as
    /// `async fn handle_params(&mut self, ..) -> Result<(), Self::Error>`.
    fn handle_params(
        &mut self,
        params: Self::Params,
        socket: &mut Socket,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let _ = (params, socket);
        async { Ok(()) }
    }

    /// Handles an Event from the browser by changing the state. The page is rendered
    /// again afterwards and the browser is sent what changed. Events of one LiveView
    /// are handled one at a time, in the order they arrived.
    ///
    /// The Event arrives already decoded, by [`Event::decode`]. One that cannot be
    /// decoded does not get here. The browser then sent what no template of this
    /// LiveView binds, which is an unexpected failure: it is logged as an error with
    /// the Event's name and without what came with it, and this LiveView ends, as it
    /// does when the handler [fails](LiveView#failure).
    ///
    /// Does nothing unless overridden. Write it as
    /// `async fn handle_event(&mut self, ..) -> Result<(), Self::Error>`.
    fn handle_event(
        &mut self,
        event: Self::Event,
        socket: &mut Socket,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let _ = (event, socket);
        async { Ok(()) }
    }

    /// Handles a Message sent through a [`Handle`] by changing the state. The page is
    /// rendered again afterwards, and what changed is pushed to the browser without
    /// its asking. Messages and Events of one LiveView go through one queue: they are
    /// handled one at a time, in the order they arrived, never two at once.
    ///
    /// Does nothing unless overridden. Write it as
    /// `async fn handle_message(&mut self, ..) -> Result<(), Self::Error>`.
    fn handle_message(
        &mut self,
        message: Self::Message,
        socket: &mut Socket,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let _ = (message, socket);
        async { Ok(()) }
    }

    /// The page for the current state.
    fn render(&self) -> Rendered;
}

/// An Event: something the browser asks of a LiveView in response to user
/// interaction. A LiveView's Events are one type, typically an enum, so the set is
/// closed, and [`LiveView::handle_event`] is given the value, not a name and a map.
///
/// On the wire an Event is what the unmodified Phoenix client understands (the wire protocol is Phoenix's, unchanged):
/// a name, and fields as text. Binding one to an element in a template writes the
/// name as the attribute's value and each field as a `phx-value-<field>` attribute
/// beside it. The client sends both back when the binding fires, and
/// [`decode`](Event::decode) makes the Event of them again.
///
/// # Deriving
///
/// `#[derive(Event)]` on an enum writes the implementation: each variant is one Event.
///
/// - A variant's name on the wire is its name in snake case: `ClearDone` is
///   `clear_done`.
/// - A variant has no fields or named ones. A field's name on the wire is its own.
/// - A field is of any type with [`FromStr`] and [`Display`](fmt::Display): the
///   numbers, `bool`, `char`, `String`, and a newtype that has both.
/// - A field written as `Option<T>` may be missing: `None` writes no attribute, and
///   no attribute is `None`.
///
/// ```
/// use griffin_web::html;
/// use griffin_web::live::{Event, Payload};
/// use serde_json::json;
///
/// #[derive(Event, Debug, PartialEq)]
/// enum TodoEvent {
///     Rename { id: u32, title: String },
///     ClearDone,
/// }
///
/// let button = html! {
///     <button phx-click={TodoEvent::Rename { id: 7, title: "Milk".into() }}>Rename</button>
/// };
/// assert_eq!(
///     button.to_html(),
///     r#"<button phx-click="rename" phx-value-id="7" phx-value-title="Milk">Rename</button>"#
/// );
///
/// // What the client sends for a click on it: the attributes, as text.
/// let value = json!({"id": "7", "title": "Milk"});
/// assert_eq!(
///     TodoEvent::decode("rename", &Payload::from(&value)),
///     Ok(TodoEvent::Rename { id: 7, title: "Milk".into() })
/// );
/// ```
///
/// A variant that does not exist is a compile error at the binding, like any other
/// misspelt name in an expression.
///
/// # Binding by name
///
/// An Event's name can also be written as plain text, as in Phoenix:
/// `<button phx-click="clear_done">`, with its fields as `phx-value-*` attributes
/// written by hand. It is the same on the wire, so [`decode`](Event::decode) makes
/// the same Event of it and the same handler gets it: `TodoEvent::ClearDone`. So it is
/// for an Event a client hook pushes under that name. Only a bound value is checked
/// by the compiler; a misspelt name is found when the browser sends it, and ends the
/// LiveView (see [`LiveView::handle_event`]).
///
/// # Forms
///
/// `phx-change` and `phx-submit` send the whole form as one URL-encoded string, so a
/// variant for them takes it in a field marked `#[form]`, of the type [`FormParams`].
/// Nothing is written for that field when the Event is bound, so a binding makes one
/// with the type's default: `phx-submit={SignupEvent::Submit { params: FormParams::default() }}`.
/// A Changeset reads the params with `Changeset::cast_form` of `griffin-domain`.
///
/// ```
/// use griffin_web::live::{Event, FormParams, Payload};
/// use serde_json::json;
///
/// #[derive(Event)]
/// enum SignupEvent {
///     Change {
///         #[form]
///         params: FormParams,
///     },
/// }
///
/// // What the client sends for the form of `user[email]` and `user[tags][]`.
/// let value = json!("user%5Bemail%5D=ada%40example.com&user%5Btags%5D%5B%5D=math");
/// let SignupEvent::Change { params } =
///     SignupEvent::decode("change", &Payload::from(&value)).unwrap();
/// assert_eq!(
///     params.pairs().collect::<Vec<_>>(),
///     [("user[email]", "ada@example.com"), ("user[tags][]", "math")]
/// );
/// ```
///
/// # By hand
///
/// The derive writes nothing a developer cannot (every macro lowers to a public API). This is what it writes
/// for the enum above, and the place for names the derive would not make and for
/// values that are not text ([`Payload::value`]):
///
/// ```
/// use griffin_web::live::{Event, EventError, Payload};
/// use griffin_web::template::Slot;
/// use serde_json::json;
///
/// #[derive(Debug, PartialEq)]
/// enum TodoEvent {
///     Rename { id: u32, title: String },
///     ClearDone,
/// }
///
/// impl Event for TodoEvent {
///     fn decode(name: &str, payload: &Payload<'_>) -> Result<TodoEvent, EventError> {
///         match name {
///             "rename" => Ok(TodoEvent::Rename {
///                 id: payload.field("id")?,
///                 title: payload.field("title")?,
///             }),
///             "clear_done" => Ok(TodoEvent::ClearDone),
///             _ => Err(EventError::Unknown),
///         }
///     }
///
///     fn encode(&self) -> (&'static str, Vec<(&'static str, String)>) {
///         match self {
///             TodoEvent::Rename { id, title } => (
///                 "rename",
///                 vec![("id", id.to_string()), ("title", title.to_string())],
///             ),
///             TodoEvent::ClearDone => ("clear_done", vec![]),
///         }
///     }
/// }
///
/// // What `phx-click={TodoEvent::Rename { id: 7, title: "Milk".into() }}` writes.
/// let event = TodoEvent::Rename { id: 7, title: "Milk".into() };
/// assert_eq!(
///     Slot::attribute("phx-click", &event),
///     Slot::raw_html(r#" phx-click="rename" phx-value-id="7" phx-value-title="Milk""#)
/// );
///
/// // What the client sends for a click on that element: the attributes, as text.
/// let value = json!({"id": "7", "title": "Milk"});
/// assert_eq!(TodoEvent::decode("rename", &Payload::from(&value)), Ok(event));
/// ```
///
/// # Limits
///
/// - The browser lowercases attribute names, so a field's name has to be lowercase,
///   as Rust's naming has it anyway.
/// - An element's `phx-value-*` attributes are sent with every Event bound to it. Two
///   Events bound to one element, a click and a key say, must not have a field of
///   the same name with different values. A field an Event does not have is ignored.
///   An Event pushed by a command ([`Js::push`]) carries its own fields instead.
/// - Nothing checks that a bound Event is of the type of the LiveView whose template
///   it is in. One of another type cannot be decoded when it comes back.
pub trait Event: Sized {
    /// The Event the browser sent: `name` is the bound name, and `payload` what came
    /// with it. Both are the browser's, so neither is to be trusted, and this must
    /// not panic on any input.
    fn decode(name: &str, payload: &Payload<'_>) -> Result<Self, EventError>;

    /// The Event as a binding writes it into a template: its name, and each field
    /// with its value as text. A field left out is not written.
    fn encode(&self) -> (&'static str, Vec<(&'static str, String)>);
}

pub use griffin_macros::Event;

/// No Event at all: the `Event` type of a LiveView that takes none.
impl Event for Infallible {
    fn decode(_name: &str, _payload: &Payload<'_>) -> Result<Infallible, EventError> {
        Err(EventError::Unknown)
    }

    fn encode(&self) -> (&'static str, Vec<(&'static str, String)>) {
        match *self {}
    }
}

/// What binds an Event to an element: `phx-click={event}` in a template, or
/// `Slot::attribute("phx-click", &event)` by hand. A Component's attributes are taken
/// by value, so a tag gives it a reference: `<Button phx-click={&event} />`.
impl<E: Event> From<&E> for AttributeValue {
    fn from(event: &E) -> AttributeValue {
        let (name, values) = event.encode();
        AttributeValue::Event { name, values }
    }
}

/// What the browser sent with an Event. For an element with `phx-value-*` attributes
/// it is a JSON object with a text field for each, and the element's own `value` if
/// it has one (a button's is empty).
#[derive(Debug, Clone, Copy)]
pub struct Payload<'a>(&'a Value);

impl<'a> Payload<'a> {
    /// The field `name`, parsed by its type's [`FromStr`]. A JSON number or boolean
    /// is parsed from its text, so `7` and `"7"` are the same.
    pub fn field<T: FromStr>(&self, name: &'static str) -> Result<T, EventError> {
        self.optional_field(name)?.ok_or(EventError::Field(name))
    }

    /// The field `name`, or `None` if the browser sent none: the element did not
    /// have the attribute. One that is there and does not parse is still an error.
    pub fn optional_field<T: FromStr>(&self, name: &'static str) -> Result<Option<T>, EventError> {
        let text = match self.0.get(name) {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(text)) => Cow::Borrowed(text.as_str()),
            Some(scalar @ (Value::Bool(_) | Value::Number(_))) => Cow::Owned(scalar.to_string()),
            Some(Value::Array(_) | Value::Object(_)) => return Err(EventError::Field(name)),
        };
        let parsed = text.parse().map_err(|_| EventError::Field(name))?;
        Ok(Some(parsed))
    }

    /// The params of a form Event: `phx-change` and `phx-submit` send the form as one
    /// URL-encoded string, which is not an object with fields. An error if this
    /// payload is not that, or is larger than a form is (see [`FormParams`]).
    pub fn form(&self, name: &'static str) -> Result<FormParams, EventError> {
        let body = self.0.as_str();
        body.and_then(FormParams::parse)
            .ok_or(EventError::Form(name))
    }

    /// Everything the browser sent, as JSON, for an [`Event`] written by hand that
    /// takes more than text fields, such as what a client hook pushes.
    pub fn value(&self) -> &'a Value {
        self.0
    }
}

impl<'a> From<&'a Value> for Payload<'a> {
    fn from(value: &'a Value) -> Payload<'a> {
        Payload(value)
    }
}

/// Why what the browser sent is not an [`Event`] of a LiveView. It never holds a
/// value the browser sent, so it is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    /// The LiveView has no Event of this name.
    Unknown,
    /// The field of this name is missing, or its value does not parse.
    Field(&'static str),
    /// The form field of this name is not a string, or is over 1 MiB.
    Form(&'static str),
}

impl fmt::Display for EventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventError::Unknown => formatter.write_str("no Event has this name"),
            EventError::Form(name) => write!(
                formatter,
                "the form field `{name}` is not a string or is over 1 MiB"
            ),
            EventError::Field(name) => {
                write!(formatter, "the field `{name}` is missing or does not parse")
            }
        }
    }
}

impl std::error::Error for EventError {}

/// What a LiveView callback is given besides its own state.
#[derive(Debug)]
pub struct Socket {
    connected: bool,
    session: Session,
    /// What the callbacks pushed since the last diff, in order: `[name, payload]` each.
    events: Vec<Value>,
    /// The reply to the Event being handled.
    reply: Option<Value>,
    /// The LiveView whose callbacks are given this, and the queue its task takes from.
    live_view: TypeId,
    queue: mpsc::WeakUnboundedSender<Incoming>,
    /// The tasks the LiveView started. Dropping the set cancels them, and the set is
    /// dropped with the LiveView's task, however that ends.
    tasks: JoinSet<()>,
    /// The URL of the page, as a path and a query: what [`Socket::query`] reads.
    url: Uri,
    /// The page title, and whether the browser has yet to be told it.
    title: Option<String>,
    title_changed: bool,
    /// The flash that goes with a redirect.
    flash: BTreeMap<String, String>,
    /// Where the callback that just ran asked to go, if it did.
    navigation: Option<Navigation>,
}

/// Where a callback asked the browser to go. The last call wins.
#[derive(Debug)]
enum Navigation {
    /// Another URL of the same route, over the same LiveView.
    Patch(String),
    /// Another LiveView of the same live session, over the same connection.
    Navigate(String),
    /// A page loaded from the server, whatever it is.
    Redirect(String),
}

impl Socket {
    fn new<V: LiveView>(
        connected: bool,
        session: Session,
        url: Uri,
        queue: mpsc::WeakUnboundedSender<Incoming>,
    ) -> Socket {
        Socket {
            connected,
            session,
            events: Vec::new(),
            reply: None,
            live_view: TypeId::of::<V>(),
            queue,
            tasks: JoinSet::new(),
            url,
            title: None,
            title_changed: false,
            flash: BTreeMap::new(),
            navigation: None,
        }
    }

    /// The query string of the page's URL, typed: a struct with a field per parameter,
    /// or a map, as [`axum::extract::Query`] takes it. It is the URL of the request in
    /// the Dead render, the one the browser joined with in the Connected render, and
    /// the one patched to after a patch. The browser sends the query, so a type that
    /// does not parse is an error to answer, not a failure of the server.
    pub fn query<T: DeserializeOwned>(&self) -> Result<T, QueryError> {
        axum::extract::Query::<T>::try_from_uri(&self.url)
            .map(|axum::extract::Query(query)| query)
            .map_err(|_| QueryError)
    }

    /// Sets the title of the page, the browser tab's: the Dead render writes it in a
    /// `<title>` and the Connected render sends it in the diff under the key `t`, as
    /// Phoenix does, when it is not what the browser was last told. The client sets
    /// the document's title.
    ///
    /// A layout that wraps the Dead render writes its own `<head>` and does not see it:
    /// it is only written in the `<title>` of the document Griffin wraps a page in
    /// when its Scope has no layout.
    pub fn set_title(&mut self, title: impl Into<String>) {
        let title = title.into();
        if self.title.as_deref() != Some(&title) {
            self.title = Some(title);
            self.title_changed = true;
        }
    }

    /// Changes the URL to `to`, a path of this LiveView's own route, without leaving
    /// it: the browser's address bar and history are updated, [`handle_params`](LiveView::handle_params)
    /// runs with the parameters of the new path, and the state is kept. The page is
    /// rendered again afterwards.
    ///
    /// `to` must be a [local path](BadTarget). A path that is not this LiveView's
    /// route, in this LiveView's live session, ends the LiveView as a failure does.
    pub fn patch(&mut self, to: &str) -> Result<(), BadTarget> {
        self.navigation = Some(Navigation::Patch(local_path(to)?));
        Ok(())
    }

    /// Goes to another LiveView over the connection that is open, without loading the
    /// page again: the browser leaves this LiveView and joins the one `to` is the route
    /// of. `to` must be a [local path](BadTarget), and the route of a LiveView in the
    /// same live session as this one (see [`Scope::live_session`](crate::router::Scope::live_session)):
    /// otherwise the browser loads `to` as a page, as for any other link.
    ///
    /// Flash left with [`put_flash`](Socket::put_flash) does not go with it.
    pub fn navigate(&mut self, to: &str) -> Result<(), BadTarget> {
        self.navigation = Some(Navigation::Navigate(local_path(to)?));
        Ok(())
    }

    /// Leaves the live world: the browser loads `to` as a page, a controller's say,
    /// with the flash left by [`put_flash`](Socket::put_flash), which the page shows
    /// once. The page needs a [`SessionLayer`](crate::session::SessionLayer) for the
    /// flash to go anywhere: without one a LiveView that redirects with flash ends
    /// as a failure.
    ///
    /// `to` must be a [local path](BadTarget): a target that is on another site is
    /// not for this to send the user to.
    pub fn redirect(&mut self, to: &str) -> Result<(), BadTarget> {
        self.navigation = Some(Navigation::Redirect(local_path(to)?));
        Ok(())
    }

    /// Leaves `message` as the flash of the kind `kind` (`"info"`, `"error"`) for
    /// the page a [`redirect`](Socket::redirect) leads to, in place of one of that kind
    /// already left. See [`Flash`](crate::session::Flash).
    ///
    /// On the way it is a token the browser holds and brings back: signed, and
    /// good for a minute, so that the browser cannot make a flash message up.
    pub fn put_flash(&mut self, kind: &str, message: impl Into<String>) {
        self.flash.insert(kind.to_owned(), message.into());
    }

    /// The flash as the token the redirect carries, if there is any. The error is a
    /// session no layer gave, which has nothing to sign with.
    fn flash_token(&self) -> Result<Option<String>, ()> {
        if self.flash.is_empty() {
            return Ok(None);
        }
        self.session.sign_flash(&self.flash).map(Some).ok_or(())
    }

    /// False in the Dead render, true once the browser has connected. Work that only
    /// a connected page needs, such as a subscription, can wait for true.
    pub fn connected(&self) -> bool {
        self.connected
    }

    /// The value the [session](crate::session) holds under `key`, or `None` if there
    /// is none or it is not a `T`. In the Dead render it is the session of the
    /// request, and in the Connected render the one the browser had when it opened
    /// the socket. A LiveView reads the session and cannot change it.
    ///
    /// There is nothing to read unless the route's Scope is piped through a
    /// [`SessionLayer`](crate::session::SessionLayer).
    pub fn session<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.session.get(key)
    }

    /// Pushes the event `name` to the browser, with `payload` as its data: a JSON
    /// object, as `json!({"id": id})` makes one. It goes with the next diff. The
    /// client hands it to every client hook that listens with
    /// `this.handleEvent(name, payload => ..)`, and dispatches it on `window` as the
    /// DOM event `phx:<name>` with the payload as its `detail`, once it has patched
    /// the page. Events reach the browser in the order they were pushed.
    ///
    /// The payload is application data sent to the browser: put nothing in it that
    /// the user may not see. In the Dead render there is no client, and nothing is
    /// pushed.
    pub fn push_event(&mut self, name: &str, payload: Value) {
        self.events.push(json!([name, payload]));
    }

    /// Replies to the Event being handled with `payload`, a JSON object: the client
    /// hook that pushed the Event with `this.pushEvent(name, value, reply => ..)` is
    /// given it. A second reply replaces the first. Outside
    /// [`LiveView::handle_event`] there is nothing to reply to, and it is dropped.
    ///
    /// Like a pushed event's, the payload is for the user's eyes.
    pub fn reply(&mut self, payload: Value) {
        self.reply = Some(payload);
    }

    /// Moves what the callbacks left for the client into `diff`, under the keys the
    /// client reads them from (`maybe_put_reply` and `maybe_put_events` in Phoenix's
    /// `diff.ex`). A reply only goes with the diff that answers an Event.
    fn flush(&mut self, diff: &mut Value, answers_event: bool) {
        if let (Some(reply), true) = (self.reply.take(), answers_event) {
            diff["r"] = reply;
        }
        // `maybe_put_title` in Phoenix's `diff.ex`: only when it changed.
        if let (Some(title), true) = (&self.title, std::mem::take(&mut self.title_changed)) {
            diff["t"] = json!(title);
        }
        if !self.events.is_empty() {
            diff["e"] = Value::Array(std::mem::take(&mut self.events));
        }
    }

    /// A handle that sends Messages to this LiveView: `socket.handle::<Self>()` in a
    /// callback of the LiveView. Give it to whatever is to tell the LiveView something
    /// later, from another task.
    ///
    /// # Panics
    ///
    /// If `V` is not the LiveView whose callback was given this Socket.
    pub fn handle<V: LiveView>(&self) -> Handle<V> {
        assert!(
            self.live_view == TypeId::of::<V>(),
            "this Socket is not that of a `{}`",
            std::any::type_name::<V>()
        );
        Handle {
            queue: self.queue.clone(),
            live_view: PhantomData,
        }
    }

    /// Starts a task that belongs to this LiveView: a timer, slow work whose result
    /// comes back as a Message through a [`Handle`]. It runs beside the LiveView, which
    /// goes on handling Events and Messages meanwhile, and it does not outlive it:
    /// when the LiveView ends, however it ends, its tasks are cancelled where they
    /// next wait. A task that panics ends the LiveView, as a handler that panics does.
    ///
    /// In the Dead render the task is cancelled when the page has been rendered, so
    /// start one only when [`connected`](Socket::connected).
    pub fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(task);
    }
}

/// Why a target of [`Socket::patch`], [`navigate`](Socket::navigate) or
/// [`redirect`](Socket::redirect) is refused. It never holds the target, which may be
/// made of what a user typed.
///
/// A target must be a **local path**: it starts with exactly one `/`, and holds no
/// backslash and no control character, and it parses as a URI. That refuses a space and
/// `"`, `<`, `>` and the backtick, so what a user typed goes into a query
/// percent-encoded (`q=red%20shoes`); non-ASCII text such as `ไทย` is accepted as it
/// is. Anything else may take the user to another
/// site: `//evil.example` and `/\evil.example` are read as that host by a browser, as is
/// `/<tab>/evil.example`, as a browser drops the tab, and `https://evil.example` and
/// `javascript:..` are not paths at all. A target with user input in it is safe to
/// pass if it was built with a Path helper, which encodes each parameter, or checked
/// by this rule, which every call here does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadTarget;

impl fmt::Display for BadTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a navigation target must be a path on this server, as `/users/7`")
    }
}

impl std::error::Error for BadTarget {}

/// The target if it is a local path: see [`BadTarget`].
fn local_path(to: &str) -> Result<String, BadTarget> {
    let mut chars = to.chars();
    let local = chars.next() == Some('/')
        && !matches!(chars.next(), Some('/' | '\\'))
        && !to.chars().any(|char| char == '\\' || char.is_control())
        // What cannot be a URI cannot be looked up: a space, say, is one to encode
        // (`%20`), not one to carry on.
        && to.parse::<Uri>().is_ok();
    local.then(|| to.to_owned()).ok_or(BadTarget)
}

/// A LiveView redirected with flash where no `SessionLayer` is to keep it.
#[derive(Debug)]
struct FlashWithoutSession;

impl fmt::Display for FlashWithoutSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(
            "a LiveView redirected with flash, which needs the Scope of its route to be piped \
             through a `SessionLayer`",
        )
    }
}

impl std::error::Error for FlashWithoutSession {}

/// Why the query string is not a `T`: see [`Socket::query`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryError;

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the query string does not parse as the type asked for")
    }
}

impl std::error::Error for QueryError {}

/// A live link, as attributes for an `<a>` tag: `<a navigate={user_path(7)}>` and
/// `<a patch={page_path(2)}>` in a template write the same, with the Path helper's
/// output as the target.
///
/// A click on `navigate` moves to another LiveView of the same live session over the
/// connection that is open, and on `patch` to another URL of this one: the browser
/// does it by itself, as `data-phx-link` says, and falls back to loading the page
/// where it cannot. The link is an ordinary one without script. Both are `push`: they
/// add to the browser's history, which back and forward then walk.
///
/// ```
/// use griffin_web::html;
///
/// let page = 2;
/// let path = format!("/pages/{page}");
/// let link = html! { <a patch={path}>Next</a> };
/// assert_eq!(
///     link.to_html(),
///     r#"<a href="/pages/2" data-phx-link="patch" data-phx-link-state="push">Next</a>"#
/// );
/// ```
///
/// A target that is not a local path (see [`BadTarget`]) renders no attributes, so no
/// link at all, and logs a warning: a `Link` cannot fail, as it is built in a template.
///
/// [`Link`] itself is for a spread, `<a {Link::patch(&path)}>`, where the kind is only
/// known at run time.
// Only `push`; `data-phx-link-state="replace"` when a link must not add to the
// history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    to: String,
    kind: &'static str,
}

impl Link {
    /// To another LiveView of the same live session.
    pub fn navigate(to: impl fmt::Display) -> Link {
        Link {
            to: to.to_string(),
            kind: "redirect",
        }
    }

    /// To another URL of the same route.
    pub fn patch(to: impl fmt::Display) -> Link {
        Link {
            to: to.to_string(),
            kind: "patch",
        }
    }
}

impl IntoIterator for Link {
    type Item = (&'static str, AttributeValue);
    type IntoIter = std::vec::IntoIter<(&'static str, AttributeValue)>;

    fn into_iter(self) -> Self::IntoIter {
        let value = |text: &str| AttributeValue::Value(text.to_owned());
        // A target that is not a local path writes no attribute at all: no `href` to
        // follow, so the open-redirect rule holds for links as for `Socket::navigate`.
        match local_path(&self.to) {
            Ok(to) => vec![
                ("href", value(&to)),
                ("data-phx-link", value(self.kind)),
                ("data-phx-link-state", value("push")),
            ],
            Err(BadTarget) => {
                // The target is the template's data, so a field of its own, quoted by a
                // subscriber, and cut short.
                tracing::warn!(
                    target = &self.to[..self.to.floor_char_boundary(100)],
                    "a live link whose target is not a local path is left out"
                );
                Vec::new()
            }
        }
        .into_iter()
    }
}

/// Sends [`Message`](LiveView::Message)s to one connected LiveView from anywhere: a
/// timer, another task. [`Socket::handle`] makes one. It can be cloned and sent
/// between tasks, and holding one does not keep the LiveView alive.
///
/// ```
/// use griffin_web::live::{Handle, LiveView};
///
/// /// Tells a LiveView the result of slow work, done in a task of its own.
/// fn report<V: LiveView<Message = u64>>(live_view: Handle<V>) {
///     tokio::spawn(async move {
///         let result = 42; // The slow work.
///         if live_view.send(result).is_err() {
///             // The user has left the page in the meantime.
///         }
///     });
/// }
/// ```
pub struct Handle<V: LiveView> {
    queue: mpsc::WeakUnboundedSender<Incoming>,
    live_view: PhantomData<fn(V)>,
}

impl<V: LiveView> Handle<V> {
    /// Queues a Message for the LiveView, behind the Events and Messages that arrived
    /// before it. It does not wait for the Message to be handled.
    ///
    /// The error says that the LiveView has ended, so nothing will handle the Message:
    /// the user left the page, the connection dropped, or a handler failed. The same
    /// goes for the handle of a Dead render, which has no task to send to.
    pub fn send(&self, message: V::Message) -> Result<(), Ended> {
        let queue = self.queue.upgrade().ok_or(Ended)?;
        let sent = queue.send(Incoming::Message(Box::new(message)));
        sent.map_err(|_| Ended)
    }
}

impl<V: LiveView> Clone for Handle<V> {
    fn clone(&self) -> Handle<V> {
        Handle {
            queue: self.queue.clone(),
            live_view: PhantomData,
        }
    }
}

impl<V: LiveView> fmt::Debug for Handle<V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Handle<{}>", std::any::type_name::<V>())
    }
}

/// Why a Message could not be sent: the LiveView has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended;

impl fmt::Display for Ended {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the LiveView has ended")
    }
}

impl std::error::Error for Ended {}

/// What a LiveView's task takes from its queue, one at a time.
pub(crate) enum Incoming {
    /// A frame from the browser.
    Frame(Message),
    /// A Message from the server side: the LiveView's `Message` type.
    Message(Box<dyn Any + Send>),
}

/// Serves the LiveView `V` on a route: `Router::new().route("/path/{id}", live::<V, _>())`.
///
/// A GET answers with the Dead render. The browser then connects through
/// [`live_socket`]. The router's state must hold the [`SigningKey`], so a router
/// without one does not compile.
///
/// # Lookups
///
/// The socket finds the LiveView of a URL, for a join or a patch, with an internal
/// `TRACE` request through the same router and Layers, so the route is registered for
/// `GET` and `TRACE`. A controller on a path does not answer a `TRACE` and so never runs
/// for a lookup (`405`). Limits: a route registered with `any(..)`, a `.fallback(..)`,
/// or a nested service on that path does answer it, and runs. A Layer inside the pages
/// that refuses or blocks `TRACE` makes every join fail: the browser then loads the
/// page again, over and over (the server logs a warning naming the path). Access logs
/// and tracing Layers show these requests as `TRACE` lines. Why not a plain
/// `GET`: it runs whatever controller is on the path, with the visitor's cookies, its
/// side effects and any one-time token it spends. Why not another method of axum's
/// `MethodRouter`: it cannot register a custom one. Why not a separate live-only
/// router: it would repeat the Pipelines and could drift from them.
pub fn live<V, S>() -> MethodRouter<S>
where
    V: LiveView,
    S: Clone + Send + Sync + 'static,
    SigningKey: FromRef<S>,
{
    // TRACE is for the socket's own lookups (a join, a patch): nothing but a LiveView
    // answers it, so a controller on a path is never run for one.
    get(live_route::<V>).trace(live_route::<V>)
}

/// Serves the socket that connected LiveViews talk over, at the path the Phoenix
/// client opens: the one it was given plus `/websocket`, so
/// `.route("/live/websocket", live_socket(pages))` for `new LiveSocket("/live", ..)`.
///
/// `pages` is the router holding the [`live`] routes, without state. The client joins
/// with the tokens of a Dead render and the URL of the page, and the socket sends that
/// URL through `pages` as a GET would go, so it finds the same LiveView with the same
/// parameters behind the same layers. Of the headers the browser opened the socket
/// with, that request carries the cookies and nothing else. A LiveView that is not in
/// `pages` cannot be joined. See the [module's example](self).
///
/// That lookup, and the one a patch makes, are internal `TRACE` requests (see
/// [`live`]): an `any()` route, a `.fallback(..)` or a nested service on a LiveView's
/// path still runs for them; a Layer in `pages` that refuses or blocks `TRACE` breaks
/// every join and makes the browser reload in a loop; and access logs and tracing
/// Layers will show `TRACE` lines.
///
/// A join is refused unless its tokens were signed by the router's [`SigningKey`]
/// less than two weeks ago, for the page and the LiveView it asks for. The client
/// then loads the page again, which gives it new tokens.
///
/// The socket itself is only opened for the application's own pages, by the checks
/// that [`SocketChecks`] documents: the `Origin` of the handshake, and the CSRF token
/// of the session. `live_socket` has them all on; [`live_socket_with`] is the way to
/// set them.
pub fn live_socket<S>(pages: Router<S>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
    SigningKey: FromRef<S>,
{
    live_socket_with(pages, SocketChecks::new())
}

/// [`live_socket`], with `checks` in place of the default ones.
///
/// ```
/// use griffin_web::axum::Router;
/// use griffin_web::live::{SocketChecks, live_socket_with};
/// use griffin_web::token::SigningKey;
///
/// let pages: Router<SigningKey> = Router::new();
/// // Behind a proxy that does not pass the browser's `Host` on.
/// let checks = SocketChecks::new().allowed_origins(["https://shop.example"]);
/// let app: Router<SigningKey> = pages
///     .clone()
///     .route("/live/websocket", live_socket_with(pages, checks));
/// ```
pub fn live_socket_with<S>(pages: Router<S>, checks: SocketChecks) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
    SigningKey: FromRef<S>,
{
    get(
        move |State(state): State<S>, uri: Uri, headers: HeaderMap, upgrade: WebSocketUpgrade| async move {
            if !checks.allow_origin(&headers, &uri) {
                // The origin is the browser's word, so it is a field of its own, which
                // a subscriber quotes, and cut short.
                let origin = headers.get(ORIGIN).map(|origin| origin.as_bytes());
                let origin = String::from_utf8_lossy(origin.unwrap_or_default());
                tracing::warn!(
                    origin = &origin[..origin.floor_char_boundary(100)],
                    "a socket was refused: its Origin is not this host's, nor one of \
                     `SocketChecks::allowed_origins`"
                );
                return StatusCode::FORBIDDEN.into_response();
            }
            let csrf = if checks.csrf {
                let mut query = form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes());
                match query.find(|(name, _)| name == "_csrf_token") {
                    Some((_, token)) => SocketCsrf::Token(token.into_owned()),
                    // Loading the page again would not help a client that sends none.
                    None => {
                        tracing::warn!(
                            "a socket was refused: it sent no `_csrf_token`. The page's \
                             script passes the container's `data-csrf-token` as that \
                             param, which is there when the LiveView's Scope is piped \
                             through a `SessionLayer`"
                        );
                        return StatusCode::FORBIDDEN.into_response();
                    }
                }
            } else {
                SocketCsrf::Unchecked
            };
            let key = SigningKey::from_ref(&state);
            // A join's request carries the cookies the browser opened the socket with,
            // so that the Layers of the pages find the session they found for the page,
            // and the CSRF token it sent, for the route to hold against that session.
            // A layer on every route for each connection. Give `join` the
            // headers instead if it shows up in a profile.
            let cookies: Vec<_> = headers.get_all(COOKIE).into_iter().cloned().collect();
            let carry = map_request(move |mut request: Request<Body>| {
                for cookie in &cookies {
                    request.headers_mut().append(COOKIE, cookie.clone());
                }
                request.extensions_mut().insert(csrf.clone());
                std::future::ready(request)
            });
            let pages = pages.with_state(state).layer(carry);
            upgrade.on_upgrade(move |socket| transport::connection(socket, pages, key))
        },
    )
}

/// What a socket checks before it serves a browser: that the browser is on one of the
/// application's own pages, and not on a page of another site that opened the socket
/// with the visitor's cookies (cross-site WebSocket hijacking).
///
/// # The origin
///
/// A browser says which site's page opens a socket in the handshake's `Origin`
/// header, and no page can change that. A handshake whose `Origin` is not allowed is
/// answered `403 Forbidden`.
///
/// - By default the one origin allowed is the host the handshake was sent to: the
///   host and port of `Origin` must be those of the `Host` header. The scheme is not
///   compared, as a server behind a proxy that terminates TLS does not know its own.
/// - With [`allowed_origins`](Self::allowed_origins), the origins allowed are the
///   ones listed and no other. That is the setting for a server behind a proxy that
///   does not pass the browser's `Host` on, and for a page served from another origin.
/// - A handshake **without** an `Origin` is let through. Every browser sends the
///   header with every WebSocket handshake, so one without it is not a browser, and a
///   program that is not a browser has no visitor's cookies to abuse and could send
///   any `Origin` it liked. It still has to pass the next check.
///
/// # The CSRF token
///
/// The socket must be opened with the param `_csrf_token`, and it must be a
/// [CSRF token](crate::session::Session::csrf_token) of the session the browser's
/// cookie holds. The Dead render writes one on the LiveView's container as
/// `data-csrf-token`, for the page's script to pass on:
///
/// ```js
/// const container = document.querySelector("[data-phx-main]");
/// const csrfToken = container.getAttribute("data-csrf-token");
/// const liveSocket = new LiveSocket("/live", Socket, { params: { _csrf_token: csrfToken } });
/// ```
///
/// So the LiveViews must be in a Scope piped through a
/// [`SessionLayer`](crate::session::SessionLayer), as
/// [`Pipeline::browser`](crate::pipeline::Pipeline::browser) has it.
///
/// - A handshake without the param is answered `403 Forbidden`.
/// - A socket opened with a token that is not its session's is served, but every join
///   on it is refused with the reason `unauthorized`, before mount runs. The Phoenix
///   client answers that by loading the page again, which gives it the token of the
///   session it has now. This is how a page recovers whose session was replaced under
///   it, by a sign-out in another tab say. A `403` would leave it trying forever, as
///   the client cannot tell why a handshake failed.
/// - [`without_csrf_check`](Self::without_csrf_check) turns this check off, for an
///   application whose LiveViews use no session.
#[derive(Debug, Clone)]
#[must_use = "the checks do nothing until they are given to `live_socket_with`"]
pub struct SocketChecks {
    /// `None` for the host the handshake was sent to.
    origins: Option<Vec<String>>,
    csrf: bool,
}

impl SocketChecks {
    /// Every check, as [`live_socket`] has them: the origin must be the host's own,
    /// and the CSRF token is required.
    pub fn new() -> SocketChecks {
        SocketChecks {
            origins: None,
            csrf: true,
        }
    }

    /// Allows the pages of these origins to open the socket, and of no other: each
    /// is a scheme, a host and a port if it is not the scheme's own, as a browser
    /// writes it in `Origin`: `"https://shop.example"`. The host the handshake was
    /// sent to is no longer allowed by itself: list it.
    pub fn allowed_origins<O: Into<String>>(
        mut self,
        origins: impl IntoIterator<Item = O>,
    ) -> SocketChecks {
        self.origins = Some(origins.into_iter().map(Into::into).collect());
        self
    }

    /// Turns the CSRF check off: the socket then serves whoever passes the origin
    /// check, with whatever session their cookie holds. For an application whose
    /// LiveViews use no session, which has no token to send.
    pub fn without_csrf_check(mut self) -> SocketChecks {
        self.csrf = false;
        self
    }

    /// Whether the handshake with these headers may open the socket.
    fn allow_origin(&self, headers: &HeaderMap, uri: &Uri) -> bool {
        let mut origins = headers.get_all(ORIGIN).into_iter();
        let Some(origin) = origins.next() else {
            // Not a browser: see the type's documentation.
            return true;
        };
        let (Ok(origin), None) = (origin.to_str(), origins.next()) else {
            return false;
        };
        if let Some(allowed) = &self.origins {
            return allowed.iter().any(|one| one.eq_ignore_ascii_case(origin));
        }
        // HTTP/2 has the host in the URI.
        let host = headers.get(HOST).and_then(|host| host.to_str().ok());
        let host = host.or_else(|| uri.authority().map(|authority| authority.as_str()));
        // `null`, the origin of a sandboxed page, is no scheme and host, and neither
        // is anything else that is not a URL.
        let Ok(origin) = origin.parse::<Uri>() else {
            return false;
        };
        match (origin.scheme_str(), origin.authority(), host) {
            (Some("http" | "https"), Some(authority), Some(host)) => {
                authority.as_str().eq_ignore_ascii_case(host)
            }
            _ => false,
        }
    }
}

impl Default for SocketChecks {
    fn default() -> SocketChecks {
        SocketChecks::new()
    }
}

/// What the socket's handshake said of CSRF, put in the request of each join for the
/// [`live`] route to hold against the session it finds.
#[derive(Clone)]
enum SocketCsrf {
    /// The `_csrf_token` the socket was opened with.
    Token(String),
    /// The application turned the check off.
    Unchecked,
}

/// The version of the Phoenix LiveView client this server speaks to (the wire protocol is Phoenix's, unchanged). The
/// client compares it with its own and warns when they differ.
const CLIENT_VERSION: &str = "1.2.12";

/// How long after the Dead render its tokens still open a join. Phoenix's value.
const TOKEN_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// What the socket puts in the request it sends through the pages for a join. The
/// [`live`] route that the URL reaches leaves its LiveView here, ready to start,
/// instead of answering with a page.
#[derive(Clone)]
struct Join {
    /// Which routes the join may mount.
    expect: Expect,
    found: Arc<Mutex<Option<Found>>>,
}

/// What the LiveView a join mounts must be.
#[derive(Clone)]
enum Expect {
    /// A join of a page: the LiveView the session token was signed for, in the live
    /// session it was signed in. Another one does not mount.
    Page {
        view: String,
        live_session: Option<String>,
    },
    /// A join that follows a live navigation: any LiveView, in the live session the
    /// token was signed in. A route in another live session does not mount: the
    /// browser loads that page as a page, which gives it the session that route's
    /// Pipeline has.
    LiveSession(Option<String>),
}

/// The route a join reached, ready to start.
struct Found {
    /// The LiveView's type name, for the log.
    view: String,
    start: Start,
}

/// What the socket puts in the request it sends through the pages to find out where a
/// URL leads, without mounting anything: the LiveView of the route it reaches, the
/// live session of the route, and the route's parameters, for a patch.
#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Option<Probed>>>);

struct Probed {
    view: &'static str,
    live_session: Option<String>,
    /// The `Params` of that LiveView.
    params: Box<dyn Any + Send>,
}

/// The name of the live session a route is in, put in the request by the Scope that
/// has it, for the [`live`] route to find. Routes in none are in the default one.
#[derive(Clone)]
pub(crate) struct LiveSession(pub(crate) String);

/// Starts a LiveView on its channel. The future is the LiveView's task: it mounts the
/// LiveView and then handles what its queue brings.
pub(crate) type Start =
    Box<dyn FnOnce(Channel) -> Pin<Box<dyn Future<Output = Result<(), Failed>> + Send>> + Send>;

/// How a LiveView's task ends when something went wrong that the application did not
/// provide for: an unexpected failure. It has been logged by whoever returns this. The
/// state is dropped with the task, and the socket, which sees the task end, tells the
/// client that its channel crashed. The client then joins again, which mounts afresh.
pub(crate) struct Failed;

/// A connected LiveView's end of the socket.
pub(crate) struct Channel {
    /// The `phx_join` that opened it.
    pub(crate) join: Message,
    /// What the browser and the server side send to this LiveView, in arrival order.
    pub(crate) incoming: mpsc::UnboundedReceiver<Incoming>,
    /// The sending end of `incoming`, for the LiveView's [`Handle`]s. It is weak: the
    /// socket holds the only strong one, so the queue closes when the socket lets go.
    pub(crate) queue: mpsc::WeakUnboundedSender<Incoming>,
    /// Frames for the browser.
    pub(crate) outgoing: mpsc::UnboundedSender<String>,
    /// The pages, to find where a patch leads.
    pub(crate) pages: Router,
}

#[derive(Deserialize)]
struct JoinPayload {
    /// The page's URL: a join of a page.
    url: Option<String>,
    /// The URL a live navigation went to: a join that follows one.
    redirect: Option<String>,
    session: String,
    #[serde(rename = "static")]
    static_token: String,
}

/// Finds the LiveView a `phx_join` asks for, if the join carries the tokens of a Dead
/// render of that LiveView: its type name, for the log, and what starts it. The error
/// is the reason the client is told; either one makes it load the page again.
pub(crate) async fn join(
    pages: &mut Router,
    key: &SigningKey,
    message: &Message,
) -> Result<(String, Start), &'static str> {
    let payload = JoinPayload::deserialize(&message.payload).map_err(|_| "stale")?;
    let session =
        SessionToken::verify(key, &payload.session, TOKEN_MAX_AGE).map_err(|_| "stale")?;
    let static_token =
        StaticToken::verify(key, &payload.static_token, TOKEN_MAX_AGE).map_err(|_| "stale")?;
    if message.topic != format!("lv:{}", session.id) || static_token.id != session.id {
        return Err("stale");
    }

    let live_session = session.live_session;
    let (url, expect) = match (payload.url, payload.redirect) {
        (Some(url), None) => {
            let view = session.view;
            (url, Expect::Page { view, live_session })
        }
        // The token is the old page's. What the new one must be is what that page's
        // live session says: only its own, as the token was signed for.
        (None, Some(url)) => (url, Expect::LiveSession(live_session)),
        _ => return Err("stale"),
    };

    // Only the path and the query are taken from it, never the host: it goes through
    // the pages as a GET would, so it reaches the same route, through the same layers,
    // with the same parameters.
    let url: Uri = url.parse().map_err(|_| "unauthorized")?;
    let path = url.path_and_query().map_or("/", |path| path.as_str());
    let of_a_page = matches!(expect, Expect::Page { .. });
    let join = Join {
        expect,
        found: Arc::default(),
    };
    let request = Request::builder()
        .method(Method::TRACE)
        .uri(path)
        .extension(join.clone())
        .body(Body::empty());
    let request = request.map_err(|_| "unauthorized")?;
    let Ok(response) = pages.call(request).await;
    let found = join.found.lock().expect("nothing panics holding it").take();
    // The page's own URL is a LiveView's route, which answers the lookup. A `405` or
    // `501` there is something else answering it: a Layer that refuses `TRACE`, or a
    // route that is not a LiveView's. The client then loads the page again, which
    // joins again: say why. Only the path is named, never a header or the query.
    // (A navigation's target may well be a controller, which answers `405`: no warning.)
    let status = response.status();
    if found.is_none()
        && of_a_page
        && [StatusCode::METHOD_NOT_ALLOWED, StatusCode::NOT_IMPLEMENTED].contains(&status)
    {
        tracing::warn!(
            path = url.path(),
            %status,
            "a join was refused: the lookup of the page's URL, a `TRACE` request, was \
             answered by something that is not a LiveView. A Layer that refuses `TRACE`, or \
             a route that is not a LiveView's, makes the browser load the page again, over \
             and over"
        );
    }
    let Found { view, start } = found.ok_or("unauthorized")?;
    Ok((view, start))
}

/// Where `to` leads: the route it reaches, if it is a LiveView's. Nothing is mounted.
/// It goes through the pages as a GET would, as a join's URL does.
async fn probe(pages: &mut Router, to: &str) -> Option<(Uri, Probed)> {
    let url: Uri = to.parse().ok()?;
    let path = url.path_and_query()?.as_str();
    let probe = Probe::default();
    let request = Request::builder()
        .method(Method::TRACE)
        .uri(path)
        .extension(probe.clone());
    let Ok(_) = pages.call(request.body(Body::empty()).ok()?).await;
    let probed = probe.0.lock().expect("nothing panics holding it").take()?;
    Some((path.parse().ok()?, probed))
}

/// The parameters at `to`, if it is a route of the LiveView `V` in the live session
/// given. A patch goes nowhere else.
async fn resolve<V: LiveView>(
    pages: &mut Router,
    to: &str,
    live_session: &Option<String>,
) -> Option<(Uri, V::Params)> {
    let (url, probed) = probe(pages, to).await?;
    if probed.view != std::any::type_name::<V>() || probed.live_session != *live_session {
        return None;
    }
    let params = probed.params.downcast::<V::Params>().ok()?;
    Some((url, *params))
}

/// The task of one connected LiveView: it owns the state (structured concurrency, no actor runtime).
/// Mount and the handlers are given the one Socket, so what mount pushed goes with the
/// Connected render.
async fn run<V: LiveView>(
    params: V::Params,
    handled: V::Params,
    url: Uri,
    live_session: Option<String>,
    session: Session,
    mut channel: Channel,
) -> Result<(), Failed> {
    let mut socket = Socket::new::<V>(true, session, url, channel.queue.clone());
    let mut view = V::mount(params, &mut socket).await;
    if let Err(error) = view.handle_params(handled, &mut socket).await {
        params_failed::<V>(&error);
        return Err(Failed);
    }
    let mut state = DiffState::new();
    let patched = match settle(&mut view, &mut socket, &mut channel.pages, &live_session).await? {
        // The join is answered with where to go instead of a page.
        Settled::Leave(navigation) => {
            let (event, payload) = leave::<V>(&mut socket, navigation)?;
            let _ = channel
                .outgoing
                .send(channel.join.reply("error", object(event, payload)));
            return Ok(());
        }
        Settled::Render { patched } => patched,
    };
    let mut rendered = state.render(view.render());
    socket.flush(&mut rendered, false);
    let mut reply = json!({"rendered": rendered, "liveview_version": CLIENT_VERSION});
    if let Some(to) = patched {
        reply["live_patch"] = json!({"to": to, "kind": "push"});
    }
    // A frame that cannot be sent means the connection is gone, and this task with it.
    let _ = channel.outgoing.send(channel.join.reply("ok", reply));

    // One at a time, in arrival order, each followed by its own render.
    loop {
        let incoming = tokio::select! {
            incoming = channel.incoming.recv() => incoming,
            // A task the LiveView started is done. One that panicked is an unexpected
            // failure of the LiveView, not dropped silently: the page may be waiting
            // for what that task was to send.
            Some(ended) = socket.tasks.join_next() => {
                if ended.is_err_and(|error| error.is_panic()) {
                    // Not the panic's message: it may hold values.
                    tracing::error!(
                        live_view = std::any::type_name::<V>(),
                        "a task of the LiveView panicked: the LiveView ends"
                    );
                    return Err(Failed);
                }
                continue;
            }
        };
        // The queue closes when the socket lets go of it: on another join of the topic.
        let Some(incoming) = incoming else { break };
        let message = match incoming {
            Incoming::Frame(message) => message,
            Incoming::Message(message) => {
                // Only a `Handle<V>` queues one, and it takes no other type.
                let message = message.downcast::<V::Message>();
                let message = message.expect("a Message of the LiveView's type");
                if let Err(error) = view.handle_message(*message, &mut socket).await {
                    tracing::error!(
                        live_view = std::any::type_name::<V>(),
                        chain = ?crate::error::chain(&error),
                        "the handler of a Message failed: the LiveView ends"
                    );
                    return Err(Failed);
                }
                // What the handler pushed goes with this diff, not with the answer to
                // some later Event.
                let sent = answer(
                    &mut view,
                    &mut socket,
                    &mut state,
                    &mut channel,
                    &live_session,
                    None,
                    false,
                );
                sent.await?;
                continue;
            }
        };
        if message.event == "phx_leave" {
            let _ = channel.outgoing.send(message.reply("ok", json!({})));
            let _ = channel.outgoing.send(channel.join.close());
            break;
        }
        if message.event == "live_patch" {
            // A click on a patch link. The URL is the browser's word: it is only looked
            // up in the routes, as a join's is, and followed if it is a route of this
            // LiveView in this live session. Any other is a navigation instead.
            let to = message.payload["url"].as_str().unwrap_or_default();
            let Some((url, params)) = resolve::<V>(&mut channel.pages, to, &live_session).await
            else {
                let _ = channel
                    .outgoing
                    .send(message.reply("ok", json!({"link_redirect": true})));
                continue;
            };
            socket.url = url;
            socket.flash.clear();
            if let Err(error) = view.handle_params(params, &mut socket).await {
                params_failed::<V>(&error);
                return Err(Failed);
            }
            let request = Some(&message);
            let sent = answer(
                &mut view,
                &mut socket,
                &mut state,
                &mut channel,
                &live_session,
                request,
                false,
            );
            sent.await?;
            continue;
        }
        if message.event != "event" {
            // Not silently dropped: the client learns that nothing was done.
            let _ = channel.outgoing.send(message.reply("error", json!({})));
            continue;
        }
        // The payload of an `event` is the client's `{type, event, value}`. Neither
        // index panics, whatever the payload is: what is not there is null.
        let name = message.payload["event"].as_str().unwrap_or_default();
        let payload = Payload(&message.payload["value"]);
        let event = match V::Event::decode(name, &payload) {
            Ok(event) => event,
            // The browser sent what no template of this LiveView binds.
            Err(error) => {
                // The name is the browser's, so it is a field of its own, which a
                // subscriber quotes, and cut short. What came with it may be a secret
                // and is not logged; the error names at most a field.
                tracing::error!(
                    live_view = std::any::type_name::<V>(),
                    event = &name[..name.floor_char_boundary(100)],
                    chain = ?crate::error::chain(&error),
                    "an Event could not be decoded: the LiveView ends"
                );
                return Err(Failed);
            }
        };
        if let Err(error) = view.handle_event(event, &mut socket).await {
            tracing::error!(
                live_view = std::any::type_name::<V>(),
                event = &name[..name.floor_char_boundary(100)],
                chain = ?crate::error::chain(&error),
                "the handler of an Event failed: the LiveView ends"
            );
            return Err(Failed);
        }
        let request = Some(&message);
        let sent = answer(
            &mut view,
            &mut socket,
            &mut state,
            &mut channel,
            &live_session,
            request,
            true,
        );
        sent.await?;
    }
    Ok(())
}

/// What a callback left to do, once the patches it asked for are followed.
enum Settled {
    /// Render the page. `patched` is where the last patch went, if one did.
    Render { patched: Option<String> },
    /// The browser is going elsewhere.
    Leave(Navigation),
}

/// The most patches in a row, which a `handle_params` that patches to where it is
/// would never stop making.
const MAX_PATCHES: usize = 20;

/// Follows the patches a callback asked for: each is the parameters of a path of this
/// LiveView, handled, which may ask for another.
async fn settle<V: LiveView>(
    view: &mut V,
    socket: &mut Socket,
    pages: &mut Router,
    live_session: &Option<String>,
) -> Result<Settled, Failed> {
    let mut patched = None;
    for _ in 0..MAX_PATCHES {
        match socket.navigation.take() {
            None => return Ok(Settled::Render { patched }),
            Some(Navigation::Patch(to)) => {
                // As Phoenix does: flash is for the redirect that follows the call
                // that left it, not for one after a patch.
                socket.flash.clear();
                let Some((url, params)) = resolve::<V>(pages, &to, live_session).await else {
                    // Not the target's text: it may be made of what a user typed.
                    tracing::error!(
                        live_view = std::any::type_name::<V>(),
                        "a patch went to a path that is not a route of this LiveView, in its \
                         live session: the LiveView ends"
                    );
                    return Err(Failed);
                };
                socket.url = url;
                if let Err(error) = view.handle_params(params, socket).await {
                    params_failed::<V>(&error);
                    return Err(Failed);
                }
                patched = Some(to);
            }
            Some(leave) => return Ok(Settled::Leave(leave)),
        }
    }
    tracing::error!(
        live_view = std::any::type_name::<V>(),
        "the LiveView patched {MAX_PATCHES} times in a row, which is a loop in its \
         `handle_params`: the LiveView ends"
    );
    Err(Failed)
}

fn params_failed<V: LiveView>(error: &V::Error) {
    tracing::error!(
        live_view = std::any::type_name::<V>(),
        chain = ?crate::error::chain(error),
        "the handler of the parameters failed: the LiveView ends"
    );
}

/// `{name: value}`.
fn object(name: &str, value: Value) -> Value {
    Value::Object(serde_json::Map::from_iter([(name.to_owned(), value)]))
}

/// The frame's event and payload that tell the client to go where `navigation` says,
/// the way Phoenix has them: `redirect` loads a page, and carries the flash as a
/// signed token for the browser to hand to it; `live_redirect` joins another
/// LiveView.
fn leave<V: LiveView>(
    socket: &mut Socket,
    navigation: Navigation,
) -> Result<(&'static str, Value), Failed> {
    match navigation {
        Navigation::Redirect(to) => {
            let mut payload = json!({"to": to});
            match socket.flash_token() {
                Ok(Some(token)) => payload["flash"] = json!(token),
                Ok(None) => {}
                Err(()) => {
                    tracing::error!(
                        live_view = std::any::type_name::<V>(),
                        "the LiveView redirected with flash, which needs the Scope of its route \
                         to be piped through a `SessionLayer`: the LiveView ends"
                    );
                    return Err(Failed);
                }
            }
            // It has gone with this redirect: a later one does not carry it again.
            socket.flash.clear();
            Ok(("redirect", payload))
        }
        Navigation::Navigate(to) => Ok(("live_redirect", json!({"to": to, "kind": "push"}))),
        Navigation::Patch(_) => unreachable!("`settle` follows every patch"),
    }
}

/// Renders the page after a callback and tells the browser, or tells it where to go.
/// `request` is the frame it answers, if one: an Event or a patch link.
async fn answer<V: LiveView>(
    view: &mut V,
    socket: &mut Socket,
    state: &mut DiffState,
    channel: &mut Channel,
    live_session: &Option<String>,
    request: Option<&Message>,
    answers_event: bool,
) -> Result<(), Failed> {
    let patched = match settle(view, socket, &mut channel.pages, live_session).await? {
        Settled::Leave(navigation) => {
            // Phoenix answers an Event with where to go, and has no render to send.
            // What was pushed with `push_event` before the call is lost.
            let (event, payload) = leave::<V>(socket, navigation)?;
            let frame = match request {
                Some(request) => request.reply("ok", object(event, payload)),
                None => channel.join.push(event, payload),
            };
            let _ = channel.outgoing.send(frame);
            return Ok(());
        }
        Settled::Render { patched } => patched,
    };
    let mut diff = state.render(view.render());
    socket.flush(&mut diff, answers_event);
    if let Some(to) = patched {
        // Before the diff, which is rendered for the new URL.
        let frame = channel
            .join
            .push("live_patch", json!({"to": to, "kind": "push"}));
        let _ = channel.outgoing.send(frame);
    }
    let no_change = diff.as_object().is_some_and(|diff| diff.is_empty());
    match request {
        // Phoenix's answer to an empty diff is an empty reply.
        Some(request) if no_change => {
            let _ = channel.outgoing.send(request.reply("ok", json!({})));
        }
        Some(request) => {
            let _ = channel
                .outgoing
                .send(request.reply("ok", json!({"diff": diff})));
        }
        // Phoenix pushes nothing when nothing changed and nothing was pushed.
        None if !no_change => {
            let _ = channel.outgoing.send(channel.join.push("diff", diff));
        }
        None => {}
    }
    Ok(())
}

/// Answers a GET with the Dead render, and a join's request with the mounted LiveView.
#[allow(clippy::too_many_arguments)]
async fn live_route<V: LiveView>(
    State(key): State<SigningKey>,
    join: Option<Extension<Join>>,
    probe: Option<Extension<Probe>>,
    live_session: Option<Extension<LiveSession>>,
    session: Option<Extension<Session>>,
    csrf: Option<Extension<SocketCsrf>>,
    method: Method,
    url: Uri,
    params: Result<Path<V::Params>, PathRejection>,
    // The same parameters again, for `handle_params` after mount.
    handled: Result<Path<V::Params>, PathRejection>,
) -> Response {
    let live_session = live_session.map(|Extension(LiveSession(name))| name);
    // A real TRACE is no visit to a page: only the socket's own lookups carry these.
    if method == Method::TRACE && join.is_none() && probe.is_none() {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    // Only a session a `SessionLayer` gave lasts, so only that one gets a CSRF token.
    let lasting_session = session.is_some();
    // Without a `SessionLayer` there is an empty session to read.
    let session = session
        .map(|Extension(session)| session)
        .unwrap_or_default();
    let params = match params {
        Ok(Path(params)) => params,
        // A path whose parameters do not parse is not this route's path.
        Err(rejection) if rejection.status() == StatusCode::BAD_REQUEST => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(rejection) => return rejection.into_response(),
    };
    // Only the socket makes these extensions: a request from outside cannot carry them.
    // Nothing is mounted, so there is nothing to check but what the route is.
    if let Some(Extension(Probe(found))) = probe {
        let (view, params) = (std::any::type_name::<V>(), Box::new(params));
        let probed = Probed {
            view,
            live_session,
            params,
        };
        *found.lock().expect("nothing panics holding it") = Some(probed);
        return StatusCode::NO_CONTENT.into_response();
    }
    if let Some(Extension(join)) = join {
        let allowed = match &join.expect {
            Expect::Page {
                view,
                live_session: signed_in,
            } => *view == std::any::type_name::<V>() && *signed_in == live_session,
            Expect::LiveSession(signed_in) => *signed_in == live_session,
        };
        if !allowed {
            return StatusCode::FORBIDDEN.into_response();
        }
        // The socket must have been opened with a CSRF token of the session its
        // cookies hold, unless the application turned that check off. Refused before
        // the LiveView is given a task, and so before mount, and the client loads the
        // page again (see `SocketChecks`).
        let genuine = match csrf {
            Some(Extension(SocketCsrf::Token(token))) => session.csrf_token_matches(&token),
            Some(Extension(SocketCsrf::Unchecked)) => true,
            None => false,
        };
        if !genuine {
            // Neither token is logged: the one sent may be another session's.
            tracing::warn!(
                live_view = std::any::type_name::<V>(),
                "a join was refused: the socket's CSRF token is not its session's"
            );
            return StatusCode::FORBIDDEN.into_response();
        }
        // Mount waits for the LiveView's task, so that it runs where the handlers do:
        // a slow one holds up no other frame of the socket, and one that panics ends
        // this LiveView alone.
        let Ok(Path(handled)) = handled else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let run: Start = Box::new(move |channel| {
            Box::pin(run::<V>(
                params,
                handled,
                url,
                live_session,
                session,
                channel,
            ))
        });
        let view = std::any::type_name::<V>().to_owned();
        *join.found.lock().expect("nothing panics holding it") = Some(Found { view, start: run });
        return StatusCode::NO_CONTENT.into_response();
    }
    // For the page's script to open the socket with (see `SocketChecks`).
    let csrf_token = lasting_session.then(|| session.csrf_token());
    // No task takes from the queue of a Dead render, so a `Handle` made of this Socket
    // reports `Ended`.
    let no_queue = mpsc::unbounded_channel().0.downgrade();
    let mut socket = Socket::new::<V>(false, session, url, no_queue);
    let mut view = V::mount(params, &mut socket).await;
    let Ok(Path(handled)) = handled else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Err(error) = view.handle_params(handled, &mut socket).await {
        params_failed::<V>(&error);
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    // Where `handle_params` sent the page, a browser is sent with a redirect. A patch is
    // the same, as there is no connection to patch over, and a navigation is.
    if let Some(navigation) = socket.navigation.take() {
        let (Navigation::Patch(to) | Navigation::Navigate(to) | Navigation::Redirect(to)) =
            &navigation;
        // Flash goes with a redirect alone. It stays in the session, which shows it on
        // the page this leads to, and without a `SessionLayer` there is nothing to keep
        // it in: that is a failure, as it is in the Connected render.
        let flash = std::mem::take(&mut socket.flash);
        if !flash.is_empty() && matches!(navigation, Navigation::Redirect(_)) {
            if !lasting_session {
                return crate::error::Unexpected::new(FlashWithoutSession).into_response();
            }
            for (kind, message) in flash {
                socket.session.put_flash(&kind, message);
            }
        }
        return Redirect::to(to).into_response();
    }

    let id = unique_id();
    let session = SessionToken {
        id: id.clone(),
        view: std::any::type_name::<V>().to_owned(),
        live_session,
    };
    let session = key.sign(SessionToken::PURPOSE, &session);
    let static_token = key.sign(StaticToken::PURPOSE, &StaticToken { id: id.clone() });

    // The element the client attaches the LiveView to. It is never diffed, so its
    // fingerprint is never read.
    use crate::template::Slot;
    let container = Rendered::new(
        0,
        &[
            "<div id=\"",
            "\" data-phx-main data-phx-session=\"",
            "\" data-phx-static=\"",
            "\"",
            ">",
            "</div>",
        ],
        vec![
            Slot::text(&id),
            Slot::text(&session),
            Slot::text(&static_token),
            Slot::attribute("data-csrf-token", &csrf_token),
            Slot::template(view.render()),
        ],
    );

    // Outside a Scope with a layout, a fixed root document. Drop it once
    // every live route has a layout from the route table.
    // Always there: the client sets `document.title` from it on mount, and an absent one
    // would show as "undefined".
    let title = Rendered::new(
        0,
        &["<title>", "</title>\n"],
        vec![Slot::text(socket.title.as_deref().unwrap_or(""))],
    )
    .to_html();
    let mut response = Html(format!(
        "<!DOCTYPE html>\n\
         <html>\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         {title}\
         <script defer type=\"module\" src=\"/assets/app.js\"></script>\n\
         </head>\n\
         <body>\n\
         {}\n\
         </body>\n\
         </html>\n",
        container.to_html()
    ))
    .into_response();
    // The layout of a Scope wraps the container, as it wraps a controller's page.
    response
        .extensions_mut()
        .insert(crate::router::Page(container));
    response
}

/// An id for the container element, unique among the pages this process renders.
/// It is not a secret. Made like Phoenix's: the time and a counter.
fn unique_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH);
    let nanos = since_epoch.map_or(0, |elapsed| elapsed.as_nanos() as u64);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let bytes = [nanos.to_be_bytes().as_slice(), &count.to_be_bytes()].concat();
    format!("phx-{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// What the Dead render signs into the container's `data-phx-session` attribute. The
/// client sends it back when it joins, which proves the join follows a page this
/// server rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionToken {
    /// The container element's id. The client joins the topic `lv:<id>`.
    pub id: String,
    /// The LiveView the page was rendered from, as its Rust type name.
    pub view: String,
    /// The live session of the route it is the page of: a join that follows a live
    /// navigation must be to a route in the same one. `None` for a route in none.
    #[serde(default)]
    pub live_session: Option<String>,
}

impl SessionToken {
    const PURPOSE: &str = "live session";

    /// Reads a `data-phx-session` value: `key` signed it, less than `max_age` ago.
    pub fn verify(
        key: &SigningKey,
        token: &str,
        max_age: Duration,
    ) -> Result<SessionToken, TokenError> {
        key.verify(SessionToken::PURPOSE, token, max_age)
    }
}

/// What the Dead render signs into the container's `data-phx-static` attribute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaticToken {
    /// The container element's id, the same as in the [`SessionToken`] beside it.
    pub id: String,
}

impl StaticToken {
    const PURPOSE: &str = "live static";

    /// Reads a `data-phx-static` value: `key` signed it, less than `max_age` ago.
    pub fn verify(
        key: &SigningKey,
        token: &str,
        max_age: Duration,
    ) -> Result<StaticToken, TokenError> {
        key.verify(StaticToken::PURPOSE, token, max_age)
    }
}
