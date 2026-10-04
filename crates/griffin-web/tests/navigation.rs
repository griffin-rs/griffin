//! Live navigation, flash on redirect and the page title, through an app driven at the
//! network boundary: HTTP requests in and responses out, a real socket with the frames
//! the Phoenix client sends. The security cases have their negative tests here: a flash
//! nobody signed, a target that leaves the site, a navigation across live sessions.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{SinkExt as _, StreamExt as _};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::Method;
use griffin_web::axum::http::{HeaderMap, Request, StatusCode, header};
use griffin_web::axum::middleware::{Next, from_fn};
use griffin_web::axum::response::IntoResponse as _;
use griffin_web::axum::{self, Router};
use griffin_web::live::{
    BadTarget, Event, EventError, LiveView, Payload, QueryError, Socket, SocketChecks, live,
    live_socket_with,
};
use griffin_web::pipeline::Pipeline;
use griffin_web::session::{Flash, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use griffin_web::{html, routes};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::{self, Write as _};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};
use std::thread::{self, ThreadId};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt as _;

const SECRET: &str = "a test secret, at least thirty-two bytes long";

/// A LiveView that records what was called on it, in order, and shows it.
struct Pager {
    log: Vec<String>,
    clicks: u32,
}

#[derive(griffin_web::live::Event)]
enum PagerEvent {
    Click,
    Next,
    PatchTo {
        to: String,
    },
    /// Patches, and carries on if the target is refused.
    TryPatch {
        to: String,
    },
    NavigateTo {
        to: String,
    },
    RedirectTo {
        to: String,
    },
    /// Leaves with a flash.
    Save,
    /// Leaves a flash and then patches instead of redirecting.
    FlashThenPatch,
    Title {
        title: String,
    },
}

/// What the Pager's handlers fail with: a bad query, or a target Griffin refuses.
#[derive(Debug)]
enum PagerError {
    Query(QueryError),
    Target(BadTarget),
}

impl From<QueryError> for PagerError {
    fn from(error: QueryError) -> Self {
        PagerError::Query(error)
    }
}

impl From<BadTarget> for PagerError {
    fn from(error: BadTarget) -> Self {
        PagerError::Target(error)
    }
}

impl fmt::Display for PagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PagerError::Query(error) => error.fmt(f),
            PagerError::Target(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PagerError {}

impl LiveView for Pager {
    type Params = u32;
    type Event = PagerEvent;
    type Message = Infallible;
    type Error = PagerError;

    async fn mount(n: u32, socket: &mut Socket) -> Pager {
        let query = socket
            .query::<HashMap<String, String>>()
            .unwrap_or_default();
        if let Some(title) = query.get("title") {
            socket.set_title(title);
        }
        Pager {
            log: vec![format!("mount:{n}:{}", socket.connected())],
            clicks: 0,
        }
    }

    async fn handle_params(&mut self, n: u32, socket: &mut Socket) -> Result<(), Self::Error> {
        let query = socket.query::<HashMap<String, String>>()?;
        self.log
            .push(format!("params:{n}:{}", query.get("q").map_or("", |q| q)));
        match n {
            // Leaves for a page, with a flash.
            0 => {
                socket.put_flash("info", "Too low");
                socket.redirect("/landing")?;
            }
            // A loop.
            99 => socket.patch("/pages/99")?,
            _ => {}
        }
        Ok(())
    }

    async fn handle_event(
        &mut self,
        event: PagerEvent,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            PagerEvent::Click => self.clicks += 1,
            PagerEvent::Next => socket.patch(&Routes::pager(2))?,
            PagerEvent::PatchTo { to } => socket.patch(&to)?,
            // The refusal is at the call: the page says so, and the LiveView lives on.
            PagerEvent::TryPatch { to } => {
                if socket.patch(&to) == Err(BadTarget) {
                    self.clicks += 100;
                }
            }
            PagerEvent::NavigateTo { to } => socket.navigate(&to)?,
            PagerEvent::RedirectTo { to } => socket.redirect(&to)?,
            PagerEvent::Save => {
                socket.put_flash("info", "Saved <b>");
                socket.redirect(&Routes::landing())?;
            }
            PagerEvent::FlashThenPatch => {
                socket.put_flash("info", "Stale");
                socket.patch(&Routes::pager(2))?;
            }
            PagerEvent::Title { title } => socket.set_title(title),
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! {
            <section>
                <p id="log">{@log.join(",")}</p>
                <p>clicks {@clicks}</p>
                <a patch={Routes::pager(3)}>Three</a>
                <a navigate={Routes::other()}>Other</a>
            </section>
        }
    }
}

/// Another LiveView of the same live session, which sets the title.
struct Other;

impl LiveView for Other {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_: (), socket: &mut Socket) -> Other {
        socket.set_title("Other page");
        Other
    }

    fn render(&self) -> Rendered {
        html! { <h1>Other</h1> }
    }
}

/// A LiveView of another live session.
struct Secret;

impl LiveView for Secret {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_: (), _socket: &mut Socket) -> Secret {
        Secret
    }

    fn render(&self) -> Rendered {
        html! { <h1>Secret</h1> }
    }
}

/// The LiveView of the conformance case `page_title`.
struct Titled {
    count: String,
}

struct Step(Value);

impl Event for Step {
    fn decode(name: &str, payload: &Payload<'_>) -> Result<Step, EventError> {
        match name {
            "step" => Ok(Step(payload.value().clone())),
            _ => Err(EventError::Unknown),
        }
    }

    fn encode(&self) -> (&'static str, Vec<(&'static str, String)>) {
        ("step", vec![])
    }
}

impl LiveView for Titled {
    type Params = ();
    type Event = Step;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_: (), _socket: &mut Socket) -> Titled {
        Titled {
            count: String::new(),
        }
    }

    async fn handle_event(
        &mut self,
        Step(step): Step,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        if let Some(count) = step["count"].as_str() {
            self.count = count.to_owned();
        }
        if let Some(title) = step["page_title"].as_str() {
            socket.set_title(title);
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <p>{@count}</p> }
    }
}

fn site(page: Rendered, flash: &Flash) -> Rendered {
    html! {
        <main>
            <p :if={flash.get("info").is_some()} class="flash">{flash.get("info").unwrap()}</p>
            {page}
        </main>
    }
}

async fn landing() -> Rendered {
    html! { <h1>Landing</h1> }
}

/// How many times the controller `/counted` ran.
static COUNTED: AtomicUsize = AtomicUsize::new(0);

async fn counted() -> Rendered {
    COUNTED.fetch_add(1, Ordering::SeqCst);
    html! { <h1>Counted</h1> }
}

fn browser(session: SessionLayer) -> Pipeline<SigningKey> {
    Pipeline::browser(session)
}

routes! {
    fn router(session: SessionLayer) -> Router<SigningKey>;

    scope "/" {
        pipe_through [browser(session)];

        live_session main {
            LIVE "/pages/{n: u32}" => Pager as pager;
            LIVE "/other" => Other as other;
        }
        live_session admin {
            LIVE "/admin/secret" => Secret as secret;
            LIVE "/admin/pages/{n: u32}" => Pager as admin_pager;
        }
        // In no live session, and with a layout that shows the flash.
        LIVE "/titled" => Titled as titled;
        scope "/" {
            layout site;
            GET "/landing" => landing as landing;
            GET "/counted" => counted as counted;
        }
    }
}

fn app() -> Router {
    router(SessionLayer::new(SECRET).unwrap()).with_state(SigningKey::new(SECRET).unwrap())
}

/// Serves the app on a local port, as a browser would reach it.
async fn serve() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app()).await.unwrap() });
    address
}

struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Response {
    /// The `name=value` of each cookie the response sets, by name.
    fn cookies(&self) -> HashMap<String, String> {
        let set = self.headers.get_all(header::SET_COOKIE).into_iter();
        set.map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
            .map(|pair| pair.split_once('=').unwrap())
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }
}

/// A GET as a browser sends it, with these cookies.
async fn get_page(path: &str, cookie: &str) -> Response {
    let mut request = Request::get(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    Response {
        status,
        headers,
        body,
    }
}

/// What a browser holds after it loaded a LiveView's page, and sends on joining.
struct Page {
    /// `griffin_session=..`
    cookie: String,
    csrf_token: String,
    topic: String,
    session: String,
    static_token: String,
    path: String,
}

impl Page {
    async fn load(path: &str) -> Page {
        let response = get_page(path, "").await;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        let session = response.cookies().remove("griffin_session").unwrap();
        let html = response.body;
        let attribute = |name: &str| {
            let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
            html[start..start + html[start..].find('"').unwrap()].to_owned()
        };
        Page {
            cookie: format!("griffin_session={session}"),
            csrf_token: attribute("data-csrf-token"),
            topic: format!("lv:{}", attribute("id")),
            session: attribute("data-phx-session"),
            static_token: attribute("data-phx-static"),
            path: path.to_owned(),
        }
    }

    /// The join of the page, as the client sends it.
    fn join(&self) -> Value {
        self.join_with(json!({"url": format!("http://localhost{}", self.path)}))
    }

    /// The join that follows a live navigation to `to`, as the client sends it: the
    /// tokens of the page it left, and `redirect` in place of `url`.
    fn navigation_join(&self, to: &str) -> Value {
        self.join_with(json!({"redirect": format!("http://localhost{to}")}))
    }

    fn join_with(&self, target: Value) -> Value {
        let mut payload = json!({
            "params": {"_mounts": 0, "_mount_attempts": 0},
            "session": self.session,
            "static": self.static_token,
            "sticky": false,
        });
        for (name, value) in target.as_object().unwrap() {
            payload[name] = value.clone();
        }
        json!(["4", "4", self.topic, "phx_join", payload])
    }
}

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Client(Stream);

impl Client {
    /// Opens the socket as the page's script does, with the CSRF token.
    async fn open(page: &Page) -> Client {
        Client::open_with(page, &page.csrf_token).await
    }

    async fn open_with(page: &Page, csrf_token: &str) -> Client {
        let address = serve().await;
        let url = format!("ws://{address}/live/websocket?vsn=2.0.0&_csrf_token={csrf_token}");
        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(url)
                .unwrap();
        request
            .headers_mut()
            .insert(header::COOKIE, page.cookie.parse().unwrap());
        Client(connect_async(request).await.unwrap().0)
    }

    /// A socket joined on `page`, with the reply to the join.
    async fn joined(page: &Page) -> (Client, Value) {
        let mut client = Client::open(page).await;
        client.send(page.join()).await;
        let reply = client.receive().await;
        assert_eq!(reply[4]["status"], "ok", "{reply}");
        (client, reply)
    }

    async fn send(&mut self, frame: Value) {
        self.0
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    async fn receive(&mut self) -> Value {
        let next = tokio::time::timeout(Duration::from_secs(5), self.0.next());
        let frame = next.await.expect("no frame from the server");
        let text = frame.expect("the server closed the socket").unwrap();
        serde_json::from_str(text.to_text().unwrap()).unwrap()
    }

    /// Sends the Event `name` with `value`, as a click on a binding does, and gives
    /// every frame up to and including its reply.
    async fn event(&mut self, page: &Page, name: &str, value: Value) -> Vec<Value> {
        let event = json!({"type": "click", "event": name, "value": value});
        self.send(json!(["4", "9", page.topic, "event", event]))
            .await;
        let mut frames = Vec::new();
        loop {
            let frame = self.receive().await;
            let done = frame[3] == "phx_reply" || frame[3] == "phx_error";
            frames.push(frame);
            if done {
                return frames;
            }
        }
    }
}

/// The response of the reply to the event `name`.
async fn event_reply(client: &mut Client, page: &Page, name: &str, value: Value) -> Value {
    let frames = client.event(page, name, value).await;
    let reply = frames.last().unwrap().clone();
    assert_eq!(reply[3], "phx_reply", "{reply}");
    assert_eq!(reply[4]["status"], "ok", "{reply}");
    reply[4]["response"].clone()
}

/// What a LiveView that ended is answered with: its channel crashed.
fn is_crash(frames: &[Value]) -> bool {
    frames.last().is_some_and(|frame| frame[3] == "phx_error")
}

/// The rendered log of a LiveView's page, from a diff or a render.
fn logged(rendered: &Value) -> String {
    rendered["0"].as_str().unwrap_or_default().to_owned()
}

// ---- parameter handling ----

#[tokio::test]
async fn the_dead_render_runs_mount_and_then_handle_params() {
    let response = get_page("/pages/3?q=tea", "").await;

    assert_eq!(response.status, StatusCode::OK);
    assert!(
        response.body.contains("mount:3:false,params:3:tea"),
        "{}",
        response.body
    );
}

#[tokio::test]
async fn the_connected_render_runs_mount_and_then_handle_params() {
    let page = Page::load("/pages/3?q=tea").await;

    let (_client, reply) = Client::joined(&page).await;

    let rendered = &reply[4]["response"]["rendered"];
    assert_eq!(logged(rendered), "mount:3:true,params:3:tea");
}

// ---- patch ----

#[tokio::test]
async fn a_patch_updates_the_url_and_handles_the_new_parameters_and_the_state_is_kept() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;
    event_reply(&mut client, &page, "click", json!({})).await;

    let frames = client.event(&page, "next", json!({})).await;

    // The URL is patched first, then the page is updated for it.
    assert_eq!(
        frames[0],
        json!(["4", null, page.topic, "live_patch", {"to": "/pages/2", "kind": "push"}])
    );
    let diff = &frames[1][4]["response"]["diff"];
    // `handle_params` ran for the new path and mount did not run again.
    assert_eq!(logged(diff), "mount:1:true,params:1:,params:2:");
    assert_eq!(frames.len(), 2);
    // The click before the patch is still counted: the state was kept.
    let diff = &event_reply(&mut client, &page, "click", json!({})).await["diff"];
    assert_eq!(diff["1"], "2");
}

#[tokio::test]
async fn a_patch_link_clicked_in_the_browser_is_handled_without_a_new_mount() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    // What the client sends when `<a patch=..>` is clicked.
    let frame =
        json!(["4", "9", page.topic, "live_patch", {"url": "http://localhost/pages/5?q=x"}]);
    client.send(frame).await;

    let reply = client.receive().await;
    assert_eq!(reply[4]["status"], "ok");
    assert_eq!(
        logged(&reply[4]["response"]["diff"]),
        "mount:1:true,params:1:,params:5:x"
    );
}

#[tokio::test]
async fn a_patch_link_to_anything_but_this_live_view_in_this_live_session_is_a_navigation() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    for url in [
        // Another LiveView, a page that is not live, a path that is none of the routes,
        // the same LiveView in another live session, and what is not a URL.
        "http://localhost/other",
        "http://localhost/landing",
        "http://localhost/nowhere",
        "http://localhost/admin/pages/1",
        "http://localhost/pages/not-a-number",
        "::",
    ] {
        let frame = json!(["4", "9", page.topic, "live_patch", {"url": url}]);
        client.send(frame).await;

        let reply = client.receive().await;
        assert_eq!(
            reply,
            json!(["4", "9", page.topic, "phx_reply", {"status": "ok", "response": {"link_redirect": true}}]),
            "{url}"
        );
    }
}

#[tokio::test]
async fn a_patch_from_the_server_to_anywhere_else_ends_the_live_view() {
    for to in ["/other", "/landing", "/nowhere", "/admin/pages/1"] {
        let page = Page::load("/pages/1").await;
        let (mut client, _) = Client::joined(&page).await;

        let frames = client.event(&page, "patch_to", json!({"to": to})).await;

        assert!(is_crash(&frames), "{to}: {frames:?}");
    }
}

#[tokio::test]
async fn patching_in_a_loop_ends_the_live_view() {
    let page = Page::load("/pages/2").await;
    let (mut client, _) = Client::joined(&page).await;

    let frames = client
        .event(&page, "patch_to", json!({"to": "/pages/99"}))
        .await;

    assert!(is_crash(&frames), "{frames:?}");
}

// ---- navigate ----

#[tokio::test]
async fn navigating_tells_the_browser_to_join_the_other_live_view() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    let response = event_reply(&mut client, &page, "navigate_to", json!({"to": "/other"})).await;

    assert_eq!(
        response,
        json!({"live_redirect": {"to": "/other", "kind": "push"}})
    );
}

#[tokio::test]
async fn a_join_that_follows_a_navigation_mounts_the_other_live_view_of_the_same_live_session() {
    let page = Page::load("/pages/1").await;
    let mut client = Client::open(&page).await;

    client.send(page.navigation_join("/other")).await;

    let reply = client.receive().await;
    assert_eq!(reply[4]["status"], "ok", "{reply}");
    let rendered = &reply[4]["response"]["rendered"];
    // The other LiveView's page, with the title it set.
    assert!(
        rendered.to_string().contains("<h1>Other</h1>"),
        "{rendered}"
    );
    assert_eq!(rendered["t"], "Other page");
}

#[tokio::test]
async fn a_join_that_follows_a_navigation_across_live_sessions_is_refused() {
    let page = Page::load("/pages/1").await;
    let mut client = Client::open(&page).await;

    // `/admin/secret` is in another live session: the client loads that page instead,
    // which gives it the session that route's Pipeline has.
    client.send(page.navigation_join("/admin/secret")).await;

    let reply = client.receive().await;
    assert_eq!(
        reply,
        json!(["4", "4", page.topic, "phx_reply", {"status": "error", "response": {"reason": "unauthorized"}}])
    );
}

#[tokio::test]
async fn a_join_that_follows_a_navigation_to_a_page_that_is_not_live_is_refused() {
    let page = Page::load("/pages/1").await;

    for to in [
        "/landing",
        "/nowhere",
        "//evil.example/other",
        "/pages/not-a-number",
    ] {
        let mut client = Client::open(&page).await;
        client.send(page.navigation_join(to)).await;

        let reply = client.receive().await;
        assert_eq!(
            reply[4],
            json!({"status": "error", "response": {"reason": "unauthorized"}}),
            "{to}"
        );
    }
}

#[tokio::test]
async fn the_view_and_the_live_session_a_token_was_signed_for_bind_a_join_of_the_page() {
    let page = Page::load("/pages/1").await;
    let view = std::any::type_name::<Pager>();
    let id = page.topic.trim_start_matches("lv:");
    // A genuine token, but of a Pager in the admin live session.
    let other_session = sign(
        SECRET.as_bytes(),
        "live session",
        Duration::ZERO,
        json!({"id": id, "view": view, "live_session": "admin"}),
    );
    let mut client = Client::open(&page).await;
    let mut join = page.join();
    join[4]["session"] = json!(other_session);

    client.send(join.clone()).await;
    let reply = client.receive().await;

    // The page the token is of is `/pages/1`, in `main`.
    assert_eq!(reply[4]["response"]["reason"], "unauthorized", "{reply}");

    // The same token follows a navigation only to routes of the live session it names.
    let mut join = page.navigation_join("/admin/secret");
    join[4]["session"] = json!(other_session);
    client.send(join).await;
    assert_eq!(client.receive().await[4]["status"], "ok");
}

#[tokio::test]
async fn a_join_that_follows_a_navigation_is_refused_without_genuine_tokens() {
    let page = Page::load("/pages/1").await;
    let id = page.topic.trim_start_matches("lv:");
    let view = std::any::type_name::<Pager>();
    // A token for the admin live session, signed by someone who lacks the key.
    let forged = sign(
        b"not the key of this server, but long enough",
        "live session",
        Duration::ZERO,
        json!({"id": id, "view": view, "live_session": "admin"}),
    );

    for session in [forged, changed(&page.session)] {
        let mut client = Client::open(&page).await;
        let mut join = page.navigation_join("/admin/secret");
        join[4]["session"] = json!(session);
        client.send(join).await;

        let reply = client.receive().await;
        assert_eq!(reply[4]["response"]["reason"], "stale", "{reply}");
    }

    // A join says where it goes in one way: both `url` and `redirect` is nonsense.
    let mut client = Client::open(&page).await;
    let mut join = page.navigation_join("/other");
    join[4]["url"] = json!("http://localhost/pages/1");
    client.send(join).await;
    assert_eq!(client.receive().await[4]["response"]["reason"], "stale");
}

#[tokio::test]
async fn a_join_that_follows_a_navigation_needs_the_csrf_token_of_the_session() {
    let page = Page::load("/pages/1").await;
    let mut client = Client::open_with(&page, "not-a-token").await;

    client.send(page.navigation_join("/other")).await;

    let reply = client.receive().await;
    assert_eq!(reply[4]["response"]["reason"], "unauthorized", "{reply}");
}

/// The text with one character of its first part changed.
fn changed(token: &str) -> String {
    let other = if token.starts_with('A') { "B" } else { "A" };
    format!("{other}{}", &token[1..])
}

// ---- redirect with flash ----

/// A token as `griffin_web::token` documents, signed here so that a test chooses the
/// key, the purpose and the age.
fn sign(key: &[u8], purpose: &str, age: Duration, data: Value) -> String {
    let issued_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - age;
    let envelope = json!({
        "purpose": purpose,
        "issued_at": issued_at.as_millis() as u64,
        "data": data,
    });
    let payload = URL_SAFE_NO_PAD.encode(envelope.to_string());
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{payload}.{signature}")
}

#[tokio::test]
async fn a_redirect_from_a_live_view_carries_a_flash_the_next_page_shows_once() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    let response = event_reply(&mut client, &page, "save", json!({})).await;

    let redirect = &response["redirect"];
    assert_eq!(redirect["to"], "/landing");
    // What the client does with it: it keeps the token in a cookie and loads the page.
    let token = redirect["flash"].as_str().unwrap();
    let cookie = format!("{}; __phoenix_flash__={token}", page.cookie);
    let landing = get_page("/landing", &cookie).await;
    assert_eq!(landing.status, StatusCode::OK);
    // The message, escaped, once.
    assert!(
        landing
            .body
            .contains(r#"<p class="flash">Saved &lt;b&gt;</p>"#),
        "{}",
        landing.body
    );
    // The cookie is spent, and the browser is told to drop it.
    let set_cookie: Vec<_> = landing
        .headers
        .get_all(header::SET_COOKIE)
        .into_iter()
        .collect();
    assert!(
        set_cookie.iter().any(|cookie| {
            let cookie = cookie.to_str().unwrap();
            cookie.starts_with("__phoenix_flash__=;") && cookie.contains("Max-Age=0")
        }),
        "{set_cookie:?}"
    );
    // The page after that has none, with the session the first one set.
    let session = landing.cookies().remove("griffin_session").unwrap();
    let after = get_page("/landing", &format!("griffin_session={session}")).await;
    assert!(!after.body.contains("flash"), "{}", after.body);
}

#[tokio::test]
async fn a_redirect_without_flash_carries_none() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    let response = event_reply(
        &mut client,
        &page,
        "redirect_to",
        json!({"to": "/landing?a=1"}),
    )
    .await;

    assert_eq!(response, json!({"redirect": {"to": "/landing?a=1"}}));
}

#[tokio::test]
async fn a_flash_the_server_did_not_sign_is_not_shown() {
    let page = Page::load("/pages/1").await;
    // A genuine token, from a real redirect: its message is "Saved <b>".
    let (mut client, _) = Client::joined(&page).await;
    let response = event_reply(&mut client, &page, "save", json!({})).await;
    let genuine = response["redirect"]["flash"].as_str().unwrap().to_owned();
    let (payload, signature) = genuine.split_once('.').unwrap();
    // What it holds, which the browser can read, with other words in place of it.
    let envelope = URL_SAFE_NO_PAD.decode(payload).unwrap();
    let envelope = String::from_utf8(envelope).unwrap();
    let swapped = envelope.replace("Saved <b>", "You have won a prize");
    assert_ne!(swapped, envelope, "the token holds the message");
    let swapped = URL_SAFE_NO_PAD.encode(swapped);

    let prize = json!({"info": "You have won a prize"});
    let tokens = [
        // The genuine signature on another message.
        format!("{swapped}.{signature}"),
        // Signed by someone who lacks the key.
        sign(
            b"not the key of this server, but long enough",
            "live flash",
            Duration::ZERO,
            prize.clone(),
        ),
        // Signed with the key of the live tokens, which is not the flash key.
        sign(SECRET.as_bytes(), "live flash", Duration::ZERO, prize),
        // A genuine token for something else: the render token of the page.
        page.session.clone(),
        // Changed after it was signed.
        changed(&genuine),
        format!("{genuine}x"),
        // Not a token.
        "garbage".to_owned(),
        "{info:Youhavewonaprize}".to_owned(),
    ];
    // The genuine token is shown, so that the refusals below mean something.
    let cookie = format!("{}; __phoenix_flash__={genuine}", page.cookie);
    let shown = get_page("/landing", &cookie).await;
    assert!(shown.body.contains("Saved"), "{}", shown.body);

    for token in tokens {
        let cookie = format!("{}; __phoenix_flash__={token}", page.cookie);
        let landing = get_page("/landing", &cookie).await;

        assert_eq!(landing.status, StatusCode::OK);
        assert!(!landing.body.contains("prize"), "{token}: {}", landing.body);
        assert!(!landing.body.contains("flash"), "{token}: {}", landing.body);
    }
}

#[tokio::test]
async fn a_flash_token_is_not_taken_for_one_of_the_live_tokens_and_the_other_way_round() {
    let page = Page::load("/pages/1").await;

    // The flash token of a redirect, given as the session token of a join.
    let (mut client, _) = Client::joined(&page).await;
    let response = event_reply(&mut client, &page, "save", json!({})).await;
    let flash = response["redirect"]["flash"].as_str().unwrap().to_owned();
    let mut client = Client::open(&page).await;
    let mut join = page.join();
    join[4]["session"] = json!(flash);
    client.send(join).await;

    assert_eq!(client.receive().await[4]["response"]["reason"], "stale");
}

#[tokio::test]
async fn a_redirect_from_handle_params_in_the_dead_render_leaves_the_flash_in_the_session() {
    let response = get_page("/pages/0", "").await;

    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.headers[header::LOCATION], "/landing");
    let session = response.cookies().remove("griffin_session").unwrap();
    let landing = get_page("/landing", &format!("griffin_session={session}")).await;
    assert!(
        landing.body.contains(r#"<p class="flash">Too low</p>"#),
        "{}",
        landing.body
    );
}

#[tokio::test]
async fn a_redirect_from_handle_params_at_the_join_answers_the_join_with_the_redirect() {
    let page = Page::load("/pages/1").await;
    let mut client = Client::open(&page).await;

    // The page the client joins is another one than the Dead render was of.
    let mut join = page.join();
    join[4]["url"] = json!("http://localhost/pages/0");
    client.send(join).await;

    let reply = client.receive().await;
    assert_eq!(reply[4]["status"], "error", "{reply}");
    let redirect = &reply[4]["response"]["redirect"];
    assert_eq!(redirect["to"], "/landing");
    assert!(redirect["flash"].is_string());
}

// ---- where a target may lead ----

/// Targets that would take the user to another site, or that no browser reads as the
/// path they look like.
const OFF_SITE: [&str; 9] = [
    "//evil.example",
    "//evil.example/pages/1",
    "/\\evil.example",
    "https://evil.example/",
    "javascript:alert(1)",
    "/\t/evil.example",
    "/\n/evil.example",
    "evil.example",
    "",
];

#[tokio::test]
async fn a_target_that_is_not_a_local_path_ends_the_live_view_and_is_never_sent() {
    for event in ["redirect_to", "navigate_to", "patch_to"] {
        for to in OFF_SITE {
            let page = Page::load("/pages/1").await;
            let (mut client, _) = Client::joined(&page).await;

            let frames = client.event(&page, event, json!({"to": to})).await;

            assert!(is_crash(&frames), "{event} {to:?}: {frames:?}");
            let sent = Value::Array(frames).to_string();
            assert!(!sent.contains("evil"), "{event} {to:?}: {sent}");
        }
    }
}

#[tokio::test]
async fn a_local_path_with_a_query_that_names_another_site_is_still_a_local_path() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;
    let to = "/landing?next=//evil.example";

    let response = event_reply(&mut client, &page, "redirect_to", json!({"to": to})).await;

    assert_eq!(response, json!({"redirect": {"to": to}}));
}

// ---- no session layer ----

struct Lonely;

#[derive(griffin_web::live::Event)]
enum LonelyEvent {
    Leave,
}

impl LiveView for Lonely {
    type Params = ();
    type Event = LonelyEvent;
    type Message = Infallible;
    type Error = BadTarget;

    async fn mount(_: (), _socket: &mut Socket) -> Lonely {
        Lonely
    }

    async fn handle_event(
        &mut self,
        _: LonelyEvent,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        socket.put_flash("info", "Nobody can carry this");
        socket.redirect("/")?;
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <button phx-click={LonelyEvent::Leave}>Leave</button> }
    }
}

#[tokio::test]
async fn a_redirect_with_flash_from_a_scope_without_a_session_layer_ends_the_live_view() {
    let pages = Router::new().route("/", live::<Lonely, _>());
    let checks = griffin_web::live::SocketChecks::new().without_csrf_check();
    let app: Router = pages
        .clone()
        .route(
            "/live/websocket",
            griffin_web::live::live_socket_with(pages, checks),
        )
        .with_state(SigningKey::new(SECRET).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app.clone()).await.unwrap() });
    let html = reqwest_free_get(address, "/").await;
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    };
    let topic = format!("lv:{}", attribute("id"));
    let (mut stream, _) = connect_async(format!("ws://{address}/live/websocket?vsn=2.0.0"))
        .await
        .unwrap();
    let join = json!(["4", "4", topic, "phx_join", {
        "url": "http://localhost/", "params": {}, "session": attribute("data-phx-session"),
        "static": attribute("data-phx-static"),
    }]);
    stream
        .send(Message::Text(join.to_string().into()))
        .await
        .unwrap();
    stream.next().await.unwrap().unwrap();
    let event = json!({"type": "click", "event": "leave", "value": {}});
    stream
        .send(Message::Text(
            json!(["4", "9", topic, "event", event]).to_string().into(),
        ))
        .await
        .unwrap();

    let frame = stream.next().await.unwrap().unwrap();

    let frame: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(frame[3], "phx_error", "{frame}");
}

/// A GET over a TCP connection, for an app without a session layer to keep the test
/// to the crates it already uses.
async fn reqwest_free_get(address: SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut body = String::new();
    stream.read_to_string(&mut body).await.unwrap();
    body
}

// ---- the title ----

#[tokio::test]
async fn the_dead_render_writes_the_title_in_the_head_escaped() {
    let response = get_page("/titled", "").await;
    assert!(
        response.body.contains("<title></title>"),
        "no title set: an empty element, so the client has a title to read"
    );

    let response = get_page(
        "/pages/1?title=%3C%2Ftitle%3E%3Cscript%3Ex%3C%2Fscript%3E",
        "",
    )
    .await;

    assert!(
        response
            .body
            .contains("<title>&lt;/title&gt;&lt;script&gt;x&lt;/script&gt;</title>"),
        "{}",
        response.body
    );
    assert!(!response.body.contains("<script>x"), "{}", response.body);
}

#[tokio::test]
async fn the_diff_has_the_title_under_the_key_phoenix_uses() {
    let (case, phoenix) = (conformance("cases"), conformance("fixtures"));
    let steps = case["steps"].as_array().unwrap();
    let page = Page::load("/titled").await;
    let (mut client, joined) = Client::joined(&page).await;
    // The first step is the join's render, which has no title yet: set it with the
    // step's own Event, then compare the diffs after it.
    assert_eq!(joined[4]["response"]["rendered"]["t"], Value::Null);

    for (number, step) in steps.iter().enumerate() {
        let frames = client.event(&page, "step", step.clone()).await;
        let reply = &frames[0];
        let response = &reply[4]["response"];
        let expected = &phoenix[number];
        if number == 0 {
            // The fixture's first diff is a full render: its title is what matters.
            assert_eq!(response["diff"]["t"], expected["t"], "step {number}");
        } else if expected.as_object().unwrap().is_empty() {
            assert_eq!(response, &json!({}), "step {number}");
        } else {
            assert_eq!(response, &json!({"diff": expected}), "step {number}");
        }
    }
}

fn conformance(dir: &str) -> Value {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/tests/conformance/{dir}/page_title.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn setting_the_title_to_what_the_browser_has_sends_nothing() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;
    event_reply(&mut client, &page, "title", json!({"title": "Same"})).await;

    let response = event_reply(&mut client, &page, "title", json!({"title": "Same"})).await;

    assert_eq!(response, json!({}));
}

// ---- live links ----

#[test]
fn live_link_attributes_take_path_helpers() {
    let by_table = html! {
        <a navigate={Routes::other()}>Other</a>
        <a patch={Routes::pager(3)} class="next">Next</a>
    };

    assert_eq!(
        by_table.to_html(),
        concat!(
            r#"<a href="/other" data-phx-link="redirect" data-phx-link-state="push">Other</a>"#,
            "\n",
            r#"<a href="/pages/3" data-phx-link="patch" data-phx-link-state="push" class="next">Next</a>"#,
        )
    );
}

#[test]
fn a_live_link_target_is_escaped_like_any_attribute() {
    let to = r#"/pages/1"><script>alert(1)</script>"#.to_owned();

    let link = html! { <a navigate={to}>x</a> };

    assert!(!link.to_html().contains("<script>"), "{}", link.to_html());
}

#[test]
fn the_route_table_has_a_path_helper_for_each_live_route() {
    assert_eq!(Routes::pager(7), "/pages/7");
    assert_eq!(Routes::other(), "/other");
    assert_eq!(Routes::admin_pager(1), "/admin/pages/1");
}

/// Whether a join that follows a navigation from `page` to `to` is let through.
async fn follows(page: &Page, to: &str) -> bool {
    let mut client = Client::open(page).await;
    client.send(page.navigation_join(to)).await;
    client.receive().await[4]["status"] == "ok"
}

#[tokio::test]
async fn the_route_table_puts_live_routes_in_the_live_session_around_them() {
    let main = Page::load("/pages/1").await;
    let admin = Page::load("/admin/secret").await;
    let none = Page::load("/titled").await;

    // From each page, only the LiveViews of its own live session are joined.
    assert!(follows(&main, "/other").await);
    assert!(!follows(&main, "/admin/secret").await);
    assert!(!follows(&main, "/titled").await);
    assert!(follows(&admin, "/admin/pages/1").await);
    assert!(!follows(&admin, "/other").await);
    assert!(!follows(&none, "/other").await);
    assert!(!follows(&none, "/admin/secret").await);
}

#[tokio::test]
async fn a_join_or_a_patch_runs_no_controller_on_the_path_it_looks_up() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    // A patch link to the controller's path, and a navigation to it.
    let frame = json!(["4", "9", page.topic, "live_patch", {"url": "http://localhost/counted"}]);
    client.send(frame).await;
    assert_eq!(
        client.receive().await[4]["response"],
        json!({"link_redirect": true})
    );
    let mut other = Client::open(&page).await;
    other.send(page.navigation_join("/counted")).await;
    assert_eq!(
        other.receive().await[4]["response"]["reason"],
        "unauthorized"
    );
    // And a join of a page whose url is the controller's.
    let mut join = page.join();
    join[4]["url"] = json!("http://localhost/counted");
    other.send(join).await;
    assert_eq!(
        other.receive().await[4]["response"]["reason"],
        "unauthorized"
    );

    assert_eq!(COUNTED.load(Ordering::SeqCst), 0);
    // The controller does run for the visit that is one.
    assert_eq!(get_page("/counted", "").await.status, StatusCode::OK);
    assert_eq!(COUNTED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_trace_request_is_no_visit_to_a_live_view() {
    let request = Request::builder()
        .method("TRACE")
        .uri("/pages/1")
        .body(Body::empty())
        .unwrap();

    let response = app().oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn a_target_with_a_space_in_its_query_is_refused_and_an_encoded_one_is_followed() {
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;

    // Refused at the call: the handler sees `Err(BadTarget)` and carries on, so the
    // LiveView lives, nothing is patched, and the page shows what the handler did.
    for (refused, to) in ["/pages/2?q=red shoes", "/pages/2?q=\"", "/pages/2?q=<b>"]
        .into_iter()
        .enumerate()
    {
        let frames = client.event(&page, "try_patch", json!({"to": to})).await;
        assert_eq!(frames.len(), 1, "{to}: {frames:?}");
        assert_eq!(frames[0][4]["status"], "ok", "{to}: {frames:?}");
        let clicks = (refused + 1) * 100;
        assert_eq!(
            frames[0][4]["response"]["diff"]["1"],
            clicks.to_string(),
            "{to}"
        );
    }
    // Non-ASCII text is accepted as it is.
    let frames = client
        .event(&page, "try_patch", json!({"to": "/pages/2?q=ไทย"}))
        .await;
    assert_eq!(frames[0][3], "live_patch", "{frames:?}");
    assert_eq!(
        logged(&frames[1][4]["response"]["diff"]),
        "mount:1:true,params:1:,params:2:ไทย"
    );

    let (mut client, _) = Client::joined(&page).await;
    let frames = client
        .event(&page, "patch_to", json!({"to": "/pages/2?q=red%20shoes"}))
        .await;
    assert_eq!(frames[0][3], "live_patch", "{frames:?}");
    let diff = &frames[1][4]["response"]["diff"];
    assert_eq!(logged(diff), "mount:1:true,params:1:,params:2:red shoes");
}

#[tokio::test]
async fn flash_does_not_outlive_a_patch_or_the_redirect_that_carried_it() {
    // A flash left before a patch is gone when a later redirect is made.
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;
    event_reply(&mut client, &page, "flash_then_patch", json!({})).await;
    let response = event_reply(&mut client, &page, "redirect_to", json!({"to": "/landing"})).await;
    assert_eq!(response, json!({"redirect": {"to": "/landing"}}));

    // So is one a redirect has carried, though the LiveView lives on.
    let page = Page::load("/pages/1").await;
    let (mut client, _) = Client::joined(&page).await;
    let response = event_reply(&mut client, &page, "save", json!({})).await;
    assert!(response["redirect"]["flash"].is_string());
    let response = event_reply(&mut client, &page, "redirect_to", json!({"to": "/landing"})).await;
    assert_eq!(response, json!({"redirect": {"to": "/landing"}}));
}

struct DeadFlash;

impl LiveView for DeadFlash {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = BadTarget;

    async fn mount(_: (), _socket: &mut Socket) -> DeadFlash {
        DeadFlash
    }

    async fn handle_params(&mut self, _: (), socket: &mut Socket) -> Result<(), Self::Error> {
        socket.put_flash("info", "Nobody can keep this");
        socket.redirect("/")?;
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <p>never</p> }
    }
}

#[tokio::test]
async fn a_redirect_with_flash_in_the_dead_render_without_a_session_layer_fails_as_the_connected_one_does()
 {
    let app: Router = Router::new()
        .route("/", live::<DeadFlash, _>())
        .with_state(SigningKey::new(SECRET).unwrap());

    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.headers().get(header::LOCATION).is_none());
}

#[test]
fn live_link_attributes_written_as_text_are_live_links_too() {
    let link = html! { <a navigate="/about">About</a><a patch="/items?page=2">Next</a> };

    assert_eq!(
        link.to_html(),
        concat!(
            r#"<a href="/about" data-phx-link="redirect" data-phx-link-state="push">About</a>"#,
            r#"<a href="/items?page=2" data-phx-link="patch" data-phx-link-state="push">Next</a>"#,
        )
    );
}

#[test]
fn a_live_link_to_anything_but_a_local_path_writes_no_link() {
    for bad in [
        "javascript:alert(1)",
        "//evil.example",
        "https://evil.example/",
    ] {
        let dynamic = html! { <a navigate={bad}>x</a><a patch={bad}>y</a> };
        assert_eq!(dynamic.to_html(), "<a>x</a><a>y</a>", "{bad}");
    }
}

// ---- a lookup that something else answers ----

/// Everything this binary logs, each event as its level and fields, with the thread it
/// was logged on: a test's server runs on the test's own thread.
struct Logs;

static LOGGED: Mutex<Vec<(ThreadId, String)>> = Mutex::new(Vec::new());

impl Logs {
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
        LOGGED
            .lock()
            .unwrap()
            .push((thread::current().id(), fields.0));
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[test]
fn a_refused_live_link_target_is_logged_quoted_and_cut_short() {
    Logs::collect();
    let long = format!("javascript:{}", "x".repeat(200));
    let forged = "//evil.example\nWARN forged line";

    let _ = html! { <a navigate={forged}>x</a><a patch={long.as_str()}>y</a> }.to_html();

    let logged = Logs::of_this_test();
    let [first, second] = logged.as_slice() else {
        panic!("not logged twice: {logged:?}");
    };
    assert!(
        first.contains(r#"target="//evil.example\nWARN forged line""#),
        "{first}"
    );
    assert!(!first.contains('\n'), "{first:?}");
    assert!(second.len() < 200, "{second}");
    assert!(second.contains(&"x".repeat(80)), "{second}");
}

/// A Layer that refuses `TRACE`, as a hardening guide may tell an application to.
async fn refuse_trace(request: Request<Body>, next: Next) -> axum::response::Response {
    if request.method() == Method::TRACE {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    next.run(request).await
}

#[tokio::test]
async fn a_join_whose_lookup_is_refused_by_a_layer_is_logged_with_the_path_alone() {
    Logs::collect();
    let pages = Router::<SigningKey>::new()
        .route("/pages/{n}", live::<Pager, _>())
        .layer(from_fn(refuse_trace));
    let checks = SocketChecks::new().without_csrf_check();
    let app: Router = pages
        .clone()
        .route("/live/websocket", live_socket_with(pages, checks))
        .with_state(SigningKey::new(SECRET).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn({
        let app = app.clone();
        async move { axum::serve(listener, app).await.unwrap() }
    });
    let response = app
        .oneshot(
            Request::get("/pages/1?q=hunter2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    };
    let topic = format!("lv:{}", attribute("id"));
    let (stream, _) = connect_async(format!("ws://{address}/live/websocket?vsn=2.0.0"))
        .await
        .unwrap();
    let mut client = Client(stream);
    let join = json!(["4", "4", topic, "phx_join", {
        "url": "http://localhost/pages/1?q=hunter2", "params": {},
        "session": attribute("data-phx-session"), "static": attribute("data-phx-static"),
    }]);

    client.send(join).await;

    // The browser is told to load the page again, and the log says why.
    assert_eq!(
        client.receive().await[4]["response"]["reason"],
        "unauthorized"
    );
    let logged = Logs::of_this_test();
    let warnings: Vec<_> = logged
        .iter()
        .filter(|line| line.starts_with("WARN "))
        .collect();
    let [warning] = warnings.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    assert!(warning.contains(r#"path="/pages/1""#), "{warning}");
    assert!(warning.contains("405"), "{warning}");
    assert!(!warning.contains("hunter2"), "{warning}");
}
