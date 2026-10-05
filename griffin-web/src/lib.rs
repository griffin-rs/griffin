//! Part of the Griffin web framework: <https://github.com/griffin-rs/griffin>.
//!
//! Griffin is still being designed. So far this crate has templates (the [`html!`]
//! macro to write them inline and [`html_file!`] to write them in files of their own,
//! [`#[component]`](macro@component) to make reusable pieces of them, and
//! [`template`], the low-level model the macros expand to),
//! LiveViews in [`live`] (the Dead render, then the Connected render and Events over
//! a socket), the signed tokens they carry in [`token`], the route table of
//! [`routes!`] with its Path helpers, the Scopes of routes with a layout that it
//! expands to in [`router`], for handlers that return a template, the Layers a Scope's
//! requests pass through in [`pipeline`], the cookie session and flash in [`session`],
//! the Layers that harden a browser's pages in [`security`], typed configuration in
//! [`config`], and error pages in [`error`].

// So that `html!` can be used inside this crate: it expands to `::griffin_web::` paths.
extern crate self as griffin_web;

pub mod config;
#[cfg(feature = "dev")]
pub mod dev;
pub mod error;
pub mod live;
pub mod pipeline;
pub mod router;
pub mod security;
pub mod session;
pub mod static_files;
pub mod template;
pub mod token;
mod transport;

/// The web server Griffin is built on, re-exported unwrapped: routers,
/// handlers and extractors are axum's own.
pub use axum;

/// Writes a template inline: HTML with Rust expressions in braces. The value is a
/// [`template::Rendered`].
///
/// ```
/// use griffin_web::html;
/// use griffin_web::template::Rendered;
///
/// struct Counter {
///     count: u32,
///     label: String,
/// }
///
/// impl Counter {
///     fn render(&self) -> Rendered {
///         html! {
///             <section class="counter">
///                 <h1>{@label}</h1>
///                 <button phx-click="inc" disabled={@count >= 10}>Clicked {@count} times</button>
///             </section>
///         }
///     }
/// }
///
/// let counter = Counter { count: 3, label: "Likes & shares".to_owned() };
/// assert_eq!(
///     counter.render().to_html(),
///     "<section class=\"counter\">\n    \
///          <h1>Likes &amp; shares</h1>\n    \
///          <button phx-click=\"inc\">Clicked 3 times</button>\n\
///      </section>"
/// );
/// ```
///
/// # Syntax
///
/// - **Elements and text** are HTML and are sent as written. A void element such as
///   `<br>` has no closing tag; `<div />` is short for `<div></div>`.
/// - **`{expression}`** is any Rust expression, type-checked by rustc like the code
///   around it. As content its value is any [`Display`](std::fmt::Display) type, and
///   it is HTML-escaped.
/// - **`{Slot::raw_html(trusted)}`** is the one way to send HTML unescaped. See
///   [`Slot::raw_html`](template::Slot::raw_html) for what the caller vouches for.
/// - **`@name`** in an expression is short for `self.name`: an Assign read from the
///   state the template is rendered from.
/// - **`name="value"`** and the bare **`name`** are static attributes.
/// - **`name={expression}`** is a dynamic attribute. Its value is escaped. A `bool`
///   writes the bare name when true and nothing when false; an `Option` writes
///   nothing when `None`. See [`AttributeValue`](template::AttributeValue) for the
///   types it takes. `class` and `style` are always written and take any `Display` type.
/// - **`phx-click={event}`**, or any other attribute, whose value is an
///   [`Event`](live::Event) of a LiveView binds that Event to the element: the
///   attribute gets the Event's name, and a `phx-value-<field>` attribute beside it
///   carries each field. `<button phx-click={TodoEvent::Delete { id: 7 }}>` is
///   `<button phx-click="delete" phx-value-id="7">`, and the handler is given
///   `TodoEvent::Delete { id: 7 }` back. A variant that does not exist does not
///   compile. A name as plain text, `phx-click="delete"`, binds the same Event.
/// - **`{expression}` inside a tag** spreads attributes decided at run time: an iterator
///   of `(name, value)` pairs, as taken by
///   [`Slot::attributes`](template::Slot::attributes). Names must not come from user input.
/// - **`:if={condition}`** on an element renders the element only when the `bool`
///   holds: `<p :if={@count > 0}>{@count} new</p>`.
/// - **`:for={pattern in iterable}`** on an element renders it once for each item, the
///   two parts being those of a Rust `for` loop:
///   `<li :for={(id, name) in &@guests} :key={id}>{name}</li>`. An `:if` beside it
///   can read what the pattern binds and leaves items out.
/// - **`:key={expression}`** beside a `:for` tells the items apart, by any
///   [`Display`](std::fmt::Display) value that no two items share, typically an id.
///   A LiveView then sends where each item moved to instead of what is in it. Without
///   a key, an item is the one that was at its position.
/// - **A capitalised tag** calls a [`Component`](template::Component), a reusable
///   piece of template: `<Badge label="new" count={@unread} />`. Its attributes are the
///   Component's, typed and checked by rustc, and `:if`, `:for` and `:key` work on it
///   as on an element. See [`#[component]`](macro@component).
/// - **Content between the tags of a Component** is given to its default slot, for the
///   Component to render where it likes: `<Card title="Guest">Hello, {@name}!</Card>`.
/// - **`<:name>`** directly inside a Component gives one entry to its slot `name`, with
///   attributes of its own if the slot declares any:
///   `<Table rows={&@users}><:col label="Name">...</:col></Table>`. A slot may be given
///   more than once. Text after a slot entry loses its leading whitespace.
/// - **`:let={pattern}`** on a slot entry binds the argument the Component passes to
///   that content, as a closure parameter would: `<:col :let={user}>{user.name}</:col>`.
///   On the Component's tag it binds the argument of the default slot.
/// - **`<!-- comments -->`, a `<!doctype>`, and the content of `<script>` and
///   `<style>`** are sent as written: a `<` or a `{` in them starts nothing.
/// - **`if` and `match`** are Rust's own, with a nested `html!` as the value of each
///   branch: `{if @admin { html! { <b>Admin</b> } } else { html! { <i>Guest</i> } }}`.
///   An `if` without a final `else` renders nothing when no branch is taken.
///
/// An element with `:if` and each branch are templates of their own, as by
/// [`Slot::template`](template::Slot::template): a LiveView sends their static parts
/// when the branch is entered, and only the slots that changed while it stays. An
/// element with `:for` is a comprehension, as by
/// [`Slot::comprehension`](template::Slot::comprehension) or, with a key,
/// [`Slot::keyed_comprehension`](template::Slot::keyed_comprehension): its static
/// parts are sent once for all items.
///
/// Expressions are borrowed, not moved, so `{@label}` works for a `String` field.
/// The iterator of a spread, the iterable of a `:for` and the attributes of a
/// Component or a slot entry are the exceptions: they are taken by value, so give a
/// reference, `:for={guest in &@guests}` and `<Badge label={&@label} />`. The content
/// given to a slot is a closure the Component may call more than once, so it cannot
/// move what it captures either.
///
/// # What it expands to
///
/// Nothing but the documented API in [`template`], which can also be written by
/// hand (every macro lowers to a public API):
///
/// ```
/// use griffin_web::html;
/// use griffin_web::template::{Rendered, Slot};
///
/// let (name, busy) = ("Ann", true);
/// let by_macro = html! { <button disabled={busy}>Hello, {name}!</button> };
/// let by_hand = Rendered::new(
///     0, // The macro hashes the statics here.
///     &["<button", ">Hello, ", "!</button>"],
///     vec![Slot::attribute("disabled", &(busy)), Slot::from(&(name))],
/// )
/// .single_root();
///
/// assert_eq!(by_macro.to_html(), by_hand.to_html());
/// ```
///
/// # Limits of living in Rust source
///
/// The template is read by Rust's lexer before the macro sees it, so its text must
/// be made of Rust tokens: an apostrophe or a lone quote can end the compile with a
/// lexer error, and `//` starts a Rust comment. Put such text in an expression,
/// `{"it's"}`. Spacing is rebuilt from where the tokens are, with indentation counted
/// from the first one. Attribute values need double quotes.
///
/// A template in a file of its own has none of these limits: see [`html_file!`].
pub use griffin_macros::html;

/// Reads a template from a file of its own: the language of [`html!`], as plain HTML
/// text. The value is a [`template::Rendered`], the one `html!` would give for the
/// same template.
///
/// The file is named relative to the **`templates` directory of the crate**, the one
/// beside its `Cargo.toml`, and ends in `.html.griffin` by convention. This is
/// `templates/counter.html.griffin`:
///
/// ```html
/// <section class="counter">
///     <h1>{@label}</h1>
///     <button phx-click="inc" disabled={@count >= 10}>Clicked {@count} times</button>
///     <p :if={@count == 0}>Don't be shy.</p>
/// </section>
/// ```
///
/// It is bound to a struct by being used in a method of it: `@name` is `self.name`
/// there, as in `html!`. An expression also reads any other name in scope at the
/// macro call, so a file can be the template of a [`#[component]`](macro@component)
/// function as well.
///
/// ```
/// use griffin_web::html_file;
/// use griffin_web::template::Rendered;
///
/// struct Counter {
///     count: u32,
///     label: String,
/// }
///
/// impl Counter {
///     fn render(&self) -> Rendered {
///         html_file!("counter.html.griffin")
///     }
/// }
///
/// let counter = Counter { count: 0, label: "Likes".to_owned() };
/// assert_eq!(
///     counter.render().to_html(),
///     "<section class=\"counter\">\n    \
///          <h1>Likes</h1>\n    \
///          <button phx-click=\"inc\">Clicked 0 times</button>\n    \
///          <p>Don't be shy.</p>\n\
///      </section>"
/// );
/// ```
///
/// Editing the file recompiles the crate, as editing its Rust does.
///
/// # What differs from `html!`
///
/// The syntax is that of [`html!`], and the limits of living in Rust source are gone:
///
/// - Text is text. An apostrophe, a lone quote and `//` need nothing special.
/// - An attribute value may be in single quotes: `data-greeting='say "hi"'`.
/// - A `{` in text or in a tag always starts a Rust expression. A brace that is text
///   is written `&lbrace;`. Quoted attribute values, comments, `<script>` and
///   `<style>` are the exceptions: there a brace is a brace.
/// - Whitespace is sent as it is in the file, except that the whitespace before the
///   first tag and after the last is dropped. A file gives the same static parts as
///   the same template in `html!` when it is indented with spaces from the first
///   column and no line ends in spaces: `html!` rebuilds its spacing from where the
///   tokens are, so it turns a tab into one space and drops spaces that end a line.
///
/// # Errors
///
/// No span of a stable proc macro can point into another file, so rustc underlines
/// the `html_file!` call for every error, and the place in the file is given in words:
///
/// - A syntax error of the template starts with the file, line and column:
///   ``/app/templates/counter.html.griffin:4:9: `<li>` is never closed``.
/// - An error rustc finds in an expression, such as a misspelt field or a value of
///   the wrong type, ends with a note that names the file, line and column of the
///   expression, with `_` for each character an identifier cannot have: ``this error
///   originates in the macro `counter_html_griffin_line_3_column_38` ``. For an
///   attribute a Component does not have, the place is that of the tag.
/// - A file that cannot be read is an error that gives the path it was looked for at.
///
/// Three kinds of error come without the note, and so say which file but not where
/// in it: a required attribute or slot that a Component tag leaves out, a `:for`
/// pattern or a `:let` pattern of the wrong shape, and what rustc reports at syntax
/// it rewrites, such as a `?` or an `.await` in an expression. A `:for` over what
/// cannot be iterated is reported twice, the first time with the note.
pub use griffin_macros::html_file;

/// Makes a function a Component: a reusable piece of template with typed attributes,
/// which other templates call as a capitalised tag (a PascalCase tag tells it from an HTML tag).
///
/// ```
/// use griffin_web::template::Rendered;
/// use griffin_web::{component, html};
///
/// #[component]
/// fn Badge(label: &str, #[default(0)] count: u32) -> Rendered {
///     html! { <span class="badge">{label} {count}</span> }
/// }
///
/// let unread = 3;
/// let page = html! { <p>Inbox <Badge label="new" count={unread} /> Drafts <Badge label="none" /></p> };
/// assert_eq!(
///     page.to_html(),
///     "<p>Inbox <span class=\"badge\">new 3</span> Drafts <span class=\"badge\">none 0</span></p>"
/// );
/// ```
///
/// Each parameter is an attribute of the tag, of the parameter's type:
///
/// - A plain parameter is **required**.
/// - **`#[default(expression)]`** makes it optional: it has that value when the tag
///   leaves it out. A bare `#[default]` is the type's own [`Default`].
/// - **`#[global] rest: Attributes`** is not an attribute. It opts the Component in to
///   extra HTML attributes such as `class` and `data-id`, which it then forwards with
///   `<button {rest}>`. See [`GlobalAttributes`](template::GlobalAttributes).
/// - **`#[slot] name: SlotEntries<'_>`** is not an attribute either. It is a slot:
///   template the tag gives, which the Component renders where it likes. See below.
///
/// In a tag, `name={expression}` gives the value of the expression, moved as Rust
/// would move it, `name="text"` a `&'static str`, and a bare `name` gives `true`.
///
/// # Slots
///
/// A parameter marked `#[slot]`, of type [`SlotEntries`](template::SlotEntries), takes
/// what the tag gives between its tags. The one named `inner_block` is the default
/// slot, which takes the content itself; any other takes each `<:name>` entry. A slot
/// is **required** unless it is also marked `#[default]`, and one that is not given
/// renders as nothing. A layout is a Component with a default slot.
///
/// ```
/// use griffin_web::template::{Rendered, SlotEntries};
/// use griffin_web::{component, html};
///
/// struct User {
///     name: &'static str,
///     age: u32,
/// }
///
/// #[component]
/// fn Card(title: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
///     html! { <section><h2>{title}</h2>{inner_block}</section> }
/// }
///
/// /// The attributes of an entry of the slot `col`: a public field for each.
/// #[derive(Default)]
/// struct Col<'a> {
///     label: &'a str,
/// }
///
/// #[component]
/// fn Table(rows: &[User], #[slot] col: SlotEntries<'_, &User, Col<'_>>) -> Rendered {
///     html! {
///         <table>
///             <tr><th :for={col in &col}>{col.label}</th></tr>
///             <tr :for={row in rows}><td :for={col in &col}>{col.render(row)}</td></tr>
///         </table>
///     }
/// }
///
/// let users = [User { name: "Ann", age: 30 }];
/// let page = html! {
///     <Card title="Guests">
///         <Table rows={&users}>
///             <:col label="Name" :let={user}>{user.name}</:col>
///             <:col label="Age" :let={User { age, .. }}>{age} years</:col>
///         </Table>
///     </Card>
/// };
/// assert_eq!(
///     page.to_html(),
///     "<section><h2>Guests</h2>\n    \
///          <table>\n    \
///              <tr><th>Name</th><th>Age</th></tr>\n    \
///              <tr><td>Ann</td><td>30 years</td></tr>\n\
///          </table>\n\
///      </section>"
/// );
/// ```
///
/// `SlotEntries<'_, T, A>` says what the slot is given and gives back: `T` is the type
/// of the argument the Component passes to the content, with `{slot.render(argument)}`,
/// and which the tag binds with `:let`; `A` is a struct of the attributes an entry may
/// carry, each a public field with a default. Both are `()` when left out, and then
/// `{slot}` renders it.
///
/// # Compile errors
///
/// All of these are reported in the template, at the place named:
///
/// - **A value of the wrong type**: rustc's `mismatched types`, at the value.
/// - **A required attribute left out**: ``missing required attribute `label` ``, at
///   the tag.
/// - **A required slot left out**: ``missing required slot `col` ``, at the tag. The
///   default slot is `inner_block`, and content that is only whitespace does not give it.
/// - **An attribute the Component does not have**: rustc's ``no method named `colour`
///   found for struct `Badge` ``, at the attribute. The same for a slot it does not
///   have, at the `<:name>`, and for content when it has no default slot, at the tag.
/// - **An attribute a slot entry does not have**: rustc's ``no field `title` ``, at the
///   attribute.
/// - **A tag that names no Component**: rustc's ``cannot find type `Badge` in this
///   scope``, at the tag, with its usual hints for a name that is misspelt or not
///   imported. A macro cannot look a name up, so these are rustc's words and not
///   Griffin's. A name that is a type but no Component is reported as "is not a
///   Component".
///
/// # What it expands to
///
/// The function stays, and can be called as one: `Badge("new", 3)`. Beside it is a
/// struct of the same name that holds the attributes and slots, with a method to give
/// each one, and an implementation of [`Component`](template::Component) that calls
/// the function. That trait documents what a tag expands to, and how to write a
/// Component by hand (every macro lowers to a public API). Required attributes are checked where a tag is
/// written: rendering the struct directly without one panics.
///
/// # Limits
///
/// - No generic parameters yet. Lifetimes can be left out, as in `&str` and
///   `Form<'_>`, but not the `'_` itself: `SlotEntries<'_>`. The struct and the
///   function hold them all under one, so what a Component passes to a slot by
///   reference must be borrowed from its parameters, not from a local value.
/// - A tag is the name of a Component in scope, not a path: import it.
/// - A Component takes no spread attributes.
/// - A slot entry takes no `:if` or `:for`, and its attributes cannot be required.
pub use griffin_macros::component;

/// Writes the route table of an application: every route in one place, grouped in
/// Scopes, each with its Pipelines and its layout (one `routes!` table). The table becomes the
/// function that builds the router, and a `Routes` with a Path helper for each route
/// the table names and the listing of them all.
///
/// ```
/// use griffin_web::axum::Router;
/// use griffin_web::html;
/// use griffin_web::live::{LiveView, Socket};
/// use griffin_web::pipeline::Pipeline;
/// use griffin_web::router::{Path, Route};
/// use griffin_web::routes;
/// use griffin_web::session::Flash;
/// use griffin_web::template::Rendered;
/// use griffin_web::token::SigningKey;
///
/// routes! {
///     pub fn router(browser: Pipeline<SigningKey>) -> Router<SigningKey>;
///
///     scope "/" {
///         pipe_through [browser];
///         layout site;
///
///         GET "/" => home as home;
///         GET "/users/{id: u32}" => show_user as user;
///         PUT "/users/{id: u32}" => rename_user;
///         LIVE "/counter/{start: i32}" => Counter as counter;
///
///         scope "/admin" {
///             layout |page, _flash| html! { <main class="admin">{page}</main> };
///
///             GET "/users/{id: u32}/posts/{slug: String}" => show_post as admin_post;
///         }
///     }
/// }
///
/// fn site(page: Rendered, _flash: &Flash) -> Rendered {
///     html! { <main>{page}</main> }
/// }
///
/// async fn home() -> Rendered {
///     html! { <a href={Routes::user(7)}>Ann</a> }
/// }
///
/// async fn show_user(Path(id): Path<u32>) -> String {
///     format!("user {id}")
/// }
///
/// async fn rename_user(Path(id): Path<u32>, name: String) -> String {
///     format!("user {id} is now {name}")
/// }
///
/// async fn show_post(Path((id, slug)): Path<(u32, String)>) -> String {
///     format!("post {slug} of user {id}")
/// }
///
/// struct Counter(i32);
///
/// impl LiveView for Counter {
///     type Params = i32;
///     type Event = std::convert::Infallible;
///     type Message = std::convert::Infallible;
///     type Error = std::convert::Infallible;
///
///     async fn mount(start: i32, _socket: &mut Socket) -> Counter {
///         Counter(start)
///     }
///
///     fn render(&self) -> Rendered {
///         html! { <p>{self.0}</p> }
///     }
/// }
///
/// assert_eq!(Routes::home(), "/");
/// assert_eq!(Routes::user(7), "/users/7");
/// assert_eq!(Routes::admin_post(7, "tea/coffee"), "/admin/users/7/posts/tea%2Fcoffee");
/// assert_eq!(
///     Routes::LIST[4],
///     Route {
///         method: "GET",
///         path: "/admin/users/{id}/posts/{slug}",
///         name: Some("admin_post"),
///         target: "show_post",
///     }
/// );
///
/// let key = SigningKey::new("read this from configuration, not from the source")?;
/// let app: Router = router(Pipeline::new()).with_state(key);
/// # Ok::<(), griffin_web::token::SigningKeyTooShort>(())
/// ```
///
/// # The table
///
/// - **The function** comes first: the signature of the function the table becomes,
///   which returns the `Router` and says what its state is, under its doc comment if
///   it has one. Every expression in the
///   table is evaluated in that function, once, so a Pipeline can be made from an
///   argument of it.
/// - **`scope "/prefix" { .. }`** is a Scope: what it holds is under its prefix. A
///   Scope inside a Scope adds its prefix to the outer one, passes the outer Pipelines
///   before its own, and has the outer layout unless it has one itself. What is
///   written outside every Scope is in a root Scope without a prefix.
/// - **`pipe_through [first, second];`** sends the requests of the Scope through
///   Pipelines, in that order. Each is an expression that gives a
///   [`Pipeline`](pipeline::Pipeline).
/// - **`layout site;`** puts the pages of the Scope in a layout, at most one for each
///   Scope: an expression that [`Scope::layout`](router::Scope::layout) takes, such as
///   the name of a function of the page and the flash.
/// - **`GET "/path" => handler;`** routes a verb and a path to a handler, named by its
///   path as Rust names a function: `users::show`. The verbs are `GET`, `POST`, `PUT`,
///   `PATCH`, `DELETE`, `HEAD` and `OPTIONS`. The path `"/"` is the prefix itself.
/// - **`LIVE "/path" => Counter;`** routes a path to a LiveView, which a GET reaches.
///   A table with one has the socket of [`live_socket`](live::live_socket) too, at
///   `/live/websocket`.
/// - **`{id: u32}`** in a path, of a route or of a Scope, is a path parameter with its
///   type, and **`{*rest: String}`** one that takes the rest of the path. The handler
///   takes them as a [`router::Path`]: of the type itself for one, `Path<u32>`, and of
///   a tuple in the order written for several, `Path<(u32, String)>`. A LiveView has
///   them as its [`Params`](live::LiveView::Params), which are `()` for none.
/// - **`as name`** after a route gives it a Path helper, `Routes::name`.
///
/// # Path helpers and the listing
///
/// Beside the function is `struct Routes`, as visible as the function is.
///
/// `Routes::user(7)` is the path of the route named `user`, as a `String` for an
/// `href` or a redirect. Its arguments are the parameters of the path, each of the type
/// the table gives it, except that a `String` is taken as a `&str`. Each is
/// percent-encoded as [`router::encode_segment`] does, so that no value can be more
/// than its one segment. Read there what it cannot keep: an empty value, `.` and `..`.
///
/// `Routes::LIST` is every route of the table as a [`router::Route`], in the order
/// written: its verb, its whole path, its name and its target. The socket is not a
/// route of the table and is not listed.
///
/// # Compile errors
///
/// - **A link to a route that does not exist**: rustc's ``no associated function or
///   constant named `usr` found for struct `Routes` ``, at the call.
/// - **An argument of the wrong type**: rustc's `mismatched types`, at the argument,
///   with a note that points at the route.
/// - **The same verb and path twice**, whatever the parameters are called, and a `LIVE`
///   route being a `GET`: ``duplicate route: `GET /users/{id}` is already routed to
///   `show_user` ``, at the second.
/// - **The same name twice**: ``duplicate route name: `user` is already the name of
///   `GET /users/{id}` ``, at the second.
/// - **A handler that does not take the parameters of its path**: ``this handler does
///   not take the path parameters of its route, which are `(u32, String)` ``, at the
///   handler in the route line. See [`router::takes_path`] for what is checked.
/// - **A LiveView whose `Params` are not the parameters of its path**: rustc's ``type
///   mismatch resolving `<Counter as LiveView>::Params == u32` ``, at the LiveView in
///   the route line, with a note that points at its `type Params`.
/// - **A table that is not written as above** is reported at the word or the path at
///   fault. The first such error ends the reading of the table; the others above are
///   all reported.
///
/// # What it expands to
///
/// Nothing but the builder in [`router`], which can also be called by hand (every macro lowers to a public API).
/// The table above is this function, and a table without a `LIVE` route ends at the
/// `merge`:
///
/// ```
/// # use griffin_web::axum::Router;
/// # use griffin_web::html;
/// # use griffin_web::live::{LiveView, Socket};
/// # use griffin_web::pipeline::Pipeline;
/// # use griffin_web::router::Path;
/// # use griffin_web::session::Flash;
/// # use griffin_web::template::Rendered;
/// # use griffin_web::token::SigningKey;
/// # fn site(page: Rendered, _flash: &Flash) -> Rendered { page }
/// # async fn home() {}
/// # async fn show_user(Path(_id): Path<u32>) {}
/// # async fn rename_user(Path(_id): Path<u32>) {}
/// # async fn show_post(Path(_): Path<(u32, String)>) {}
/// # struct Counter;
/// # impl LiveView for Counter {
/// #     type Params = i32;
/// #     type Event = std::convert::Infallible;
/// #     type Message = std::convert::Infallible;
/// #     type Error = std::convert::Infallible;
/// #     async fn mount(_start: i32, _socket: &mut Socket) -> Counter { Counter }
/// #     fn render(&self) -> Rendered { html! { <p>Count</p> } }
/// # }
/// use griffin_web::axum::routing::{get, put};
/// use griffin_web::live::live_socket;
/// use griffin_web::router::{Scope, live_takes_path, takes_path};
///
/// pub fn router(browser: Pipeline<SigningKey>) -> Router<SigningKey> {
///     let pages = Router::new().merge(
///         Scope::new("/").scope(
///             Scope::new("/")
///                 .pipe_through(browser)
///                 .layout(site)
///                 .route("/", get(home))
///                 .route("/users/{id}", get(takes_path::<u32, _, _>(show_user)))
///                 .route("/users/{id}", put(takes_path::<u32, _, _>(rename_user)))
///                 .route("/counter/{start}", live_takes_path::<Counter, i32, _>())
///                 .scope(
///                     Scope::new("/admin")
///                         .layout(|page, _flash| html! { <main class="admin">{page}</main> })
///                         .route(
///                             "/users/{id}/posts/{slug}",
///                             get(takes_path::<(u32, String), _, _>(show_post)),
///                         ),
///                 ),
///         ),
///     );
///     pages.clone().route("/live/websocket", live_socket(pages))
/// }
/// ```
///
/// Each Path helper is a function that formats the path with
/// [`router::encode_segment`], and the listing is a constant slice.
///
/// # Limits
///
/// - One table, in one file: there is no syntax to split it or to bring another in.
///   What the table cannot say is added to the `Router` the function returns.
/// - A handler of a route with parameters has to take them all, as one
///   [`router::Path`] of exactly the types written. A route without parameters is not
///   checked: a handler that takes a `Path` there fails when a request comes.
/// - The types in a path have no place of their own for rustc to point at, so an
///   error in one is reported at the whole path.
/// - A join of a LiveView sends its URL through every route of the table, so the
///   handler of a `GET` route that a join names runs before the join is refused.
/// - The path of the socket is always `/live/websocket`, and a `GET` route of the
///   table at that path fails when the router is built.
pub use griffin_macros::routes;
