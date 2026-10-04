//! Error pages, through a small app driven as a tower service: an HTTP request in, a
//! response out.

use griffin_web::axum::Router;
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::extract::State;
use griffin_web::axum::http::{HeaderMap, Request, StatusCode, header};
use griffin_web::axum::response::{IntoResponse, Response};
use griffin_web::axum::routing::{get, post};
use griffin_web::config::{self, ConfigError, Secret};
use griffin_web::error::{ErrorPage, ErrorPages, Unexpected};
use griffin_web::html;
use griffin_web::live::{LiveView, Socket, live};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::{Path, Scope};
use griffin_web::session::{Session, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use serde::Deserialize;
use std::convert::Infallible;
use std::fmt::{self, Write as _};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use std::thread::{self, ThreadId};
use tower::ServiceExt as _;

/// What a Context fails with when something it did not provide for went wrong. It
/// knows nothing of HTTP.
#[derive(Debug)]
struct DatabaseDown {
    cause: io::Error,
}

impl fmt::Display for DatabaseDown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("could not load the order")
    }
}

impl std::error::Error for DatabaseDown {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

fn find_order() -> Result<&'static str, DatabaseDown> {
    let cause = io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "connection refused by 10.0.0.5:5432",
    );
    Err(DatabaseDown { cause })
}

/// The application's own error type: every error a handler ends with `?` on.
enum AppError {
    Unexpected(Unexpected),
}

impl From<DatabaseDown> for AppError {
    /// The caller is the `?` of the handler, which is then where it failed.
    #[track_caller]
    fn from(error: DatabaseDown) -> AppError {
        AppError::Unexpected(Unexpected::new(error))
    }
}

/// The one place where an error of the application becomes a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unexpected(failure) => failure.into_response(),
        }
    }
}

const SHOW_ORDER: u32 = line!();
async fn show_order() -> Result<Rendered, AppError> {
    let order = find_order()?;
    Ok(html! { <h1>Order of {order}</h1> })
}

const DASHBOARD: u32 = line!();
async fn dashboard() -> Rendered {
    let user = 7;
    panic!("the cart of user {user} is gone");
}

async fn show_user(Path(id): Path<u32>) -> Response {
    match id {
        // A page of the handler's own, a status and nothing else, and no error at all.
        7 => (StatusCode::NOT_FOUND, html! { <h1>User 7 has left</h1> }).into_response(),
        13 => StatusCode::FORBIDDEN.into_response(),
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

fn routes() -> Router {
    let site = Scope::new("/")
        .layout(|page, _flash| html! { <main class="site">{page}</main> })
        .route("/orders/1", get(show_order))
        .route("/dashboard", get(dashboard))
        .route("/users/{id}", get(show_user));
    Router::new().merge(site)
}

fn app(page: ErrorPage) -> Router {
    routes().layer(ErrorPages::new(page))
}

/// The application's own pages: the ones it has, and Griffin's for the rest.
fn error_page(status: StatusCode) -> Option<Rendered> {
    match status {
        StatusCode::NOT_FOUND => {
            Some(html! { <main class="shop"><h1>We have no such page</h1></main> })
        }
        StatusCode::INTERNAL_SERVER_ERROR => Some(html! { <h1>That went wrong on our side</h1> }),
        _ => None,
    }
}

async fn send(app: Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = app.oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    // An error page changes the length of the body: the header has to follow.
    assert_eq!(headers[header::CONTENT_LENGTH], body.len().to_string());
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

async fn get_page(app: Router, path: &str) -> (StatusCode, HeaderMap, String) {
    send(app, Request::get(path).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn the_development_page_shows_the_chain_of_an_unexpected_failure_and_where_it_failed() {
    let (status, headers, html) = get_page(app(ErrorPage::Development), "/orders/1").await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    // What failed: the type of the error, its message, and the message of its cause.
    assert!(html.contains("error::DatabaseDown"), "{html}");
    let message = html.find("could not load the order").expect(&html);
    let cause = html
        .find("connection refused by 10.0.0.5:5432")
        .expect(&html);
    assert!(message < cause, "{html}");
    // Where: the `?` in the handler.
    let location = format!("{}:{}:", file!(), SHOW_ORDER + 2);
    assert!(html.contains(&location), "no {location} in {html}");
}

/// The page that reveals nothing, for a 500.
const PLAIN_500: &str = "<!doctype html>\n\
     <html lang=\"en\">\n\
     <head>\n\
     <meta charset=\"utf-8\">\n\
     <title>500 Internal Server Error</title>\n\
     </head>\n\
     <body>\n\
     <h1>Internal Server Error</h1>\n\
     </body>\n\
     </html>\n";

#[tokio::test]
async fn the_plain_page_reveals_nothing_of_an_unexpected_failure_and_the_log_has_its_chain() {
    Logs::collect();

    // The page that reveals nothing is the one an application gets without asking.
    let (status, headers, html) = get_page(app(ErrorPage::default()), "/orders/1").await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    // No message, no type, no file and no line: the whole page, and no header but
    // those of any page.
    assert_eq!(html, PLAIN_500);
    assert_eq!(headers.len(), 2, "{headers:?}");

    let logged = Logs::of_this_test();
    let [logged] = logged.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    assert!(logged.starts_with("ERROR "), "{logged}");
    assert!(logged.contains("error::DatabaseDown"), "{logged}");
    assert!(logged.contains("could not load the order"), "{logged}");
    assert!(
        logged.contains("connection refused by 10.0.0.5:5432"),
        "{logged}"
    );
    let location = format!("{}:{}:", file!(), SHOW_ORDER + 2);
    assert!(logged.contains(&location), "no {location} in {logged}");
}

#[tokio::test]
async fn a_panic_in_a_handler_is_an_unexpected_failure_with_its_message_and_its_place() {
    Logs::collect();
    let location = format!("{}:{}:", file!(), DASHBOARD + 3);

    let (status, _, html) = get_page(app(ErrorPage::Development), "/dashboard").await;
    let (plain_status, headers, plain) = get_page(app(ErrorPage::Plain), "/dashboard").await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(html.contains("<h1>panic</h1>"), "{html}");
    assert!(html.contains("the cart of user 7 is gone"), "{html}");
    // Where: the `panic!` itself.
    assert!(html.contains(&location), "no {location} in {html}");

    assert_eq!(plain_status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(plain, PLAIN_500);
    assert_eq!(headers.len(), 2, "{headers:?}");

    let logged = Logs::of_this_test();
    assert_eq!(logged.len(), 2, "{logged:?}");
    for logged in &logged {
        assert!(logged.starts_with("ERROR "), "{logged}");
        assert!(logged.contains("the cart of user 7 is gone"), "{logged}");
        assert!(logged.contains(&location), "no {location} in {logged}");
    }
}

#[tokio::test]
async fn a_live_view_that_panics_in_the_mount_of_its_dead_render_gets_the_page() {
    struct Clock;
    impl LiveView for Clock {
        type Params = ();
        type Event = Infallible;
        type Message = Infallible;
        type Error = Infallible;

        async fn mount(_params: (), _socket: &mut Socket) -> Clock {
            panic!("this machine has no clock");
        }

        fn render(&self) -> Rendered {
            html! { <p>Tick</p> }
        }
    }
    let app = |page| {
        Router::new()
            .route("/clock", live::<Clock, _>())
            .layer(ErrorPages::new(page))
            .with_state(SigningKey::new(SECRET).unwrap())
    };

    // The Dead render is a request like any other.
    let (status, _, html) = get_page(app(ErrorPage::Development), "/clock").await;
    let (_, _, plain) = get_page(app(ErrorPage::Plain), "/clock").await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(html.contains("this machine has no clock"), "{html}");
    assert_eq!(plain, PLAIN_500);
}

/// Griffin's page for a status.
fn plain(title: &str, heading: &str) -> String {
    PLAIN_500
        .replace("500 Internal Server Error", title)
        .replace("Internal Server Error", heading)
}

#[tokio::test]
async fn not_found_and_method_not_allowed_have_pages_of_their_own() {
    // Whichever page is chosen for an unexpected failure.
    for page in [ErrorPage::Plain, ErrorPage::Development] {
        let (not_found, _, no_route) = get_page(app(page), "/no/such/page").await;
        let post = Request::post("/orders/1").body(Body::empty()).unwrap();
        let (not_allowed, headers, no_method) = send(app(page), post).await;

        assert_eq!(not_found, StatusCode::NOT_FOUND);
        assert_eq!(no_route, plain("404 Not Found", "Not Found"));
        assert_eq!(not_allowed, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            no_method,
            plain("405 Method Not Allowed", "Method Not Allowed")
        );
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        // What the response already said stays.
        assert_eq!(headers[header::ALLOW], "GET,HEAD");
    }
}

#[tokio::test]
async fn an_error_status_gets_its_page_only_when_the_response_has_no_body() {
    let (unparsed, _, no_user) = get_page(app(ErrorPage::Plain), "/users/seven").await;
    let (forbidden, _, refused) = get_page(app(ErrorPage::Plain), "/users/13").await;
    let (left, _, own_page) = get_page(app(ErrorPage::Plain), "/users/7").await;
    let (no_content, headers, nothing) = get_page(app(ErrorPage::Plain), "/users/1").await;

    // A path whose parameter does not parse, and a status a handler returned bare.
    assert_eq!(unparsed, StatusCode::NOT_FOUND);
    assert_eq!(no_user, plain("404 Not Found", "Not Found"));
    assert_eq!(forbidden, StatusCode::FORBIDDEN);
    assert_eq!(refused, plain("403 Forbidden", "Forbidden"));
    // The handler's own page, in the layout of its Scope.
    assert_eq!(left, StatusCode::NOT_FOUND);
    assert_eq!(
        own_page,
        "<main class=\"site\"><h1>User 7 has left</h1></main>"
    );
    // Not an error.
    assert_eq!(no_content, StatusCode::NO_CONTENT);
    assert_eq!(nothing, "");
    assert!(!headers.contains_key(header::CONTENT_TYPE), "{headers:?}");
}

#[tokio::test]
async fn the_application_overrides_the_pages_it_has_pages_of_its_own_for() {
    let app = |page| routes().layer(ErrorPages::new(page).pages(error_page));

    let (not_found, _, no_route) = get_page(app(ErrorPage::Plain), "/no/such/page").await;
    let (_, _, forbidden) = get_page(app(ErrorPage::Plain), "/users/13").await;
    let (failed, _, failure) = get_page(app(ErrorPage::Plain), "/orders/1").await;
    let (_, _, development) = get_page(app(ErrorPage::Development), "/orders/1").await;

    assert_eq!(not_found, StatusCode::NOT_FOUND);
    assert_eq!(
        no_route,
        "<main class=\"shop\"><h1>We have no such page</h1></main>"
    );
    // A status the application has no page for keeps Griffin's.
    assert_eq!(forbidden, plain("403 Forbidden", "Forbidden"));
    // Its page for an unexpected failure is given the status and nothing of the error.
    assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failure, "<h1>That went wrong on our side</h1>");
    // The development page is not the application's to replace.
    assert!(
        development.contains("could not load the order"),
        "{development}"
    );
}

/// What the user asked for has no index. The message quotes it.
#[derive(Debug)]
struct NoIndex<T>(T);

impl<T: fmt::Display> fmt::Display for NoIndex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no index for {}", self.0)
    }
}

impl<T: fmt::Debug + fmt::Display> std::error::Error for NoIndex<T> {}

async fn search(Path(query): Path<String>) -> Unexpected {
    Unexpected::new(NoIndex(query))
}

async fn search_panics(Path(query): Path<String>) {
    panic!("no index for {query}");
}

#[tokio::test]
async fn the_development_page_escapes_everything_it_shows() {
    let app = Router::new()
        .route("/search/{query}", get(search))
        .route("/panic/{query}", get(search_panics))
        .layer(ErrorPages::new(ErrorPage::Development));
    // `<script>alert('q')</script><b>&`, as a browser sends it in a path.
    let markup = "%3Cscript%3Ealert('q')%3C%2Fscript%3E%3Cb%3E&";

    for path in ["/search", "/panic"] {
        let (_, _, html) = get_page(app.clone(), &format!("{path}/{markup}")).await;

        // The user's markup, in the message, as text.
        let escaped = "no index for &lt;script&gt;alert(&#39;q&#39;)&lt;/script&gt;&lt;b&gt;&amp;";
        assert!(html.contains(escaped), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
    }
    // The name of a type has angle brackets of its own.
    let (_, _, html) = get_page(app, "/search/q").await;
    let kind = "error::NoIndex&lt;alloc::string::String&gt;";
    assert!(html.contains(kind), "{html}");
    assert!(!html.contains("NoIndex<"), "{html}");
}

const SECRET: &str = "a test secret, at least thirty-two bytes long";

/// The settings of an application. Which error page it shows is one of them.
#[derive(Debug, Deserialize)]
struct Config {
    secret_key_base: Secret,
    #[serde(default)]
    error_page: ErrorPage,
}

/// The configuration, with `variables` as the environment and no file.
fn configuration(variables: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let no_file = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("there is no such file.toml");
    let pair = |(name, value): &(&str, &str)| (name.to_string(), value.to_string());
    config::load_from(no_file, "SHOP", variables.iter().map(pair))
}

#[test]
fn which_page_is_shown_is_a_setting_of_the_configuration() {
    let secret = ("SHOP_SECRET_KEY_BASE", SECRET);
    let file = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("error_page.toml");
    std::fs::write(&file, "error_page = \"development\"").unwrap();
    let variables = [(secret.0.to_owned(), secret.1.to_owned())];

    let unset = configuration(&[secret]).unwrap();
    let by_environment = configuration(&[secret, ("SHOP_ERROR_PAGE", "development")]).unwrap();
    let by_file: Config = config::load_from(&file, "SHOP", variables).unwrap();
    let unknown = configuration(&[secret, ("SHOP_ERROR_PAGE", "verbose")]).unwrap_err();

    // The page that reveals nothing, unless the configuration asks for the other.
    assert_eq!(unset.error_page, ErrorPage::Plain);
    assert_eq!(by_environment.error_page, ErrorPage::Development);
    assert_eq!(by_file.error_page, ErrorPage::Development);
    // Another value is refused when the application starts, by the name of the setting.
    let refusal = unknown.to_string();
    assert!(refusal.contains("for key `error_page`"), "{refusal}");
}

/// An error that carries what the handler had at hand, as errors do.
#[derive(Debug)]
struct CheckoutFailed {
    config: Arc<Config>,
    session: Session,
}

impl fmt::Display for CheckoutFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { config, session } = self;
        write!(f, "checkout failed with {config:?} and {session:?}")
    }
}

impl std::error::Error for CheckoutFailed {}

async fn sign_in(session: Session) -> StatusCode {
    session.insert("card", "4111-1111-1111-1111");
    StatusCode::NO_CONTENT
}

async fn checkout(State(config): State<Arc<Config>>, session: Session) -> Unexpected {
    assert_eq!(
        session.get::<String>("card").unwrap(),
        "4111-1111-1111-1111"
    );
    Unexpected::new(CheckoutFailed { config, session })
}

/// An error whose message the application wrote a secret into itself.
async fn leak(State(config): State<Arc<Config>>) -> Unexpected {
    let message = format!("could not sign with {}", config.secret_key_base.expose());
    Unexpected::new(io::Error::other(message))
}

#[tokio::test]
async fn the_development_page_shows_no_secret_and_nothing_of_the_session() {
    Logs::collect();
    let page = ("SHOP_ERROR_PAGE", "development");
    let config = Arc::new(configuration(&[("SHOP_SECRET_KEY_BASE", SECRET), page]).unwrap());
    let session = SessionLayer::new(config.secret_key_base.expose()).unwrap();
    let shop = Scope::new("/")
        .pipe_through(Pipeline::new().layer(session))
        .route("/sign-in", post(sign_in))
        .route("/checkout", get(checkout))
        .route("/leak", get(leak));
    let app = Router::new()
        .merge(shop)
        .layer(ErrorPages::new(config.error_page))
        .with_state(config);

    let sign_in = Request::post("/sign-in").body(Body::empty()).unwrap();
    let (_, headers, _) = send(app.clone(), sign_in).await;
    let cookie = headers[header::SET_COOKIE].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap();
    let checkout = Request::get("/checkout")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let (status, _, html) = send(app.clone(), checkout).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // The error is shown as `Display` and as `Debug` print it, and Griffin's types
    // print neither a secret nor what a session holds.
    assert!(html.contains("checkout failed with Config"), "{html}");
    assert!(html.contains("CheckoutFailed {"), "{html}");
    assert!(html.contains("Secret(REDACTED)"), "{html}");
    assert!(html.contains("Session(REDACTED)"), "{html}");
    let (_, sealed) = cookie.split_once('=').unwrap();
    let logged = Logs::of_this_test().join("\n");
    for hidden in [SECRET, "thirty-two", "4111", "card", sealed] {
        assert!(!html.contains(hidden), "{hidden} in {html}");
        assert!(!logged.contains(hidden), "{hidden} in {logged}");
    }

    // The limit: redaction is by type. A secret the application took out of its
    // `Secret` and wrote into a message is text like any other, and is shown.
    let (_, _, html) = get_page(app, "/leak").await;
    assert!(html.contains(SECRET), "{html}");
}

/// Everything the tests of this file log, each event as its level and fields, with
/// the thread it was logged on. A test's app runs on the test's own thread.
struct Logs;

static LOGGED: Mutex<Vec<(ThreadId, String)>> = Mutex::new(Vec::new());

impl Logs {
    /// Starts collecting, for a test that then reads what its thread logged. The
    /// subscriber is the process's one: a thread's own would not see an event whose
    /// call site another test's thread reached first, without one.
    fn collect() {
        static COLLECTING: Once = Once::new();
        COLLECTING.call_once(|| tracing::subscriber::set_global_default(Logs).unwrap());
    }

    /// What this thread has logged.
    fn of_this_test() -> Vec<String> {
        let (logged, thread) = (LOGGED.lock().unwrap(), thread::current().id());
        let of_thread = logged.iter().filter(|(logger, _)| *logger == thread);
        of_thread.map(|(_, fields)| fields.clone()).collect()
    }
}

impl tracing::Subscriber for Logs {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
                write!(self.0, " {field}={value:?}").unwrap();
            }
        }
        let mut fields = Fields(event.metadata().level().to_string());
        event.record(&mut fields);
        let logged = (thread::current().id(), fields.0);
        LOGGED.lock().unwrap().push(logged);
    }

    // Spans are not what is looked at.
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}
