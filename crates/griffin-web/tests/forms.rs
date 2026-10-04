//! Forms through the network boundary: the form Events of a LiveView as the Phoenix
//! client sends them (a URL-encoded string in `value`), and a controller form post
//! behind the browser Pipeline. Both validate with the same `Changeset`.

use futures_util::{SinkExt as _, StreamExt as _};
use griffin_domain::{Action, Changeset, Errors};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{Request, StatusCode, header};
use griffin_web::axum::response::{IntoResponse as _, Redirect, Response};
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::live_socket_with;
use griffin_web::live::{Event, FormParams, LiveView, Socket, SocketChecks, live};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::security::csrf_field;
use griffin_web::session::{Session, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt as _;

const SECRET: &str = "a test secret, at least thirty-two bytes long";

#[derive(Clone, Debug, PartialEq)]
struct Email(String);

impl FromStr for Email {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        raw.contains('@')
            .then(|| Email(raw.to_owned()))
            .ok_or("must contain @")
    }
}

#[derive(griffin_domain::Changeset, Clone, Debug)]
struct Signup {
    email: Email,
    #[allow(dead_code)]
    tags: Vec<String>,
}

/// A Capability: what the Context needs of the outside world, as a trait.
trait Accounts {
    fn insert(&self, email: &str) -> bool;
}

/// An in-memory store that counts its calls, so a test sees when a Context ran.
struct Memory {
    calls: AtomicUsize,
    emails: Mutex<HashSet<String>>,
}

impl Memory {
    fn new() -> Memory {
        Memory {
            calls: AtomicUsize::new(0),
            emails: Mutex::new(HashSet::from(["taken@example.com".to_owned()])),
        }
    }
}

impl Accounts for Memory {
    fn insert(&self, email: &str) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.emails.lock().unwrap().insert(email.to_owned())
    }
}

/// The Context function: stage three. It reads the Command without consuming the
/// Changeset, so that it can report on it.
fn register(accounts: &impl Accounts, changeset: Changeset<Signup>) -> Changeset<Signup> {
    let Some(signup) = changeset.valid() else {
        return changeset;
    };
    if accounts.insert(&signup.email.0) {
        changeset
    } else {
        changeset.add_error("email", "has already been taken")
    }
}

fn rule(_: &Signup, _: &mut Errors) {}

#[derive(Event)]
enum SignupEvent {
    Change {
        #[form]
        params: FormParams,
    },
    Submit {
        #[form]
        params: FormParams,
    },
}

struct SignupPage {
    accounts: Memory,
    summary: String,
}

impl LiveView for SignupPage {
    type Params = ();
    type Event = SignupEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_: (), _socket: &mut Socket) -> SignupPage {
        SignupPage {
            accounts: Memory::new(),
            summary: String::new(),
        }
    }

    async fn handle_event(
        &mut self,
        event: SignupEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        let (params, submitting) = match event {
            SignupEvent::Change { params } => (params, false),
            SignupEvent::Submit { params } => (params, true),
        };
        let mut changeset = Changeset::<Signup>::cast_form("signup", params.pairs()).validate(rule);
        changeset = if submitting {
            register(&self.accounts, changeset.with_action(Action::Insert))
        } else {
            changeset.with_action(Action::Validate)
        };
        let form = changeset.to_form();
        self.summary = format!("{form:?}");
        Ok(())
    }

    fn render(&self) -> Rendered {
        let calls = self.accounts.calls.load(Ordering::SeqCst).to_string();
        html! {
            <pre data-calls={calls}>{@summary}</pre>
        }
    }
}

fn app() -> Router {
    let pages = Router::new().route("/signup", live::<SignupPage, _>());
    let checks = SocketChecks::new().without_csrf_check();
    pages
        .clone()
        .route("/live/websocket", live_socket_with(pages, checks))
        .with_state(SigningKey::new(SECRET).unwrap())
}

async fn serve() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app()).await.unwrap() });
    address
}

struct Client(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Client {
    async fn send(&mut self, frame: Value) {
        self.0
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        let next = tokio::time::timeout(Duration::from_secs(5), self.0.next());
        let text = next.await.expect("no frame").expect("closed").unwrap();
        serde_json::from_str(text.to_text().unwrap()).unwrap()
    }
}

/// A socket with the signup page joined, and its topic.
async fn joined() -> (Client, String) {
    let html = {
        let request = Request::get("/signup").body(Body::empty()).unwrap();
        let response = app().oneshot(request).await.unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    };
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    };
    let topic = format!("lv:{}", attribute("id"));
    let address = serve().await;
    let url = format!("ws://{address}/live/websocket?vsn=2.0.0");
    let mut client = Client(connect_async(url).await.unwrap().0);
    let join = json!(["4", "4", topic, "phx_join", {
        "url": "http://localhost/signup",
        "params": {"_mounts": 0, "_mount_attempts": 0},
        "session": attribute("data-phx-session"),
        "static": attribute("data-phx-static"),
        "sticky": false,
    }]);
    client.send(join).await;
    assert_eq!(client.receive().await[4]["status"], "ok");
    (client, topic)
}

/// What the client sends for a form event: the serialized form is one string.
fn form_event(event: &str, value: &str) -> Value {
    json!({"type": "form", "event": event, "value": value, "meta": {"_target": "signup[email]"}})
}

/// The text of the page after the event: slot 1 is the summary and slot 0 the calls.
async fn after(client: &mut Client, topic: &str, payload: Value) -> Value {
    client
        .send(json!(["4", "5", topic, "event", payload]))
        .await;
    let mut reply = client.receive().await;
    assert_eq!(reply[4]["status"], "ok", "{reply}");
    reply[4]["response"]["diff"].take()
}

fn encode(pairs: &[(&str, &str)]) -> String {
    let mut encoder = form_urlencoded::Serializer::new(String::new());
    encoder.extend_pairs(pairs);
    encoder.finish()
}

#[tokio::test]
async fn a_large_legitimate_form_is_read_and_pairs_past_the_limit_are_dropped_not_fatal() {
    let (mut client, topic) = joined().await;
    // 2400 inputs, each with the `_unused_` marker the client adds: 4800 pairs, more
    // than the old limit, and the email is among them.
    let mut pairs: Vec<(String, String)> = (0..2_400)
        .flat_map(|n| {
            [
                (format!("signup[_unused_f{n}]"), String::new()),
                (format!("signup[f{n}]"), "x".to_owned()),
            ]
        })
        .collect();
    pairs.push(("signup[email]".to_owned(), "big@example.com".to_owned()));
    let encode_all = |pairs: &[(String, String)]| {
        let pairs: Vec<_> = pairs
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        encode(&pairs)
    };
    let diff = after(
        &mut client,
        &topic,
        form_event("change", &encode_all(&pairs)),
    )
    .await;
    assert!(
        diff["1"].as_str().unwrap().contains("big@example.com"),
        "{diff}"
    );

    // Past 5000 pairs they are ignored, and the LiveView carries on.
    pairs = (0..5_000)
        .map(|_| ("filler".to_owned(), "x".to_owned()))
        .collect();
    pairs.push(("signup[email]".to_owned(), "late@example.com".to_owned()));
    let diff = after(
        &mut client,
        &topic,
        form_event("change", &encode_all(&pairs)),
    )
    .await;
    assert!(
        !diff["1"].as_str().unwrap().contains("late@example.com"),
        "{diff}"
    );
    // ...and the form shows why: it is invalid, and a submit never reaches the Context.
    assert!(diff["1"].as_str().unwrap().contains("too large"), "{diff}");
    let diff = after(
        &mut client,
        &topic,
        form_event("submit", &encode_all(&pairs)),
    )
    .await;
    assert!(diff["1"].as_str().unwrap().contains("too large"), "{diff}");
    assert!(diff.get("0").is_none(), "{diff}");
}

#[tokio::test]
async fn a_form_body_is_read_up_to_one_mebibyte_and_a_larger_one_ends_the_live_view() {
    let (mut client, topic) = joined().await;
    let prefix = "signup[email]=";
    let exactly = format!("{prefix}{}", "a".repeat((1 << 20) - prefix.len()));
    assert_eq!(exactly.len(), 1 << 20);
    after(&mut client, &topic, form_event("change", &exactly)).await;

    let over = format!("{exactly}a");
    let payload = form_event("change", &over);
    client
        .send(json!(["4", "6", topic, "event", payload]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["4", "4", topic, "phx_error", {"reason": "channel_crash"}])
    );
}

#[tokio::test]
async fn a_change_event_with_a_valid_form_never_runs_the_context() {
    let (mut client, topic) = joined().await;

    // Valid and free, then valid and taken: both pass stages one and two, so only
    // a Context run on change could insert, count a call, or find the email taken.
    for email in ["free@example.com", "taken@example.com"] {
        let value = encode(&[("signup[email]", email)]);
        let diff = after(&mut client, &topic, form_event("change", &value)).await;
        let summary = diff["1"].as_str().unwrap();
        assert!(summary.contains("errors: []"), "{summary}");
        assert!(!summary.contains("already"), "{summary}");
        // `calls` is slot 0 and is still "0": it was not sent again.
        assert!(diff.get("0").is_none(), "{diff}");
    }

    // The same valid, taken email on submit does reach the Context.
    let value = encode(&[("signup[email]", "taken@example.com")]);
    let diff = after(&mut client, &topic, form_event("submit", &value)).await;
    assert_eq!(diff["0"], " data-calls=\"1\"");
    assert!(diff["1"].as_str().unwrap().contains("already"), "{diff}");
}

#[tokio::test]
async fn a_change_event_carries_the_nested_and_list_params_and_runs_no_context() {
    let (mut client, topic) = joined().await;
    let value = encode(&[
        ("signup[_unused_email]", ""),
        ("signup[email]", "ada"),
        ("signup[tags][]", "math"),
        ("signup[tags][]", "engines"),
        ("_target", "signup[email]"),
    ]);

    let diff = after(&mut client, &topic, form_event("change", &value)).await;

    let summary = diff["1"].as_str().unwrap().replace("&quot;", "\"");
    assert!(
        summary.contains(r#"values: ["math", "engines"]"#),
        "{summary}"
    );
    // Not touched: the client says the user has not used it yet.
    assert!(summary.contains("touched: false"), "{summary}");
    assert!(summary.contains("must contain @"), "{summary}");
    assert!(summary.contains("action: Some(Validate)"), "{summary}");
    // The Context was not called: `calls` is slot 0, and has not changed from "0".
    assert!(diff.get("0").is_none(), "{diff}");
}

#[tokio::test]
async fn a_submit_runs_the_context_once_and_a_taken_email_is_an_error_on_its_field() {
    let (mut client, topic) = joined().await;

    let taken = encode(&[("signup[email]", "taken@example.com")]);
    let diff = after(&mut client, &topic, form_event("submit", &taken)).await;
    assert_eq!(diff["0"], " data-calls=\"1\"");
    assert!(
        diff["1"]
            .as_str()
            .unwrap()
            .contains("has already been taken"),
        "{diff}"
    );

    let free = encode(&[("signup[email]", "ada@example.com")]);
    let diff = after(&mut client, &topic, form_event("submit", &free)).await;
    assert_eq!(diff["0"], " data-calls=\"2\"");
    assert!(!diff["1"].as_str().unwrap().contains("already"), "{diff}");
}

#[tokio::test]
async fn a_submit_that_fails_a_field_does_not_reach_the_context() {
    let (mut client, topic) = joined().await;
    let value = encode(&[("signup[email]", "nope")]);

    let diff = after(&mut client, &topic, form_event("submit", &value)).await;

    assert!(diff.get("0").is_none(), "{diff}");
}

#[tokio::test]
async fn names_that_select_nothing_declared_are_ignored_and_the_live_view_carries_on() {
    let (mut client, topic) = joined().await;
    let deep = format!("signup{}", "[a]".repeat(5_000));
    let value = encode(&[
        ("signup[email]", "ada@example.com"),
        ("signup[admin]", "true"),
        ("admin", "true"),
        ("__proto__[polluted]", "1"),
        ("signup[", "x"),
        (&deep, "x"),
    ]);

    let diff = after(&mut client, &topic, form_event("change", &value)).await;

    let summary = diff["1"].as_str().unwrap();
    assert!(summary.contains("ada@example.com"), "{summary}");
    assert!(!summary.contains("admin"), "{summary}");
}

#[tokio::test]
async fn a_form_event_that_is_malformed_ends_the_live_view_as_any_undecodable_event_does() {
    let too_big = format!("signup[email]={}", "a".repeat(1 << 21));
    for (case, value) in [
        (
            "an object, not a string",
            json!({"signup": {"email": "a@b"}}),
        ),
        ("no value", Value::Null),
        ("a body larger than a form is", json!(too_big)),
    ] {
        let (mut client, topic) = joined().await;
        let payload = json!({"type": "form", "event": "change", "value": value});

        client
            .send(json!(["4", "5", topic, "event", payload]))
            .await;

        assert_eq!(
            client.receive().await,
            json!(["4", "4", topic, "phx_error", {"reason": "channel_crash"}]),
            "{case}"
        );
    }
}

#[test]
fn a_form_event_encodes_without_its_params() {
    let event = SignupEvent::Change {
        params: FormParams::default(),
    };
    assert_eq!(event.encode(), ("change", vec![]));
}

/// A controller form: the page writes the CSRF field, the post validates with the
/// same Changeset and answers with the errors or a redirect with a flash.
fn site() -> Router {
    async fn show(session: Session) -> Rendered {
        html! { <form method="post" action="/signup">{csrf_field(&session)}</form> }
    }
    async fn create(session: Session, params: FormParams) -> Response {
        let changeset =
            Changeset::<Signup>::cast_form("signup", params.pairs()).with_action(Action::Insert);
        match changeset.apply() {
            Ok(signup) => {
                session.put_flash("info", format!("welcome {}", signup.email.0));
                Redirect::to("/signup").into_response()
            }
            Err(changeset) => {
                let form = format!("{:?}", changeset.to_form());
                (StatusCode::UNPROCESSABLE_ENTITY, form).into_response()
            }
        }
    }
    async fn count(params: FormParams) -> String {
        params.pairs().count().to_string()
    }
    let session = SessionLayer::new(SECRET).unwrap();
    Router::new().merge(
        Scope::new("/")
            .pipe_through(Pipeline::browser(session))
            .layout(|page, flash| {
                html! { <main><p id="flash">{flash.get("info").unwrap_or_default()}</p>{page}</main> }
            })
            .route("/signup", get(show).post(create))
            .route("/count", axum::routing::post(count)),
    )
}

/// The session cookie and CSRF token a GET of the page gives.
async fn visit(app: &Router) -> (String, String) {
    let response = app
        .clone()
        .oneshot(Request::get("/signup").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains(r#"<input type="hidden" name="_csrf_token" value=""#),
        "{html}"
    );
    let start = html.find(r#"value=""#).unwrap() + 7;
    let token = html[start..start + html[start..].find('"').unwrap()].to_owned();
    (cookie, token)
}

async fn post_form_response(app: &Router, cookie: &str, body: String) -> Response {
    post_to(app, "/signup", cookie, body).await
}

async fn post_to(app: &Router, path: &str, cookie: &str, body: String) -> Response {
    let request = Request::post(path)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// The page the cookie gets, as a browser that follows the redirect would.
async fn page(app: &Router, cookie: &str) -> (String, String) {
    let request = Request::get("/signup")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let next = response.headers().get(header::SET_COOKIE);
    let next = next.map_or(cookie.to_owned(), |value| {
        value
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    });
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (String::from_utf8(body.to_vec()).unwrap(), next)
}

async fn post_form(app: &Router, cookie: &str, body: String) -> (StatusCode, String) {
    let response = post_form_response(app, cookie, body).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_controller_form_post_without_the_csrf_token_is_refused() {
    let app = site();
    let (cookie, _token) = visit(&app).await;

    let body = encode(&[("signup[email]", "ada@example.com")]);
    let (status, _) = post_form(&app, &cookie, body).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_controller_form_post_with_the_token_is_validated_by_the_same_changeset() {
    let app = site();
    let (cookie, token) = visit(&app).await;

    let invalid = encode(&[("_csrf_token", &token), ("signup[email]", "nope")]);
    let (status, body) = post_form(&app, &cookie, invalid).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("must contain @"), "{body}");

    let valid = encode(&[
        ("_csrf_token", &token),
        ("signup[email]", "ada@example.com"),
    ]);
    let response = post_form_response(&app, &cookie, valid).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signup");
    // The flash left by the post is on the page the redirect leads to, once.
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = cookie.split(';').next().unwrap().to_owned();
    let (shown, cookie) = page(&app, &cookie).await;
    assert!(shown.contains("welcome ada@example.com"), "{shown}");
    let (again, _) = page(&app, &cookie).await;
    assert!(!again.contains("welcome"), "{again}");
}

#[tokio::test]
async fn a_controller_form_with_a_hostile_body_is_answered_not_panicked_on() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    let mut body = encode(&[("_csrf_token", &token)]);
    body.push_str(&format!("&signup{}=x", "[a]".repeat(50_000)));

    let (status, _) = post_form(&app, &cookie, body).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// A body of `n` bare names after the CSRF token: a pair for every `&`.
fn many_pairs(token: &str, n: usize) -> String {
    format!("_csrf_token={token}{}", "&a".repeat(n))
}

#[tokio::test]
async fn a_controller_form_body_past_the_pair_limit_is_kept_to_the_limit() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    // About 900 KB: under the byte limit, with a hundred thousand times the pairs.
    let response = post_to(&app, "/count", &cookie, many_pairs(&token, 450_000)).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    // MAX_PAIRS kept, and the one more that shows the form is too large.
    assert_eq!(body, "5001");
}

#[tokio::test]
async fn a_controller_post_past_the_pair_limit_re_renders_with_the_form_error() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    // A valid email, then more pairs than a form may have.
    let mut body = encode(&[
        ("_csrf_token", &token),
        ("signup[email]", "ada@example.com"),
    ]);
    body.push_str(&"&a".repeat(5_000));

    let (status, shown) = post_form(&app, &cookie, body).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(shown.contains("too large"), "{shown}");
}

#[tokio::test]
async fn a_controller_form_body_larger_than_one_mebibyte_is_refused_with_413() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    // 1.2 MB: not a form, so refused (413 Payload Too Large) before any pair is made,
    // the token being right or not.
    for body in [many_pairs(&token, 600_000), many_pairs("wrong", 600_000)] {
        let response = post_to(&app, "/count", &cookie, body).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}

#[tokio::test]
async fn a_controller_form_post_with_a_wrong_token_is_refused_whatever_its_size() {
    let app = site();
    let (cookie, _) = visit(&app).await;

    let response = post_to(&app, "/count", &cookie, many_pairs("wrong", 1000)).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_controller_form_that_is_not_urlencoded_is_refused_with_415() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    let request = Request::post("/count")
        .header(header::COOKIE, &cookie)
        .header("x-csrf-token", &token)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("a=b"))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn csrf_token_is_found_after_any_number_of_pairs_within_the_byte_limit() {
    let app = site();
    let (cookie, token) = visit(&app).await;
    let filler = vec!["a"; 100_000].join("&");

    let response = post_to(
        &app,
        "/count",
        &cookie,
        format!("{filler}&_csrf_token={token}"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    // A large form with a wrong token is still refused.
    let response = post_to(
        &app,
        "/count",
        &cookie,
        format!("{filler}&_csrf_token=wrong"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
