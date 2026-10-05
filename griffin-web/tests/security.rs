//! The security defaults, through small apps driven at the network boundary: HTTP
//! requests in and responses out, socket handshakes and frames. Each refusal has its
//! negative test here.

use futures_util::{SinkExt as _, StreamExt as _};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{Request, StatusCode, header};
use griffin_web::axum::response::Response;
use griffin_web::axum::routing::{get, post, put};
use griffin_web::axum::{self, Form, Router};
use griffin_web::html;
use griffin_web::live::{LiveView, Socket, SocketChecks, live, live_socket, live_socket_with};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::security::{CsrfLayer, SecureHeadersLayer};
use griffin_web::session::{Session, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::{self, Write as _};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::{self, ThreadId};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt as _;

/// A value no log line, `Debug` output or response may show.
const SECRET: &str = "hunter2-hunter2-hunter2-hunter2-hunter2";

const FORM: &str = "application/x-www-form-urlencoded";

/// How many times the state-changing handler of an app ran.
type Transfers = Arc<AtomicUsize>;

/// An app with a page that shows a form and a route the form posts to, behind the
/// default browser Pipeline.
fn bank(transfers: &Transfers) -> Router {
    let transfers = transfers.clone();
    let transfer = move |Form(form): Form<HashMap<String, String>>| async move {
        transfers.fetch_add(1, Ordering::SeqCst);
        format!("transferred {}", form["amount"])
    };
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(SessionLayer::new(SECRET).unwrap()))
        // What a form's hidden field would carry.
        .route(
            "/form",
            get(|session: Session| async move { session.csrf_token() }),
        )
        .route(
            "/transfer",
            post(transfer.clone()).get(|| async { "balance" }),
        )
        // The same handler for the other state-changing methods.
        .route(
            "/transfer/{id}",
            put(transfer.clone())
                .patch(transfer.clone())
                .delete(transfer),
        );
    Router::new().merge(site)
}

async fn body(response: Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// What a browser holds after it loaded the page with the form: the session cookie
/// and the token in the form.
struct Visit {
    cookie: String,
    token: String,
}

async fn visit(app: Router) -> Visit {
    let request = Request::get("/form").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = set_cookie.split_once(';').unwrap().0.to_owned();
    let token = body(response).await;
    Visit { cookie, token }
}

/// The form post a browser sends: the cookie it holds, if any, and the form's fields.
fn form_post(cookie: Option<&str>, fields: &str) -> Request<Body> {
    let mut request = Request::post("/transfer").header(header::CONTENT_TYPE, FORM);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::from(fields.to_owned())).unwrap()
}

#[tokio::test]
async fn a_form_post_without_a_token_is_refused() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;

    let request = form_post(Some(&visit.cookie), "amount=5");
    let response = bank(&transfers).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

#[tokio::test]
async fn a_form_post_with_the_token_of_its_session_succeeds() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;

    let fields = format!("amount=5&_csrf_token={}", visit.token);
    let request = form_post(Some(&visit.cookie), &fields);
    let response = bank(&transfers).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // The handler still reads the form the Layer looked into.
    assert_eq!(body(response).await, "transferred 5");
    assert_eq!(transfers.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_form_post_with_the_token_of_another_session_is_refused() {
    let transfers = Transfers::default();
    let victim = visit(bank(&transfers)).await;
    let attacker = visit(bank(&transfers)).await;

    let fields = format!("amount=5&_csrf_token={}", attacker.token);
    let request = form_post(Some(&victim.cookie), &fields);
    let response = bank(&transfers).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

#[tokio::test]
async fn a_form_post_with_a_token_and_no_session_is_refused() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;
    let fields = format!("amount=5&_csrf_token={}", visit.token);

    // No cookie at all, and a cookie that is not a session.
    for cookie in [None, Some("griffin_session=forged")] {
        let request = form_post(cookie, &fields);
        let response = bank(&transfers).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{cookie:?}");
    }
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

#[tokio::test]
async fn a_token_that_was_changed_or_is_not_one_is_refused() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;
    let (first, rest) = visit.token.split_at(1);
    let changed = format!("{}{rest}", if first == "A" { "B" } else { "A" });

    for token in [
        changed.as_str(),
        &visit.token[..visit.token.len() - 2],
        &visit.token[..visit.token.len() / 2],
        "",
        "not base64!",
        "AAAA",
    ] {
        let fields = format!("amount=5&_csrf_token={token}");
        let request = form_post(Some(&visit.cookie), &fields);
        let response = bank(&transfers).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{token}");
    }
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

#[tokio::test]
async fn every_method_that_is_not_safe_needs_the_token() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;

    for method in ["PUT", "PATCH", "DELETE"] {
        let request = |token: Option<&str>| {
            let mut request = Request::builder()
                .method(method)
                .uri("/transfer/7")
                .header(header::CONTENT_TYPE, FORM)
                .header(header::COOKIE, &visit.cookie);
            if let Some(token) = token {
                // As a script sends it: in a header, not in the body.
                request = request.header("x-csrf-token", token);
            }
            request.body(Body::from("amount=5")).unwrap()
        };
        let before = transfers.load(Ordering::SeqCst);

        let refused = bank(&transfers).oneshot(request(None)).await.unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method}");
        assert_eq!(transfers.load(Ordering::SeqCst), before, "{method} ran");

        let accepted = bank(&transfers).oneshot(request(Some(&visit.token)));
        assert_eq!(accepted.await.unwrap().status(), StatusCode::OK, "{method}");
    }
}

#[tokio::test]
async fn a_safe_method_needs_no_token() {
    let transfers = Transfers::default();
    let request = Request::get("/transfer").body(Body::empty()).unwrap();

    let response = bank(&transfers).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, "balance");
}

#[tokio::test]
async fn the_token_is_masked_anew_each_time_and_each_one_is_good() {
    let transfers = Transfers::default();
    let first = visit(bank(&transfers)).await;
    // The same session, asked for its token again.
    let request = Request::get("/form").header(header::COOKIE, &first.cookie);
    let response = bank(&transfers).oneshot(request.body(Body::empty()).unwrap());
    let response = response.await.unwrap();
    // The session already had its token: the cookie is not sent again.
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    let second = body(response).await;

    assert_ne!(first.token, second);
    for token in [&first.token, &second] {
        let fields = format!("amount=5&_csrf_token={token}");
        let request = form_post(Some(&first.cookie), &fields);
        let response = bank(&transfers).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn a_form_too_large_to_look_into_is_refused() {
    let transfers = Transfers::default();
    let visit = visit(bank(&transfers)).await;

    // The token is there, after more than the Layer reads.
    let fields = format!(
        "filler={}&_csrf_token={}",
        "x".repeat(3_000_000),
        visit.token
    );
    let request = form_post(Some(&visit.cookie), &fields);
    let response = bank(&transfers).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

#[tokio::test]
async fn without_a_session_layer_a_state_changing_request_is_refused() {
    let transfers = Transfers::default();
    let counted = transfers.clone();
    let transfer = move || async move {
        counted.fetch_add(1, Ordering::SeqCst);
    };
    // The CSRF Layer with no `SessionLayer` before it: there is nothing to check
    // a token against, so nothing passes.
    let site = Scope::new("/")
        .pipe_through(Pipeline::new().layer(CsrfLayer::new()))
        .route("/transfer", post(transfer));

    let request = form_post(None, "amount=5&_csrf_token=AAAA");
    let response = Router::new().merge(site).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(transfers.load(Ordering::SeqCst), 0, "the handler ran");
}

/// A LiveView behind the session, which tells when a connected one is mounted.
struct Vault;

/// How many times `Vault` was mounted connected, for the test named in its route.
static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn opened(test: &str) -> usize {
    let opened = OPENED.lock().unwrap();
    opened.iter().filter(|name| *name == test).count()
}

impl LiveView for Vault {
    type Params = String;
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(test: String, socket: &mut Socket) -> Vault {
        if socket.connected() {
            OPENED.lock().unwrap().push(test);
        }
        Vault
    }

    fn render(&self) -> Rendered {
        html! { <p>Gold</p> }
    }
}

/// An app whose LiveView is behind the default browser Pipeline, with a socket that
/// checks what `checks` says, or what `live_socket` checks by default.
fn live_app(checks: Option<SocketChecks>) -> Router {
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(SessionLayer::new(SECRET).unwrap()))
        .route("/vault/{test}", live::<Vault, _>());
    let pages = Router::new().merge(site);
    let socket = match checks {
        Some(checks) => live_socket_with(pages.clone(), checks),
        None => live_socket(pages.clone()),
    };
    pages
        .route("/live/websocket", socket)
        .with_state(SigningKey::new(SECRET).unwrap())
}

/// Serves the app on a local port, as a browser would reach it.
async fn serve(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

/// What a browser holds after it loaded a LiveView's page: the session cookie, and
/// what the page's script reads from the container.
struct LivePage {
    cookie: String,
    csrf_token: String,
    join: Value,
}

/// Loads the Dead render of `Vault` for `test`, as a browser with no cookie.
async fn load(test: &str) -> LivePage {
    let path = format!("/vault/{test}");
    let request = Request::get(&path).body(Body::empty()).unwrap();
    let response = live_app(None).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = set_cookie.split_once(';').unwrap().0.to_owned();
    let html = body(response).await;
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    };
    let topic = format!("lv:{}", attribute("id"));
    LivePage {
        cookie,
        csrf_token: attribute("data-csrf-token"),
        join: json!(["1", "1", topic, "phx_join", {
            "url": format!("http://localhost{path}"),
            "params": {"_mounts": 0, "_mount_attempts": 0},
            "session": attribute("data-phx-session"),
            "static": attribute("data-phx-static"),
            "sticky": false,
        }]),
    }
}

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The handshake of a socket: what the client puts in the URL, and what the browser
/// adds by itself.
#[derive(Default)]
struct Handshake<'a> {
    csrf_token: Option<&'a str>,
    cookie: Option<&'a str>,
    /// `{host}` stands for the host and port the handshake is sent to.
    origin: Option<&'a str>,
}

impl Handshake<'_> {
    /// Opens the socket of `app`, or gives the status it was refused with.
    async fn open(&self, app: Router) -> Result<Stream, StatusCode> {
        let address = serve(app).await;
        let mut url = format!("ws://{address}/live/websocket?vsn=2.0.0");
        if let Some(token) = self.csrf_token {
            write!(url, "&_csrf_token={token}").unwrap();
        }
        let mut handshake = url.into_client_request().unwrap();
        if let Some(cookie) = self.cookie {
            let cookie = cookie.parse().unwrap();
            handshake.headers_mut().insert(header::COOKIE, cookie);
        }
        if let Some(origin) = self.origin {
            let origin = origin.replace("{host}", &address.to_string());
            let origin = origin.parse().unwrap();
            handshake.headers_mut().insert(header::ORIGIN, origin);
        }
        match connect_async(handshake).await {
            Ok((stream, _)) => Ok(stream),
            Err(tungstenite::Error::Http(response)) => Err(response.status()),
            Err(error) => panic!("the handshake failed: {error}"),
        }
    }
}

/// The `(status, response)` of the reply to `page`'s join on `stream`.
async fn join(stream: &mut Stream, page: &LivePage) -> (String, Value) {
    let frame = Message::Text(page.join.to_string().into());
    stream.send(frame).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), stream.next());
    let reply = reply.await.expect("no frame from the server").unwrap();
    let reply: Value = serde_json::from_str(reply.unwrap().to_text().unwrap()).unwrap();
    assert_eq!(reply[3], "phx_reply", "{reply}");
    let status = reply[4]["status"].as_str().unwrap().to_owned();
    (status, reply[4]["response"].clone())
}

/// The reply that makes the client give up on the join and load the page again.
fn refused() -> (String, Value) {
    ("error".to_owned(), json!({"reason": "unauthorized"}))
}

#[tokio::test]
async fn a_socket_opened_with_the_token_and_the_cookie_of_a_page_joins() {
    let page = load("joins").await;
    let handshake = Handshake {
        csrf_token: Some(&page.csrf_token),
        cookie: Some(&page.cookie),
        origin: Some("http://{host}"),
    };

    let mut stream = handshake.open(live_app(None)).await.unwrap();
    let (status, _) = join(&mut stream, &page).await;

    assert_eq!(status, "ok");
    assert_eq!(opened("joins"), 1);
}

#[tokio::test]
async fn a_socket_opened_without_a_csrf_token_is_refused() {
    let page = load("no-token").await;
    let handshake = Handshake {
        csrf_token: None,
        cookie: Some(&page.cookie),
        origin: Some("http://{host}"),
    };

    let refusal = handshake.open(live_app(None)).await.unwrap_err();

    assert_eq!(refusal, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_socket_opened_with_a_wrong_csrf_token_joins_nothing() {
    let page = load("wrong-token").await;
    let other = load("wrong-token").await;
    let (first, rest) = page.csrf_token.split_at(1);
    let changed = format!("{}{rest}", if first == "A" { "B" } else { "A" });

    for (token, cookie) in [
        // Another session's token,
        (other.csrf_token.as_str(), Some(page.cookie.as_str())),
        // the right token with another session, with no session,
        (&page.csrf_token, Some(other.cookie.as_str())),
        (&page.csrf_token, None),
        // and what is no token of this session at all.
        (&changed, Some(page.cookie.as_str())),
        ("", Some(page.cookie.as_str())),
        ("null", Some(page.cookie.as_str())),
    ] {
        let handshake = Handshake {
            csrf_token: Some(token),
            cookie,
            origin: Some("http://{host}"),
        };
        // The socket opens, so that the client is told to load the page again,
        let mut stream = handshake.open(live_app(None)).await.unwrap();
        // which is what this answer to a join does.
        assert_eq!(join(&mut stream, &page).await, refused(), "{token}");
    }
    assert_eq!(opened("wrong-token"), 0, "a LiveView was mounted");
}

#[tokio::test]
async fn a_socket_opened_from_a_foreign_origin_is_refused() {
    let page = load("foreign-origin").await;

    for origin in [
        "https://evil.example",
        // Another port of this host is another origin,
        "http://127.0.0.1:1",
        "http://127.0.0.1",
        // and so is a name that only starts or ends like this host.
        "http://{host}.evil.example",
        "http://evil.example.{host}",
        "http://user@{host}",
        // A sandboxed page, and a page that is no web page.
        "null",
        "file://",
        "chrome-extension://{host}",
        "",
    ] {
        // Everything else about the handshake is right.
        let handshake = Handshake {
            csrf_token: Some(&page.csrf_token),
            cookie: Some(&page.cookie),
            origin: Some(origin),
        };
        let refusal = handshake.open(live_app(None)).await;
        assert_eq!(refusal.unwrap_err(), StatusCode::FORBIDDEN, "{origin}");
    }
}

#[tokio::test]
async fn only_the_origins_listed_are_allowed_once_there_is_a_list() {
    let page = load("listed-origins").await;
    let checks = || SocketChecks::new().allowed_origins(["https://shop.example"]);
    let handshake = |origin| Handshake {
        csrf_token: Some(&page.csrf_token),
        cookie: Some(&page.cookie),
        origin: Some(origin),
    };

    let listed = handshake("https://shop.example");
    let mut stream = listed.open(live_app(Some(checks()))).await.unwrap();
    assert_eq!(join(&mut stream, &page).await.0, "ok");

    for origin in [
        // The host itself is not in the list,
        "http://{host}",
        // and the list is of whole origins: the scheme and the port count.
        "http://shop.example",
        "https://shop.example:8443",
        "https://shop.example.evil.example",
    ] {
        let refusal = handshake(origin).open(live_app(Some(checks()))).await;
        assert_eq!(refusal.unwrap_err(), StatusCode::FORBIDDEN, "{origin}");
    }

    // An empty list allows no page at all.
    let none = SocketChecks::new().allowed_origins(Vec::<String>::new());
    let refusal = handshake("http://{host}").open(live_app(Some(none))).await;
    assert_eq!(refusal.unwrap_err(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_handshake_without_an_origin_is_not_a_browsers_and_still_needs_the_token() {
    let page = load("no-origin").await;

    // No browser leaves `Origin` out, so this is a program with a session of its own.
    let handshake = Handshake {
        csrf_token: Some(&page.csrf_token),
        cookie: Some(&page.cookie),
        origin: None,
    };
    let mut stream = handshake.open(live_app(None)).await.unwrap();
    assert_eq!(join(&mut stream, &page).await.0, "ok");

    let without_token = Handshake {
        cookie: Some(&page.cookie),
        ..Handshake::default()
    };
    let refusal = without_token.open(live_app(None)).await.unwrap_err();
    assert_eq!(refusal, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_csrf_check_of_the_socket_is_turned_off_only_by_an_explicit_call() {
    let page = load("unchecked").await;
    let unchecked = SocketChecks::new().without_csrf_check();

    let handshake = Handshake {
        origin: Some("http://{host}"),
        ..Handshake::default()
    };
    let mut stream = handshake.open(live_app(Some(unchecked))).await.unwrap();
    assert_eq!(join(&mut stream, &page).await.0, "ok");

    // The origin is still checked.
    let foreign = Handshake {
        origin: Some("https://evil.example"),
        ..Handshake::default()
    };
    let unchecked = SocketChecks::new().without_csrf_check();
    let refusal = foreign.open(live_app(Some(unchecked))).await.unwrap_err();
    assert_eq!(refusal, StatusCode::FORBIDDEN);
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

/// Fails if `shown` has any of the `secrets` in it.
fn assert_shows_none_of(shown: &str, secrets: &[&str]) {
    for secret in secrets {
        assert!(!secret.is_empty());
        assert!(!shown.contains(secret), "`{secret}` is in: {shown}");
    }
}

#[tokio::test]
async fn a_refusal_is_logged_and_answered_without_a_secret_or_a_token() {
    Logs::collect();
    let transfers = Transfers::default();
    let victim = visit(bank(&transfers)).await;
    let attacker = visit(bank(&transfers)).await;
    let page = load("redaction").await;
    let other = load("redaction").await;
    let secrets = [
        SECRET,
        &victim.token,
        &attacker.token,
        // The sealed session: whoever has it is the visitor.
        victim.cookie.split_once('=').unwrap().1,
        &page.csrf_token,
        &other.csrf_token,
        page.cookie.split_once('=').unwrap().1,
    ];
    let mut answers = Vec::new();

    // A forged form post, with a token that is good for another session,
    let fields = format!("amount=5&_csrf_token={}", attacker.token);
    let request = form_post(Some(&victim.cookie), &fields);
    let response = bank(&transfers).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    answers.push(format!(
        "{:?} {}",
        response.headers().clone(),
        body(response).await
    ));
    // a socket from another site, one without a token,
    for (csrf_token, origin) in [
        (Some(page.csrf_token.as_str()), "https://evil.example"),
        (None, "http://{host}"),
    ] {
        let handshake = Handshake {
            csrf_token,
            cookie: Some(&page.cookie),
            origin: Some(origin),
        };
        let refusal = handshake.open(live_app(None)).await.unwrap_err();
        assert_eq!(refusal, StatusCode::FORBIDDEN);
    }
    // and a join on a socket opened with another session's token.
    let handshake = Handshake {
        csrf_token: Some(&other.csrf_token),
        cookie: Some(&page.cookie),
        origin: Some("http://{host}"),
    };
    let mut stream = handshake.open(live_app(None)).await.unwrap();
    let reply = join(&mut stream, &page).await;
    assert_eq!(reply, refused());
    answers.push(reply.1.to_string());

    // Each refusal was logged, as a warning that says what was refused and why,
    let logged = Logs::of_this_test();
    let refusals: Vec<_> = logged
        .iter()
        .filter(|line| line.contains("refused"))
        .collect();
    assert_eq!(refusals.len(), 4, "{logged:#?}");
    assert!(
        refusals.iter().all(|line| line.starts_with("WARN ")),
        "{logged:#?}"
    );
    assert!(
        refusals[0].contains("method=POST path=\"/transfer\""),
        "{logged:#?}"
    );
    assert!(
        refusals[1].contains("origin=\"https://evil.example\""),
        "{logged:#?}"
    );
    // and nothing logged or answered gives a secret or a token away.
    for shown in logged.iter().chain(&answers) {
        assert_shows_none_of(shown, &secrets);
    }
}

#[tokio::test]
async fn the_origin_of_a_refused_socket_is_logged_escaped_and_cut_short() {
    Logs::collect();
    // An origin made to look like a log line of its own, and far too long.
    let origin = format!("https://evil.example\tERROR forged {}", "x".repeat(5000));
    let handshake = Handshake {
        origin: Some(&origin),
        ..Handshake::default()
    };

    let refusal = handshake.open(live_app(None)).await.unwrap_err();

    assert_eq!(refusal, StatusCode::FORBIDDEN);
    let mut logged = Logs::of_this_test();
    logged.retain(|line| line.contains("refused"));
    let [logged] = logged.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    let quoted = r#"origin="https://evil.example\tERROR forged xxx"#;
    assert!(logged.contains(quoted), "{logged}");
    assert!(logged.len() < 400, "{} bytes logged", logged.len());
}

#[test]
fn debug_output_shows_no_secret_and_no_token() {
    let session = Session::default();
    session.insert("card", SECRET);
    let token = session.csrf_token();
    // What the session keeps of its CSRF token, which the browser never sees.
    let kept: String = session.get("_csrf_token").unwrap();
    assert_eq!(kept.len(), 43);

    let shown = [
        format!("{session:?}"),
        format!("{:?}", SessionLayer::new(SECRET).unwrap()),
        format!("{:?}", SigningKey::new(SECRET).unwrap()),
        format!("{:?}", griffin_web::config::Secret::new(SECRET)),
        format!("{:?}", CsrfLayer::new()),
        format!("{:?}", SecureHeadersLayer::new()),
        format!("{:?}", SocketChecks::new()),
    ];

    for shown in &shown {
        assert_shows_none_of(shown, &[SECRET, &token, &kept]);
    }
}

/// The headers `Pipeline::browser` puts on every response, with their values.
const DEFAULT_HEADERS: [(&str, &str); 4] = [
    (
        "content-security-policy",
        "base-uri 'self'; frame-ancestors 'self'",
    ),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("x-content-type-options", "nosniff"),
    ("x-permitted-cross-domain-policies", "none"),
];

async fn page() -> Rendered {
    html! { <h1>Welcome</h1> }
}

#[tokio::test]
async fn the_default_headers_are_on_an_html_response() {
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(SessionLayer::new(SECRET).unwrap()))
        .route("/", get(page));

    let request = Request::get("/").body(Body::empty()).unwrap();
    let response = Router::new().merge(site).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers()[header::CONTENT_TYPE].to_str().unwrap();
    assert!(content_type.starts_with("text/html"), "{content_type}");
    for (name, value) in DEFAULT_HEADERS {
        let values: Vec<_> = response.headers().get_all(name).into_iter().collect();
        assert_eq!(values, [value], "{name}");
    }
}

#[tokio::test]
async fn a_refusal_carries_the_default_headers_too() {
    let request = form_post(None, "amount=5");
    let response = bank(&Transfers::default()).oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    for (name, value) in DEFAULT_HEADERS {
        assert_eq!(response.headers().get(name).unwrap(), value, "{name}");
    }
}

#[tokio::test]
async fn a_default_header_is_left_out_only_by_an_explicit_call() {
    let headers = SecureHeadersLayer::new().without(header::REFERRER_POLICY);
    let site = Scope::new("/")
        .pipe_through(Pipeline::new().layer(headers))
        .route("/", get(page));

    let request = Request::get("/").body(Body::empty()).unwrap();
    let response = Router::new().merge(site).oneshot(request).await.unwrap();

    for (name, value) in DEFAULT_HEADERS {
        let expected = (name != "referrer-policy").then_some(value);
        let sent = response.headers().get(name);
        let sent = sent.map(|sent| sent.to_str().unwrap());
        assert_eq!(sent, expected, "{name}");
    }
}

#[tokio::test]
async fn a_header_the_handler_set_itself_is_kept() {
    let strict = "default-src 'self'";
    let strict_page =
        move || async move { ([(header::CONTENT_SECURITY_POLICY, strict)], page().await) };
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(SessionLayer::new(SECRET).unwrap()))
        .route("/", get(strict_page));

    let request = Request::get("/").body(Body::empty()).unwrap();
    let response = Router::new().merge(site).oneshot(request).await.unwrap();

    let policies = response.headers().get_all(header::CONTENT_SECURITY_POLICY);
    assert_eq!(policies.into_iter().collect::<Vec<_>>(), [strict]);
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
}
