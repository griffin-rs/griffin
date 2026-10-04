//! The cookie session and flash, through a small app driven as a tower service: an
//! HTTP request in, a response out, and the cookie carried from one to the next as a
//! browser would.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt as _, StreamExt as _};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{Request, StatusCode, header};
use griffin_web::axum::response::{Redirect, Response};
use griffin_web::axum::routing::{get, post};
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{LiveView, Socket, live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::{Path, Scope};
use griffin_web::session::{SecretTooShort, Session, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tower::ServiceExt as _;

const SECRET: &str = "a test secret, at least thirty-two bytes long";

async fn sign_in(session: Session, Path(name): Path<String>) -> StatusCode {
    session.insert("user", &name);
    StatusCode::NO_CONTENT
}

async fn who(session: Session) -> String {
    let user = session.get::<String>("user");
    user.unwrap_or_else(|| "nobody".to_owned())
}

async fn sign_out(session: Session) {
    session.clear();
}

/// Puts `bytes` more bytes in the session, and takes the user out of it.
async fn fill(session: Session, Path(bytes): Path<usize>) {
    session.insert("filler", "x".repeat(bytes));
    session.remove("user");
}

async fn forget_filler(session: Session) {
    session.remove("filler");
}

/// A Pipeline for a router with state of any kind.
fn browser<S: Clone + Send + Sync + 'static>(secret: &str) -> Pipeline<S> {
    Pipeline::new().layer(SessionLayer::new(secret).unwrap())
}

fn app_keyed_from(secret: &str) -> Router {
    let site = Scope::new("/")
        .pipe_through(browser(secret))
        .route("/sign-in/{name}", post(sign_in))
        .route("/sign-out", post(sign_out))
        .route("/fill/{bytes}", post(fill))
        .route("/forget-filler", post(forget_filler))
        .route("/who", get(who));
    let bare = Scope::new("/bare").route("/who", get(who));
    Router::new().merge(site).merge(bare)
}

fn app() -> Router {
    app_keyed_from(SECRET)
}

/// Sends a request with the cookie a browser would send, if it holds one.
async fn send(app: Router, request: Request<()>, cookie: Option<&str>) -> Response {
    let (mut parts, ()) = request.into_parts();
    if let Some(cookie) = cookie {
        parts
            .headers
            .insert(header::COOKIE, cookie.parse().unwrap());
    }
    let request = Request::from_parts(parts, Body::empty());
    app.oneshot(request).await.unwrap()
}

fn get_page(path: &str) -> Request<()> {
    Request::get(path).body(()).unwrap()
}

fn post_to(path: &str) -> Request<()> {
    Request::post(path).body(()).unwrap()
}

async fn body(response: Response) -> String {
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// The one `Set-Cookie` header of a response.
fn set_cookie(response: &Response) -> &str {
    let mut all = response.headers().get_all(header::SET_COOKIE).into_iter();
    let cookie = all.next().expect("no Set-Cookie header");
    assert!(all.next().is_none(), "more than one Set-Cookie header");
    cookie.to_str().unwrap()
}

/// What a browser sends back for a `Set-Cookie` header: its `name=value`.
fn cookie(response: &Response) -> String {
    let (pair, _attributes) = set_cookie(response).split_once(';').unwrap();
    pair.to_owned()
}

#[tokio::test]
async fn a_value_written_to_the_session_is_read_in_the_next_request() {
    let response = send(app(), post_to("/sign-in/ann"), None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let cookie = cookie(&response);

    let response = send(app(), get_page("/who"), Some(&cookie)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await, "ann");
}

/// The answer of `/who` to a browser that sends `cookie`.
async fn who_is(cookie: &str) -> String {
    let response = send(app(), get_page("/who"), Some(cookie)).await;
    assert_eq!(response.status(), StatusCode::OK, "{cookie}");
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    body(response).await
}

/// The session cookie of a browser signed in as `ann`: `(name, value)`.
async fn anns_cookie(app: Router) -> (String, String) {
    let response = send(app, post_to("/sign-in/ann"), None).await;
    let cookie = cookie(&response);
    let (name, value) = cookie.split_once('=').unwrap();
    (name.to_owned(), value.to_owned())
}

#[tokio::test]
async fn the_cookie_is_encrypted_and_kept_from_scripts_other_sites_and_plain_http() {
    let response = send(app(), post_to("/sign-in/ann"), None).await;

    let (pair, attributes) = set_cookie(&response).split_once("; ").unwrap();
    assert_eq!(attributes, "HttpOnly; SameSite=Lax; Secure; Path=/");
    let (name, value) = pair.split_once('=').unwrap();
    assert_eq!(name, "griffin_session");
    // A nonce of 12 bytes, the session as JSON, and a tag of 16.
    let sealed = STANDARD.decode(value).unwrap();
    assert_eq!(sealed.len(), 12 + r#"{"user":"ann"}"#.len() + 16);
    assert!(!sealed.windows(3).any(|bytes| bytes == b"ann"));
}

#[tokio::test]
async fn the_same_session_is_sealed_differently_each_time() {
    let (_, first) = anns_cookie(app()).await;
    let (_, second) = anns_cookie(app()).await;

    assert_ne!(first, second);
}

#[tokio::test]
async fn a_tampered_cookie_is_no_session() {
    let (name, value) = anns_cookie(app()).await;
    assert_eq!(who_is(&format!("{name}={value}")).await, "ann");

    // One bit changed in each byte in turn: the nonce, the ciphertext and the tag.
    let sealed = STANDARD.decode(&value).unwrap();
    for position in 0..sealed.len() {
        let mut tampered = sealed.clone();
        tampered[position] ^= 1;
        let cookie = format!("{name}={}", STANDARD.encode(tampered));

        assert_eq!(who_is(&cookie).await, "nobody", "byte {position}");
    }
}

#[tokio::test]
async fn a_truncated_cookie_is_no_session() {
    let (name, value) = anns_cookie(app()).await;

    // Cut short as text, which leaves base64 of any shape, and cut short as bytes,
    // down to less than a nonce and to nothing.
    for length in 0..value.len() {
        let cookie = format!("{name}={}", &value[..length]);
        assert_eq!(who_is(&cookie).await, "nobody", "{length} characters");
    }
    let sealed = STANDARD.decode(&value).unwrap();
    for length in 0..sealed.len() {
        let cookie = format!("{name}={}", STANDARD.encode(&sealed[..length]));
        assert_eq!(who_is(&cookie).await, "nobody", "{length} bytes");
    }
}

#[tokio::test]
async fn a_cookie_sealed_with_another_key_is_no_session() {
    let other = app_keyed_from("another secret, also thirty-two bytes long");
    let (name, value) = anns_cookie(other).await;

    assert_eq!(who_is(&format!("{name}={value}")).await, "nobody");
}

#[tokio::test]
async fn a_cookie_that_was_never_sealed_is_no_session() {
    let forged = STANDARD.encode(r#"{"user":"root"}"#);
    for value in [
        r#"{"user":"root"}"#,
        forged.as_str(),
        "",
        "=",
        "not base64 !",
        "\"quoted\"",
    ] {
        assert_eq!(who_is(&format!("griffin_session={value}")).await, "nobody");
    }
    // Beside other cookies, and in a header that is not text at all.
    assert_eq!(who_is("theme=dark; griffin_session; a=b").await, "nobody");
    let request = Request::get("/who")
        .header(header::COOKIE, &b"griffin_session=\xff\xfe"[..])
        .body(Body::empty())
        .unwrap();
    let response = app().oneshot(request).await.unwrap();
    assert_eq!(body(response).await, "nobody");
}

#[tokio::test]
async fn a_sealed_value_is_only_good_under_the_name_it_was_sealed_for() {
    // What `Cookie::split_parse` reads as another cookie's value is not the session's.
    let (name, value) = anns_cookie(app()).await;

    assert_eq!(who_is(&format!("other={value}")).await, "nobody");
    assert_eq!(who_is(&format!("other=1; {name}={value}")).await, "ann");
}

#[tokio::test]
async fn a_session_that_is_only_read_sends_no_cookie() {
    let response = send(app(), get_page("/who"), None).await;

    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(body(response).await, "nobody");
}

#[tokio::test]
async fn a_value_taken_out_of_the_session_is_gone_from_the_next_request() {
    let (name, value) = anns_cookie(app()).await;
    let signed_in = format!("{name}={value}");

    let response = send(app(), post_to("/fill/10"), Some(&signed_in)).await;

    assert_eq!(who_is(&cookie(&response)).await, "nobody");
}

#[tokio::test]
async fn an_emptied_session_tells_the_browser_to_drop_the_cookie() {
    let (name, value) = anns_cookie(app()).await;
    let signed_in = format!("{name}={value}");

    let response = send(app(), post_to("/sign-out"), Some(&signed_in)).await;

    assert_eq!(response.status(), StatusCode::OK);
    let dropped = set_cookie(&response);
    assert!(
        dropped.starts_with("griffin_session=; HttpOnly; SameSite=Lax; Secure; Path=/; Max-Age=0;"),
        "{dropped}"
    );
    // Emptied by taking out its last value, the same.
    let filled = send(app(), post_to("/fill/10"), None).await;
    let response = send(app(), post_to("/forget-filler"), Some(&cookie(&filled))).await;
    assert!(set_cookie(&response).contains("; Max-Age=0;"));
    // Signing out of nothing changes nothing.
    let response = send(app(), post_to("/sign-out"), None).await;
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn a_session_too_large_for_a_cookie_is_an_error_and_not_a_lost_session() {
    let fits = send(app(), post_to("/fill/2900"), None).await;
    let too_large = send(app(), post_to("/fill/3100"), None).await;

    assert_eq!(fits.status(), StatusCode::OK);
    assert!(set_cookie(&fits).len() <= 4096);
    assert_eq!(too_large.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(too_large.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(body(too_large).await, "");
}

#[tokio::test]
async fn a_handler_that_takes_the_session_outside_a_session_layer_is_an_error() {
    let response = send(app(), get_page("/bare/who"), None).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn the_cookie_is_sent_over_plain_http_only_when_asked_for() {
    let layer = SessionLayer::new(SECRET).unwrap().secure(false);
    let site = Scope::new("/")
        .pipe_through(Pipeline::new().layer(layer))
        .route("/sign-in/{name}", post(sign_in));

    let response = send(Router::new().merge(site), post_to("/sign-in/ann"), None).await;

    let (_, attributes) = set_cookie(&response).split_once("; ").unwrap();
    assert_eq!(attributes, "HttpOnly; SameSite=Lax; Path=/");
}

/// A LiveView that greets whoever the session says is signed in.
struct Greeting {
    user: String,
    connected: bool,
}

impl LiveView for Greeting {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), socket: &mut Socket) -> Greeting {
        let user = socket.session::<String>("user");
        Greeting {
            user: user.unwrap_or_else(|| "nobody".to_owned()),
            connected: socket.connected(),
        }
    }

    fn render(&self) -> Rendered {
        html! { <p data-connected={@connected}>Hello, {@user}</p> }
    }
}

fn live_app() -> Router {
    let pages = Scope::new("/")
        .pipe_through(browser(SECRET))
        .route("/sign-in/{name}", post(sign_in))
        .route("/greeting", live::<Greeting, _>());
    let pages = Router::new().merge(pages);
    pages
        .clone()
        .route("/live/websocket", live_socket(pages))
        .with_state(SigningKey::new(SECRET).unwrap())
}

/// What the Connected render of the Dead render `html` of `/greeting` shows, on a
/// socket opened with the cookie a browser would send, if it holds one.
async fn join_greeting(html: &str, cookie: Option<&str>) -> Value {
    let reply = reply_to_join_of_greeting(html, cookie).await;
    assert_eq!(reply["status"], "ok", "{reply}");
    reply["response"]["rendered"].clone()
}

/// The reply to that join: its status and response.
async fn reply_to_join_of_greeting(html: &str, cookie: Option<&str>) -> Value {
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        &html[start..start + html[start..].find('"').unwrap()]
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, live_app()).await.unwrap() });
    // As the page's script opens it: with the CSRF token the Dead render left.
    let csrf_token = attribute("data-csrf-token");
    let url = format!("ws://{address}/live/websocket?vsn=2.0.0&_csrf_token={csrf_token}");
    let mut upgrade = url.into_client_request().unwrap();
    if let Some(cookie) = cookie {
        let headers = upgrade.headers_mut();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
    }
    let (mut socket, _) = connect_async(upgrade).await.unwrap();

    let join = json!(["1", "1", format!("lv:{}", attribute("id")), "phx_join", {
        "url": "http://localhost/greeting",
        "params": {"_mounts": 0, "_mount_attempts": 0},
        "session": attribute("data-phx-session"),
        "static": attribute("data-phx-static"),
        "sticky": false,
    }]);
    let frame = Message::Text(join.to_string().into());
    socket.send(frame).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next());
    let reply = reply.await.expect("no frame from the server").unwrap();
    let mut reply: Value = serde_json::from_str(reply.unwrap().to_text().unwrap()).unwrap();
    reply[4].take()
}

#[tokio::test]
async fn the_session_is_read_at_mount_in_the_dead_render() {
    let (name, value) = anns_cookie(live_app()).await;
    let cookie = format!("{name}={value}");

    let response = send(live_app(), get_page("/greeting"), Some(&cookie)).await;

    assert_eq!(response.status(), StatusCode::OK);
    // The first Dead render of a session puts its CSRF token in it, for the socket.
    let cookie = self::cookie(&response);
    let html = body(response).await;
    assert!(html.contains("<p>Hello, ann</p>"), "{html}");

    // After that, reading the session does not send the cookie again.
    let response = send(live_app(), get_page("/greeting"), Some(&cookie)).await;
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    let html = body(response).await;
    assert!(html.contains("<p>Hello, ann</p>"), "{html}");
}

#[tokio::test]
async fn the_session_is_read_at_mount_in_the_connected_render() {
    let (name, value) = anns_cookie(live_app()).await;
    let cookie = format!("{name}={value}");
    let response = send(live_app(), get_page("/greeting"), Some(&cookie)).await;
    let cookie = self::cookie(&response);
    let html = body(response).await;

    let rendered = join_greeting(&html, Some(&cookie)).await;

    assert_eq!(
        rendered,
        json!({
            "0": " data-connected",
            "1": "ann",
            "s": 0,
            "p": {"0": ["<p", ">Hello, ", "</p>"]},
            "r": 1,
        })
    );
}

#[tokio::test]
async fn a_live_view_mounts_with_an_empty_session_when_there_is_no_cookie() {
    let response = send(live_app(), get_page("/greeting"), None).await;
    // The session the Dead render started holds the socket's CSRF token and no user.
    let cookie = cookie(&response);
    let html = body(response).await;
    assert!(html.contains("<p>Hello, nobody</p>"), "{html}");

    let rendered = join_greeting(&html, Some(&cookie)).await;

    assert_eq!(rendered["1"], "nobody");
}

#[tokio::test]
async fn a_tampered_cookie_is_no_session_at_mount_either() {
    let (name, value) = anns_cookie(live_app()).await;
    let mut sealed = STANDARD.decode(&value).unwrap();
    sealed[20] ^= 1;
    let cookie = format!("{name}={}", STANDARD.encode(sealed));

    let response = send(live_app(), get_page("/greeting"), Some(&cookie)).await;
    let html = body(response).await;
    let reply = reply_to_join_of_greeting(&html, Some(&cookie)).await;

    assert!(html.contains("<p>Hello, nobody</p>"), "{html}");
    // A socket opened with it has no session either, so not the one whose CSRF token
    // the page carries: nothing is mounted, and the client loads the page again.
    let refused = json!({"status": "error", "response": {"reason": "unauthorized"}});
    assert_eq!(reply, refused);
}

/// A browser as far as cookies go: it keeps what a response sets and sends it back.
#[derive(Default)]
struct Browser {
    cookie: Option<String>,
}

impl Browser {
    async fn send(&mut self, app: Router, request: Request<()>) -> Response {
        let response = send(app, request, self.cookie.as_deref()).await;
        if let Some(set_cookie) = response.headers().get(header::SET_COOKIE) {
            let set_cookie = set_cookie.to_str().unwrap();
            let dropped = set_cookie.contains("; Max-Age=0;");
            self.cookie = (!dropped).then(|| cookie(&response));
        }
        response
    }

    async fn get(&mut self, app: Router, path: &str) -> String {
        let response = self.send(app, get_page(path)).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        body(response).await
    }

    /// Posts to `path` and gives where the response redirects to.
    async fn post(&mut self, app: Router, path: &str) -> String {
        let response = self.send(app, post_to(path)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned()
    }
}

async fn save(session: Session) -> Redirect {
    session.put_flash("info", "Saved <it>");
    Redirect::to("/")
}

async fn home() -> Rendered {
    html! { <h1>Home</h1> }
}

/// Leaves a second message of a kind and one of another kind.
async fn fail(session: Session) -> Redirect {
    session.put_flash("info", "Saving");
    session.put_flash("info", "Not saved");
    session.put_flash("error", "The disk is full");
    Redirect::to("/errors")
}

/// A redirect on the way to the page.
async fn hop() -> Redirect {
    Redirect::to("/")
}

/// A page with flash of its own request.
async fn now(session: Session) -> Rendered {
    session.put_flash("info", "Just now");
    html! { <h1>Now</h1> }
}

async fn errors() -> Rendered {
    html! { <h1>Errors</h1> }
}

/// A handler outside every layout shows the flash itself.
async fn notice(session: Session) -> String {
    let flash = session.take_flash();
    flash.get("info").unwrap_or("no flash").to_owned()
}

fn flash_app() -> Router {
    let site = Scope::new("/")
        .pipe_through(browser(SECRET))
        .layout(|page, flash| {
            let info = flash.get("info").unwrap_or("no flash");
            html! { <main><p class="info">{info}</p>{page}</main> }
        })
        .route("/", get(home))
        .route("/now", get(now))
        .route("/hop", get(hop))
        .route("/who", get(who))
        .route("/greeting", live::<Greeting, _>())
        .route("/save", post(save))
        .route("/sign-in/{name}", post(sign_in));
    let errors = Scope::new("/errors")
        .pipe_through(browser(SECRET))
        .layout(|page, flash| {
            let (info, error) = (flash.get("info"), flash.get("error"));
            html! { <main><p>{info.unwrap_or("-")}, {error.unwrap_or("-")}</p>{page}</main> }
        })
        .route("/", get(errors))
        .route("/fail", post(fail));
    let bare = Scope::new("/bare")
        .pipe_through(browser(SECRET))
        .route("/notice", get(notice));
    Router::new()
        .merge(site)
        .merge(errors)
        .merge(bare)
        .with_state(SigningKey::new(SECRET).unwrap())
}

#[tokio::test]
async fn flash_set_before_a_redirect_is_shown_on_the_next_page_and_gone_on_the_one_after() {
    let mut browser = Browser::default();

    let location = browser.post(flash_app(), "/save").await;
    let next = browser.get(flash_app(), &location).await;
    let after = browser.get(flash_app(), &location).await;

    assert_eq!(location, "/");
    assert_eq!(
        next,
        "<main><p class=\"info\">Saved &lt;it&gt;</p><h1>Home</h1></main>"
    );
    assert_eq!(
        after,
        "<main><p class=\"info\">no flash</p><h1>Home</h1></main>"
    );
    // The session held nothing else, so the cookie went with the flash.
    assert_eq!(browser.cookie, None);
}

#[tokio::test]
async fn flash_is_taken_out_of_a_session_that_keeps_its_other_values() {
    let mut browser = Browser::default();
    browser.send(flash_app(), post_to("/sign-in/ann")).await;

    browser.post(flash_app(), "/save").await;
    let next = browser.get(flash_app(), "/").await;
    let after = browser.get(flash_app(), "/").await;

    assert!(
        next.contains("<p class=\"info\">Saved &lt;it&gt;</p>"),
        "{next}"
    );
    assert!(after.contains("<p class=\"info\">no flash</p>"), "{after}");
    assert!(browser.get(flash_app(), "/who").await.contains("ann"));
}

#[tokio::test]
async fn flash_waits_through_responses_that_are_not_a_page_in_a_layout() {
    let mut browser = Browser::default();
    browser.post(flash_app(), "/save").await;

    // Another redirect on the way, which has nowhere to show it.
    let hop = browser.send(flash_app(), get_page("/hop")).await;
    assert_eq!(hop.status(), StatusCode::SEE_OTHER);
    assert!(hop.headers().get(header::SET_COOKIE).is_none());

    let page = browser.get(flash_app(), "/").await;
    assert!(
        page.contains("<p class=\"info\">Saved &lt;it&gt;</p>"),
        "{page}"
    );
}

#[tokio::test]
async fn flash_left_by_the_handler_of_a_page_is_shown_on_that_page_only() {
    let mut browser = Browser::default();

    let now = browser.get(flash_app(), "/now").await;
    let after = browser.get(flash_app(), "/").await;

    assert_eq!(
        now,
        "<main><p class=\"info\">Just now</p><h1>Now</h1></main>"
    );
    assert!(after.contains("<p class=\"info\">no flash</p>"), "{after}");
    assert_eq!(browser.cookie, None);
}

#[tokio::test]
async fn flash_has_a_message_for_each_kind_and_the_last_one_left_wins() {
    let mut browser = Browser::default();

    let location = browser.post(flash_app(), "/errors/fail").await;
    let page = browser.get(flash_app(), &location).await;

    assert_eq!(
        page,
        "<main><p>Not saved, The disk is full</p><h1>Errors</h1></main>"
    );
}

#[tokio::test]
async fn a_handler_outside_a_layout_takes_the_flash_itself() {
    let mut browser = Browser::default();
    browser.post(flash_app(), "/save").await;

    let notice = browser.get(flash_app(), "/bare/notice").await;
    let after = browser.get(flash_app(), "/bare/notice").await;

    assert_eq!(notice, "Saved <it>");
    assert_eq!(after, "no flash");
}

#[tokio::test]
async fn flash_is_shown_in_the_layout_around_the_dead_render_of_a_live_view() {
    let mut browser = Browser::default();
    browser.post(flash_app(), "/save").await;

    let page = browser.get(flash_app(), "/greeting").await;
    let after = browser.get(flash_app(), "/greeting").await;

    assert!(
        page.starts_with("<main><p class=\"info\">Saved &lt;it&gt;</p><div id=\"phx-"),
        "{page}"
    );
    assert!(
        after.starts_with("<main><p class=\"info\">no flash</p><div id=\"phx-"),
        "{after}"
    );
}

#[tokio::test]
async fn a_layout_without_a_session_layer_has_no_flash() {
    let site = Scope::new("/")
        .layout(|page, flash| {
            let info = flash.get("info").unwrap_or("no flash");
            html! { <main><p>{info}</p>{page}</main> }
        })
        .route("/", get(home));

    let response = send(Router::new().merge(site), get_page("/"), None).await;

    assert_eq!(
        body(response).await,
        "<main><p>no flash</p><h1>Home</h1></main>"
    );
}

#[test]
fn a_secret_shorter_than_32_bytes_is_refused() {
    let error = SessionLayer::new("thirty-one bytes is not enough!").unwrap_err();

    assert_eq!(error, SecretTooShort);
    assert_eq!(
        error.to_string(),
        "the session secret must be at least 32 bytes long"
    );
    assert!(SessionLayer::new("thirty-two bytes are just enough").is_ok());
}

#[test]
fn debug_output_shows_neither_the_secret_nor_the_session() {
    let layer = SessionLayer::new(SECRET).unwrap();
    let session = Session::default();
    session.insert("user", "ann");

    assert_eq!(
        format!("{layer:?}"),
        "SessionLayer { key: Key, flash_key: SigningKey(REDACTED), secure: true }"
    );
    assert_eq!(format!("{session:?}"), "Session(REDACTED)");
}
