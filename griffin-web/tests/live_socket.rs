//! The Connected render and Events of a LiveView, through real socket frames: the app
//! is served on a local port, a WebSocket client sends the frames the Phoenix client
//! would, and the tests assert on the frames that come back.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{SinkExt as _, StreamExt as _};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::Request;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{
    Ended, Event, EventError, Handle, LiveView, Payload, Socket, SocketChecks, live,
    live_socket_with,
};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::{self, Write as _};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex, Once};
use std::thread::{self, ThreadId};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt as _;

const SECRET: &str = "a test secret, at least thirty-two bytes long";

struct Counter {
    count: i32,
    connected: bool,
}

enum CounterEvent {
    Inc,
    Add { by: i32 },
    Nothing,
}

/// By hand, which is what `#[derive(Event)]` writes.
impl Event for CounterEvent {
    fn decode(name: &str, payload: &Payload<'_>) -> Result<CounterEvent, EventError> {
        match name {
            "inc" => Ok(CounterEvent::Inc),
            "add" => Ok(CounterEvent::Add {
                by: payload.field("by")?,
            }),
            "nothing" => Ok(CounterEvent::Nothing),
            _ => Err(EventError::Unknown),
        }
    }

    fn encode(&self) -> (&'static str, Vec<(&'static str, String)>) {
        match self {
            CounterEvent::Inc => ("inc", vec![]),
            CounterEvent::Add { by } => ("add", vec![("by", by.to_string())]),
            CounterEvent::Nothing => ("nothing", vec![]),
        }
    }
}

impl LiveView for Counter {
    type Params = i32;
    type Event = CounterEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(start: i32, socket: &mut Socket) -> Counter {
        Counter {
            count: start,
            connected: socket.connected(),
        }
    }

    async fn handle_event(
        &mut self,
        event: CounterEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            CounterEvent::Inc => self.count += 1,
            CounterEvent::Add { by } => self.count += by,
            CounterEvent::Nothing => {}
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <p data-connected={@connected.to_string()}>Count: {@count}</p> }
    }
}

struct Hello;

impl LiveView for Hello {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> Hello {
        Hello
    }

    fn render(&self) -> Rendered {
        html! { <h1>Hello</h1> }
    }
}

/// A LiveView whose page shows the last Event it handled, as the handler got it.
struct Echo {
    last: String,
}

#[derive(Event, Debug)]
enum EchoEvent {
    Rename {
        id: u32,
        title: String,
        done: bool,
        note: Option<String>,
    },
    ClearDone,
}

impl LiveView for Echo {
    type Params = ();
    type Event = EchoEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> Echo {
        Echo {
            last: String::new(),
        }
    }

    async fn handle_event(
        &mut self,
        event: EchoEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        self.last = format!("{event:?}");
        Ok(())
    }

    fn render(&self) -> Rendered {
        let rename = EchoEvent::Rename {
            id: 7,
            title: "Milk".to_owned(),
            done: true,
            note: None,
        };
        html! {
            <p>
                {@last}
                <button id="typed" phx-click={rename}>Rename</button>
                <button id="named" phx-click="clear_done">Clear</button>
            </p>
        }
    }
}

/// A LiveView that tells a listening test when its state is dropped, which is when the
/// task that owns it has ended, and when it begins to handle an Event. The route's
/// parameter names the test.
struct Watched {
    test: String,
}

static LISTENERS: LazyLock<Mutex<HashMap<String, mpsc::UnboundedSender<()>>>> =
    LazyLock::new(Mutex::default);

impl Drop for Watched {
    fn drop(&mut self) {
        if let Some(listener) = LISTENERS.lock().unwrap().get(&self.test) {
            let _ = listener.send(());
        }
    }
}

/// The one Event of `Watched`.
struct Hang;

impl Event for Hang {
    fn decode(name: &str, _payload: &Payload<'_>) -> Result<Hang, EventError> {
        if name == "hang" {
            Ok(Hang)
        } else {
            Err(EventError::Unknown)
        }
    }

    fn encode(&self) -> (&'static str, Vec<(&'static str, String)>) {
        ("hang", vec![])
    }
}

impl LiveView for Watched {
    type Params = String;
    type Event = Hang;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(test: String, _socket: &mut Socket) -> Watched {
        Watched { test }
    }

    /// Its Event is one whose handling never finishes.
    async fn handle_event(
        &mut self,
        _event: Hang,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        let _ = LISTENERS.lock().unwrap()[&self.test].send(());
        std::future::pending().await
    }

    fn render(&self) -> Rendered {
        html! { <p>Watched</p> }
    }
}

/// The LiveView of the conformance case `pushed_events_and_reply`: its template, and a
/// handler that does what a step of the case says.
struct Tally {
    count: String,
}

/// One step of the case, as the test sends it.
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

impl LiveView for Tally {
    type Params = String;
    type Event = Step;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(count: String, socket: &mut Socket) -> Tally {
        // The case's first step. In the Dead render there is no client to push to.
        socket.push_event("ready", json!({"at": "mount"}));
        Tally { count }
    }

    async fn handle_event(
        &mut self,
        Step(step): Step,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        if let Some(count) = step["count"].as_str() {
            self.count = count.to_owned();
        }
        for event in step["push_events"].as_array().into_iter().flatten() {
            socket.push_event(event[0].as_str().unwrap(), event[1].clone());
        }
        if !step["reply"].is_null() {
            socket.reply(step["reply"].clone());
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <p>{@count}</p> }
    }
}

/// A socket on which the `Watched` LiveView of `test` is joined with join ref `"4"`,
/// its topic, and where what it tells is heard.
async fn joined_watched(test: &str) -> (Client, String, mpsc::UnboundedReceiver<()>) {
    let mut dropped = listen(test);
    let mut client = Client::connect(serve().await).await;
    let page = Page::get(&format!("/watched/{test}")).await;
    // The Dead render's state went with the response.
    dropped.try_recv().expect("the Dead render kept its state");
    client.send(page.join("4")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");
    // The connected LiveView's state is alive.
    dropped.try_recv().expect_err("the state did not last");
    (client, page.topic, dropped)
}

/// Waits for the next thing `Watched` tells: its state was dropped, or its handler
/// began. Hearing nothing fails the test instead of hanging it.
async fn heard(listener: &mut mpsc::UnboundedReceiver<()>) {
    let next = tokio::time::timeout(Duration::from_secs(5), listener.recv());
    next.await.expect("nothing heard from the LiveView");
}

/// A LiveView whose page shows what it has handled, in order. Each mount leaves a
/// [`Handle`] to it where its test finds it. The route's parameter names the test.
struct Journal {
    test: String,
    handled: Vec<String>,
}

static HANDLES: LazyLock<Mutex<HashMap<String, Handle<Journal>>>> = LazyLock::new(Mutex::default);

/// For each test, what holds its `Journal` in the handler of `Hold` until the test
/// opens it.
static GATES: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> = LazyLock::new(Mutex::default);

fn gate(test: &str) -> Arc<Notify> {
    let mut gates = GATES.lock().unwrap();
    gates.entry(test.to_owned()).or_default().clone()
}

/// Where what the LiveView of `test` tells is heard. See [`heard`].
fn listen(test: &str) -> mpsc::UnboundedReceiver<()> {
    let (listener, told) = mpsc::unbounded_channel();
    LISTENERS.lock().unwrap().insert(test.to_owned(), listener);
    told
}

/// Tells the test that listens under `name`, if one does.
fn tell(name: &str) {
    if let Some(listener) = LISTENERS.lock().unwrap().get(name) {
        let _ = listener.send(());
    }
}

#[derive(Event)]
enum JournalEvent {
    Note {
        text: String,
    },
    /// Handling it tells the test that it began, and does not finish until the test
    /// opens the gate.
    Hold,
    /// Its handler returns an error.
    Fail,
    /// Its handler panics.
    Panic,
    /// Its handler starts a task that reports back with a Message.
    Work,
    /// Its handler starts a task that panics.
    WorkBadly,
}

enum JournalMessage {
    Note(&'static str),
    /// Its handler changes nothing.
    Nothing,
    /// Its handler pushes an event to the browser, and changes nothing on the page.
    Push,
    /// Its handler returns an error.
    Fail,
    /// Its handler panics.
    Panic,
}

/// Tells the test when the state is dropped: the task that owned it has ended.
impl Drop for Journal {
    fn drop(&mut self) {
        tell(&self.test);
    }
}

/// What a task started by a `Journal` holds for as long as it exists. Dropping it
/// tells the test that listens under its name: that task is gone.
struct Running(String);

impl Drop for Running {
    fn drop(&mut self) {
        tell(&self.0);
    }
}

/// The failure of a Journal's handlers.
#[derive(Debug)]
struct LedgerClosed;

impl std::fmt::Display for LedgerClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the ledger is closed")
    }
}

impl std::error::Error for LedgerClosed {}

impl LiveView for Journal {
    type Params = String;
    type Event = JournalEvent;
    type Message = JournalMessage;
    type Error = LedgerClosed;

    async fn mount(test: String, socket: &mut Socket) -> Journal {
        assert!(
            !(socket.connected() && test == "mount-panics"),
            "this test's Connected mount panics"
        );
        let handle = socket.handle::<Journal>();
        HANDLES.lock().unwrap().insert(test.clone(), handle);
        if socket.connected() {
            // Two tasks that never finish by themselves.
            for _ in 0..2 {
                let running = Running(format!("{test}/task"));
                socket.spawn(async move {
                    let _running = running;
                    std::future::pending().await
                });
            }
        }
        Journal {
            test,
            handled: vec![],
        }
    }

    async fn handle_event(
        &mut self,
        event: JournalEvent,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            JournalEvent::Note { text } => self.handled.push(text),
            JournalEvent::Hold => {
                tell(&self.test);
                gate(&self.test).notified().await;
                self.handled.push("held".to_owned());
            }
            JournalEvent::Fail => return Err(LedgerClosed),
            JournalEvent::Panic => panic!("the password was {}", "hunter2"),
            JournalEvent::Work => {
                let journal = socket.handle::<Journal>();
                socket.spawn(async move {
                    let _ = journal.send(JournalMessage::Note("worked"));
                });
            }
            JournalEvent::WorkBadly => {
                socket.spawn(async { panic!("the password was {}", "hunter2") });
            }
        }
        Ok(())
    }

    async fn handle_message(
        &mut self,
        message: JournalMessage,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match message {
            JournalMessage::Note(note) => self.handled.push(note.to_owned()),
            JournalMessage::Nothing => {}
            JournalMessage::Push => {
                socket.push_event("saved", json!({"by": "a Message"}));
                // There is no Event to reply to: this goes nowhere.
                socket.reply(json!({"lost": true}));
            }
            JournalMessage::Fail => return Err(LedgerClosed),
            JournalMessage::Panic => panic!("the password was {}", "hunter2"),
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! { <p>{@handled.join(" ")}</p> }
    }
}

/// The `Journal` of a test, joined with join ref `"4"`.
struct Joined {
    client: Client,
    page: Page,
    /// The handle the Connected mount left.
    journal: Handle<Journal>,
    /// Where what the connected `Journal` tells is heard: its handler of `Hold` began,
    /// or its state was dropped.
    told: mpsc::UnboundedReceiver<()>,
}

async fn joined_journal(test: &str) -> Joined {
    let client = Client::connect(serve().await).await;
    join_journal(test, client).await
}

/// Joins the `Journal` of `test` on a socket.
async fn join_journal(test: &str, mut client: Client) -> Joined {
    let mut told = listen(test);
    let page = Page::get(&format!("/journal/{test}")).await;
    // The Dead render's state went with the response.
    told.try_recv().expect("the Dead render kept its state");
    client.send(page.join("4")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");
    let journal = HANDLES.lock().unwrap().remove(test);
    Joined {
        client,
        page,
        journal: journal.expect("mount left no handle"),
        told,
    }
}

fn app() -> Router {
    let pages = Router::new()
        .route("/counter/{start}", live::<Counter, _>())
        .route("/hello", live::<Hello, _>())
        .route("/echo", live::<Echo, _>())
        .route("/tally/{count}", live::<Tally, _>())
        .route("/watched/{test}", live::<Watched, _>())
        .route("/journal/{test}", live::<Journal, _>());
    // These pages have no session, so there is no CSRF token for the socket to ask
    // for. `tests/security.rs` has the socket with its checks on.
    let checks = SocketChecks::new().without_csrf_check();
    pages
        .clone()
        .route("/live/websocket", live_socket_with(pages, checks))
        .with_state(SigningKey::new(SECRET).unwrap())
}

/// Serves the app on a local port, as a browser would reach it.
async fn serve() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app()).await.unwrap() });
    address
}

/// The socket as the Phoenix client opens it. Frames are JSON text.
struct Client(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Client {
    async fn connect(address: SocketAddr) -> Client {
        let url = format!("ws://{address}/live/websocket?vsn=2.0.0");
        Client(connect_async(url).await.unwrap().0)
    }

    async fn send(&mut self, frame: Value) {
        let text = frame.to_string();
        self.0.send(Message::Text(text.into())).await.unwrap();
    }

    /// The next frame from the server. A frame that never comes fails the test
    /// instead of hanging it.
    async fn receive(&mut self) -> Value {
        let next = tokio::time::timeout(Duration::from_secs(5), self.0.next());
        let frame = next.await.expect("no frame from the server");
        let text = frame.expect("the server closed the socket").unwrap();
        serde_json::from_str(text.to_text().unwrap()).unwrap()
    }
}

/// The HTML a GET of `path` returns.
async fn dead_render(path: &str) -> String {
    let request = Request::get(path).body(Body::empty()).unwrap();
    let response = app().oneshot(request).await.unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

/// What the client reads from the container of a Dead render before it joins.
struct Page {
    url: String,
    topic: String,
    session: String,
    static_token: String,
}

impl Page {
    /// The Dead render of `path`, as a GET returns it.
    async fn get(path: &str) -> Page {
        let html = dead_render(path).await;
        let attribute = |name: &str| {
            let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
            html[start..start + html[start..].find('"').unwrap()].to_owned()
        };
        Page {
            url: format!("http://localhost{path}"),
            topic: format!("lv:{}", attribute("id")),
            session: attribute("data-phx-session"),
            static_token: attribute("data-phx-static"),
        }
    }

    /// The `phx_join` frame the client sends for this page.
    fn join(&self, join_ref: &str) -> Value {
        json!([join_ref, join_ref, self.topic, "phx_join", {
            "url": self.url,
            "params": {"_mounts": 0, "_mount_attempts": 0},
            "session": self.session,
            "static": self.static_token,
            "sticky": false,
        }])
    }
}

#[tokio::test]
async fn joining_with_the_tokens_of_a_dead_render_replies_with_the_connected_render() {
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/counter/5").await;

    client.send(page.join("4")).await;

    assert_eq!(
        client.receive().await,
        json!(["4", "4", page.topic, "phx_reply", {"status": "ok", "response": {
            // Mount ran again with the route's parameter, this time connected.
            "rendered": {
                "0": " data-connected=\"true\"",
                "1": "5",
                "s": 0,
                "p": {"0": ["<p", ">Count: ", "</p>"]},
                "r": 1,
            },
            // The version of the client this server speaks to, which the client checks.
            "liveview_version": "1.2.12",
        }}])
    );
}

/// The reply to `page`'s join on a fresh socket, without the envelope: `(status, response)`.
async fn join(page: &Page) -> (String, Value) {
    let mut client = Client::connect(serve().await).await;
    client.send(page.join("1")).await;
    let mut reply = client.receive().await;
    let payload = reply[4].take();
    assert_eq!(reply, json!(["1", "1", page.topic, "phx_reply", null]));
    (
        payload["status"].as_str().unwrap().to_owned(),
        payload["response"].clone(),
    )
}

/// The reply that makes the client give up on the join and load the page again.
fn refused(reason: &str) -> (String, Value) {
    ("error".to_owned(), json!({"reason": reason}))
}

/// A token in the format `griffin_web::token` documents, signed here so that a test
/// chooses the secret and the age.
fn sign(secret: &str, purpose: &str, age: Duration, data: Value) -> String {
    let issued_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - age;
    let envelope = json!({
        "purpose": purpose,
        "issued_at": issued_at.as_millis() as u64,
        "data": data,
    });
    let payload = URL_SAFE_NO_PAD.encode(envelope.to_string());
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{payload}.{signature}")
}

/// A page of the counter whose tokens were signed with `secret`, `age` ago.
fn counter_page_signed(secret: &str, age: Duration) -> Page {
    let view = std::any::type_name::<Counter>();
    Page {
        url: "http://localhost/counter/5".to_owned(),
        topic: "lv:phx-signed-by-the-test".to_owned(),
        session: sign(
            secret,
            "live session",
            age,
            json!({"id": "phx-signed-by-the-test", "view": view}),
        ),
        static_token: sign(
            secret,
            "live static",
            age,
            json!({"id": "phx-signed-by-the-test"}),
        ),
    }
}

/// The token with one character of the part before (`0`) or after (`1`) the dot changed.
fn changed(token: &str, part: usize) -> String {
    let at = if part == 0 { 0 } else { token.len() - 1 };
    let other = if token.as_bytes()[at] == b'A' {
        "B"
    } else {
        "A"
    };
    format!("{}{other}{}", &token[..at], &token[at + 1..])
}

const TWO_WEEKS: Duration = Duration::from_secs(14 * 24 * 60 * 60);
const MINUTE: Duration = Duration::from_secs(60);

#[tokio::test]
async fn a_join_with_a_token_this_server_did_not_sign_is_refused() {
    // Tokens made the way these tests make them are accepted when the secret is right,
    let (status, _) = join(&counter_page_signed(SECRET, MINUTE)).await;
    assert_eq!(status, "ok");
    // so it is the other secret that gets this join refused.
    let forged = counter_page_signed("another secret, also thirty-two bytes long", MINUTE);
    assert_eq!(join(&forged).await, refused("stale"));

    let genuine = Page::get("/counter/5").await;
    for session in [
        changed(&genuine.session, 0),
        changed(&genuine.session, 1),
        forged.session,
        String::new(),
    ] {
        let page = Page {
            session,
            topic: genuine.topic.clone(),
            static_token: genuine.static_token.clone(),
            url: genuine.url.clone(),
        };
        assert_eq!(join(&page).await, refused("stale"), "{}", page.session);
    }
    for static_token in [
        changed(&genuine.static_token, 0),
        changed(&genuine.static_token, 1),
        forged.static_token,
        String::new(),
    ] {
        let page = Page {
            static_token,
            topic: genuine.topic.clone(),
            session: genuine.session.clone(),
            url: genuine.url.clone(),
        };
        assert_eq!(join(&page).await, refused("stale"), "{}", page.static_token);
    }
}

#[tokio::test]
async fn a_join_without_tokens_is_refused() {
    let page = Page::get("/counter/5").await;
    let mut client = Client::connect(serve().await).await;

    client
        .send(json!(["1", "1", page.topic, "phx_join", {"url": page.url}]))
        .await;

    assert_eq!(
        client.receive().await,
        json!(["1", "1", page.topic, "phx_reply", {"status": "error", "response": {"reason": "stale"}}])
    );
}

#[tokio::test]
async fn a_join_with_an_expired_token_is_refused() {
    let (status, _) = join(&counter_page_signed(SECRET, TWO_WEEKS - MINUTE)).await;
    assert_eq!(status, "ok");

    let fresh = counter_page_signed(SECRET, MINUTE);
    let expired = counter_page_signed(SECRET, TWO_WEEKS + MINUTE);
    let expired_session = Page {
        session: expired.session.clone(),
        ..counter_page_signed(SECRET, MINUTE)
    };
    let expired_static = Page {
        static_token: expired.static_token.clone(),
        ..fresh
    };

    assert_eq!(join(&expired).await, refused("stale"));
    assert_eq!(join(&expired_session).await, refused("stale"));
    assert_eq!(join(&expired_static).await, refused("stale"));
}

#[tokio::test]
async fn a_join_with_a_token_signed_for_something_else_is_refused() {
    let genuine = Page::get("/counter/5").await;
    let swapped = Page {
        session: genuine.static_token.clone(),
        static_token: genuine.session.clone(),
        ..genuine
    };

    assert_eq!(join(&swapped).await, refused("stale"));
}

#[tokio::test]
async fn a_join_must_be_for_the_page_its_tokens_came_from() {
    let other = Page::get("/counter/5").await;

    let topic_of_another_page = Page {
        topic: other.topic.clone(),
        ..Page::get("/counter/5").await
    };
    assert_eq!(join(&topic_of_another_page).await, refused("stale"));

    let static_token_of_another_page = Page {
        static_token: other.static_token.clone(),
        ..Page::get("/counter/5").await
    };
    assert_eq!(join(&static_token_of_another_page).await, refused("stale"));

    // The tokens of one LiveView do not open another,
    let url_of_another_live_view = Page {
        url: "http://localhost/counter/5".to_owned(),
        ..Page::get("/hello").await
    };
    assert_eq!(
        join(&url_of_another_live_view).await,
        refused("unauthorized")
    );

    // nor a URL that is no LiveView's.
    for url in [
        "http://localhost/nowhere",
        "http://localhost/counter/many",
        "",
    ] {
        let no_live_view = Page {
            url: url.to_owned(),
            ..Page::get("/counter/5").await
        };
        assert_eq!(join(&no_live_view).await, refused("unauthorized"), "{url}");
    }
}

/// A socket on which the counter at `/counter/5` was joined with join ref `"4"`, and
/// that LiveView's topic.
async fn joined_counter() -> (Client, String) {
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/counter/5").await;
    client.send(page.join("4")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");
    (client, page.topic)
}

/// The reply to the message `message_ref` that LiveView was sent.
fn reply(topic: &str, message_ref: &str, response: Value) -> Value {
    json!(["4", message_ref, topic, "phx_reply", {"status": "ok", "response": response}])
}

#[tokio::test]
async fn an_event_is_answered_with_a_diff_of_only_the_slots_that_changed() {
    let (mut client, topic) = joined_counter().await;

    let click = json!({"type": "click", "event": "inc", "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;

    // The count is slot 1. Slot 0 and the statics did not change and are not sent.
    assert_eq!(
        client.receive().await,
        reply(&topic, "5", json!({"diff": {"1": "6"}}))
    );
}

#[tokio::test]
async fn an_event_reaches_the_handler_with_its_value() {
    let (mut client, topic) = joined_counter().await;

    // What the client sends for a click on `<button phx-click="add" phx-value-by="10">`:
    // each `phx-value-*` attribute as text, and the button's own empty `value`.
    let click = json!({"type": "click", "event": "add", "value": {"by": "10", "value": ""}});
    client.send(json!(["4", "5", topic, "event", click])).await;

    assert_eq!(
        client.receive().await,
        reply(&topic, "5", json!({"diff": {"1": "15"}}))
    );
}

#[tokio::test]
async fn an_event_bound_in_a_template_comes_back_to_the_handler_as_the_typed_value() {
    let mut client = Client::connect(serve().await).await;
    // What the binding wrote, for the client to read.
    let typed = r#"<button id="typed" phx-click="rename" phx-value-id="7" phx-value-title="Milk" phx-value-done="true">"#;
    let html = dead_render("/echo").await;
    assert!(html.contains(typed), "{html}");
    let page = Page::get("/echo").await;
    client.send(page.join("4")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");

    // What the client sends for a click on that button: the name, each `phx-value-*`
    // attribute as text (`extractMeta` in the client), and the button's empty `value`.
    let value = json!({"id": "7", "title": "Milk", "done": "true", "value": ""});
    let click = json!({"type": "click", "event": "rename", "value": value, "cid": null});
    client
        .send(json!(["4", "5", page.topic, "event", click]))
        .await;

    // The handler got numbers and booleans, not text, and `None` for what was left out.
    let handled = "Rename { id: 7, title: &quot;Milk&quot;, done: true, note: None }";
    assert_eq!(
        client.receive().await,
        reply(&page.topic, "5", json!({"diff": {"0": handled}}))
    );
}

#[tokio::test]
async fn an_event_bound_by_name_reaches_the_same_handler() {
    let mut client = Client::connect(serve().await).await;
    let named = r#"<button id="named" phx-click="clear_done">"#;
    let html = dead_render("/echo").await;
    assert!(html.contains(named), "{html}");
    let page = Page::get("/echo").await;
    client.send(page.join("4")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");

    let click = json!({"type": "click", "event": "clear_done", "value": {"value": ""}});
    client
        .send(json!(["4", "5", page.topic, "event", click]))
        .await;

    assert_eq!(
        client.receive().await,
        reply(&page.topic, "5", json!({"diff": {"0": "ClearDone"}}))
    );
}

#[tokio::test]
async fn an_event_that_changes_nothing_is_answered_without_a_diff() {
    let (mut client, topic) = joined_counter().await;

    let click = json!({"type": "click", "event": "nothing", "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;

    // As Phoenix answers when the diff is empty.
    assert_eq!(client.receive().await, reply(&topic, "5", json!({})));
}

#[tokio::test]
async fn events_sent_back_to_back_are_handled_in_order_each_with_its_own_render() {
    let (mut client, topic) = joined_counter().await;
    let event = |name: &str, value: Value| json!({"type": "click", "event": name, "value": value});

    // All three are on their way before the first answer is read.
    for (message_ref, name, value) in [
        ("5", "inc", json!({})),
        ("6", "add", json!({"by": 10})),
        ("7", "inc", json!({})),
    ] {
        let payload = event(name, value);
        client
            .send(json!(["4", message_ref, topic, "event", payload]))
            .await;
    }

    for (message_ref, count) in [("5", "6"), ("6", "16"), ("7", "17")] {
        assert_eq!(
            client.receive().await,
            reply(&topic, message_ref, json!({"diff": {"1": count}}))
        );
    }
}

/// The conformance case `pushed_events_and_reply`, or what Phoenix emitted for it. See
/// `tests/conformance/README.md`.
fn conformance(dir: &str) -> Value {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/tests/conformance/{dir}/pushed_events_and_reply.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn pushed_events_and_a_reply_reach_the_client_in_the_diff_as_phoenix_sends_them() {
    let (case, phoenix) = (conformance("cases"), conformance("fixtures"));
    let steps = case["steps"].as_array().unwrap();
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/tally/0").await;
    client.send(page.join("4")).await;

    // An event pushed at mount goes with the Connected render (`"e"`).
    let joined = client.receive().await;
    assert_eq!(joined[4]["response"]["rendered"], phoenix[0]);

    // Each later step is an Event whose handler pushes the step's events and replies
    // with its reply (`"r"`). Neither is sent again with the next diff.
    for (number, step) in steps.iter().enumerate().skip(1) {
        let message_ref = number.to_string();
        let event = json!({"type": "hook", "event": "step", "value": step});
        let frame = json!(["4", message_ref, page.topic, "event", event]);
        client.send(frame).await;

        assert_eq!(
            client.receive().await,
            reply(&page.topic, &message_ref, json!({"diff": phoenix[number]})),
            "step {number}"
        );
    }
}

#[tokio::test]
async fn an_event_pushed_in_the_dead_render_goes_nowhere() {
    let html = dead_render("/tally/0").await;

    assert!(html.contains("<p>0</p>"), "{html}");
    assert!(!html.contains("ready"), "{html}");
}

#[tokio::test]
async fn leaving_is_answered_and_closes_the_topic() {
    let (mut client, topic) = joined_counter().await;

    client.send(json!(["4", "5", topic, "phx_leave", {}])).await;

    assert_eq!(client.receive().await, reply(&topic, "5", json!({})));
    assert_eq!(
        client.receive().await,
        json!(["4", "4", topic, "phx_close", {}])
    );
    // The LiveView is gone: its topic no longer takes events.
    let click = json!({"type": "click", "event": "inc", "value": {}});
    client.send(json!(["4", "6", topic, "event", click])).await;
    assert_eq!(
        client.receive().await,
        json!(["4", "6", topic, "phx_reply", {"status": "error", "response": {"reason": "unmatched topic"}}])
    );
}

#[tokio::test]
async fn leaving_ends_the_live_views_task() {
    let (mut client, topic, mut dropped) = joined_watched("leaving").await;

    client.send(json!(["4", "5", topic, "phx_leave", {}])).await;

    heard(&mut dropped).await;
}

#[tokio::test]
async fn a_dropped_connection_ends_the_live_views_task() {
    let (client, _topic, mut dropped) = joined_watched("dropped-connection").await;

    // The browser goes away without a leave or a close frame.
    drop(client);

    heard(&mut dropped).await;
}

#[tokio::test]
async fn a_heartbeat_is_answered_while_an_event_is_being_handled() {
    let (mut client, topic, mut began) = joined_watched("busy").await;
    let click = json!({"type": "click", "event": "hang", "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;
    heard(&mut began).await;

    client
        .send(json!([null, "6", "phoenix", "heartbeat", {}]))
        .await;

    assert_eq!(
        client.receive().await,
        json!([null, "6", "phoenix", "phx_reply", {"status": "ok", "response": {}}])
    );
}

#[tokio::test]
async fn a_dropped_connection_cancels_an_event_that_is_being_handled() {
    let (mut client, topic, mut began_or_dropped) = joined_watched("cancelled").await;
    let click = json!({"type": "click", "event": "hang", "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;
    heard(&mut began_or_dropped).await; // The handler began. It will not finish.

    drop(client);

    heard(&mut began_or_dropped).await; // The state was dropped.
}

#[tokio::test]
async fn a_message_a_live_view_does_not_understand_is_answered_with_an_error() {
    let (mut client, topic) = joined_counter().await;
    let error = |message_ref: &str| json!(["4", message_ref, topic, "phx_reply", {"status": "error", "response": {}}]);

    client.send(json!(["4", "5", topic, "no_such", {}])).await;
    assert_eq!(client.receive().await, error("5"));

    // The LiveView is still there, its state untouched.
    let click = json!({"type": "click", "event": "inc", "value": {}});
    client.send(json!(["4", "7", topic, "event", click])).await;
    assert_eq!(
        client.receive().await,
        reply(&topic, "7", json!({"diff": {"1": "6"}}))
    );
}

#[tokio::test]
async fn a_message_sent_through_a_handle_from_mount_updates_the_page_with_a_pushed_diff() {
    let Joined {
        mut client,
        page,
        journal,
        ..
    } = joined_journal("message").await;

    // From another task than the LiveView's.
    let sent = tokio::spawn(async move { journal.send(JournalMessage::Note("tick")) });
    sent.await.unwrap().unwrap();

    // No Event asked for this diff, so it is not a reply and has no ref: Phoenix's
    // `diff` message (`push_diff` in `channel.ex`).
    assert_eq!(
        client.receive().await,
        json!(["4", null, page.topic, "diff", {"0": "tick"}])
    );
}

#[tokio::test]
async fn a_message_that_changes_nothing_pushes_nothing() {
    let Joined {
        mut client,
        page: Page { topic, .. },
        journal,
        ..
    } = joined_journal("message-changes-nothing").await;

    journal.send(JournalMessage::Nothing).unwrap();
    journal.send(JournalMessage::Note("after")).unwrap();

    // The first frame is that of the second Message: as Phoenix, which pushes no
    // empty diff.
    assert_eq!(
        client.receive().await,
        json!(["4", null, topic, "diff", {"0": "after"}])
    );
}

#[tokio::test]
async fn an_event_pushed_by_the_handler_of_a_message_goes_with_that_messages_diff() {
    let Joined {
        mut client,
        page: Page { topic, .. },
        journal,
        ..
    } = joined_journal("message-pushes").await;

    journal.send(JournalMessage::Push).unwrap();

    // The browser sent nothing in between. The page did not change, and the pushed
    // event alone makes a diff: under `e`, as with the answer to an Event.
    assert_eq!(
        client.receive().await,
        json!(["4", null, topic, "diff", {"e": [["saved", {"by": "a Message"}]]}])
    );
    // The reply the handler left did not wait for the next Event either: it is gone.
    let note = json!({"type": "click", "event": "note", "value": {"text": "next"}});
    client.send(json!(["4", "5", topic, "event", note])).await;
    assert_eq!(
        client.receive().await,
        reply(&topic, "5", json!({"diff": {"0": "next"}}))
    );
}

#[tokio::test]
async fn a_second_join_of_a_topic_ends_the_first_live_view_though_a_handle_to_it_remains() {
    let Joined {
        mut client,
        page,
        journal: first,
        told: mut dropped,
    } = joined_journal("joined-twice").await;

    client.send(page.join("6")).await;
    assert_eq!(client.receive().await[4]["status"], "ok");

    // The first one's state is gone, and its handle says so. It does not reach the
    // one that took its place,
    heard(&mut dropped).await;
    assert_eq!(first.send(JournalMessage::Note("late")), Err(Ended));
    // whose own handle does.
    let second = HANDLES.lock().unwrap().remove("joined-twice").unwrap();
    second.send(JournalMessage::Note("new")).unwrap();
    assert_eq!(
        client.receive().await,
        json!(["6", null, page.topic, "diff", {"0": "new"}])
    );
}

#[tokio::test]
async fn events_and_messages_are_handled_one_at_a_time_in_the_order_they_arrived() {
    let Joined {
        mut client,
        page: Page { topic, .. },
        journal,
        told: mut began,
    } = joined_journal("order").await;
    let note = |text: &str| json!({"type": "click", "event": "note", "value": {"text": text}});
    let heartbeat = json!([null, "0", "phoenix", "heartbeat", {}]);
    let beat = json!([null, "0", "phoenix", "phx_reply", {"status": "ok", "response": {}}]);

    // While the LiveView is held in the handler of one Event, a queue builds up
    // behind it: Messages and Events in turn.
    let hold = json!({"type": "click", "event": "hold", "value": {}});
    client.send(json!(["4", "5", topic, "event", hold])).await;
    heard(&mut began).await;
    journal.send(JournalMessage::Note("m1")).unwrap();
    client
        .send(json!(["4", "6", topic, "event", note("e2")]))
        .await;
    // The socket takes frames in order, so the Event is in the queue once a heartbeat
    // sent after it is answered. That answer is also the first frame since the handler
    // began: nothing behind it was handled meanwhile.
    client.send(heartbeat.clone()).await;
    assert_eq!(client.receive().await, beat);
    journal.send(JournalMessage::Note("m3")).unwrap();
    client
        .send(json!(["4", "7", topic, "event", note("e4")]))
        .await;
    client.send(heartbeat).await;
    assert_eq!(client.receive().await, beat);
    journal.send(JournalMessage::Note("m5")).unwrap();

    gate("order").notify_one();

    // Each in the order it arrived, each with a render of its own: an Event's is the
    // reply to it, a Message's is pushed.
    let replied = |to: &str, page: &str| reply(&topic, to, json!({"diff": {"0": page}}));
    let pushed = |page: &str| json!(["4", null, topic, "diff", {"0": page}]);
    assert_eq!(client.receive().await, replied("5", "held"));
    assert_eq!(client.receive().await, pushed("held m1"));
    assert_eq!(client.receive().await, replied("6", "held m1 e2"));
    assert_eq!(client.receive().await, pushed("held m1 e2 m3"));
    assert_eq!(client.receive().await, replied("7", "held m1 e2 m3 e4"));
    assert_eq!(client.receive().await, pushed("held m1 e2 m3 e4 m5"));
}

/// The answer to a frame for a topic nothing is joined on, under join ref `"4"`.
fn unmatched(topic: &str, message_ref: &str) -> Value {
    let response = json!({"status": "error", "response": {"reason": "unmatched topic"}});
    json!(["4", message_ref, topic, "phx_reply", response])
}

#[tokio::test]
async fn a_handler_that_returns_an_error_or_panics_ends_the_live_view_and_the_client_is_told() {
    // The handler of an Event, and that of a Message.
    for failing in ["fail", "panic", "message-fails", "message-panics"] {
        let Joined {
            mut client,
            page: Page { topic, .. },
            journal,
            told: mut dropped,
        } = joined_journal(&format!("ends-{failing}")).await;

        match failing {
            "message-fails" => journal.send(JournalMessage::Fail).unwrap(),
            "message-panics" => journal.send(JournalMessage::Panic).unwrap(),
            event => {
                let event = json!({"type": "click", "event": event, "value": {}});
                client.send(json!(["4", "5", topic, "event", event])).await;
            }
        }

        // No reply to the Event itself, as when a Phoenix channel crashes.
        assert_eq!(client.receive().await, crashed(&topic), "{failing}");
        // The state went with the task,
        heard(&mut dropped).await;
        // and the topic no longer takes Events.
        let note = json!({"type": "click", "event": "note", "value": {"text": "late"}});
        client.send(json!(["4", "6", topic, "event", note])).await;
        assert_eq!(client.receive().await, unmatched(&topic, "6"), "{failing}");
    }
}

#[tokio::test]
async fn a_panic_in_one_live_view_does_not_disturb_one_on_another_connection() {
    let address = serve().await;
    let click = json!({"type": "click", "event": "inc", "value": {}});
    // Another user's counter, on a connection of its own, with state of its own.
    let mut other = Client::connect(address).await;
    let counter = Page::get("/counter/5").await;
    other.send(counter.join("4")).await;
    assert_eq!(other.receive().await[4]["status"], "ok");
    other
        .send(json!(["4", "5", counter.topic, "event", click]))
        .await;
    assert_eq!(
        other.receive().await,
        reply(&counter.topic, "5", json!({"diff": {"1": "6"}}))
    );
    let Joined {
        mut client,
        page,
        told: mut dropped,
        ..
    } = join_journal("panic", Client::connect(address).await).await;

    let panic = json!({"type": "click", "event": "panic", "value": {}});
    client
        .send(json!(["4", "5", page.topic, "event", panic]))
        .await;

    // The client of the one that panicked is told what it recovers from, and the
    // state is gone.
    assert_eq!(client.receive().await, crashed(&page.topic));
    heard(&mut dropped).await;
    // Its socket still serves it: the rejoin mounts afresh.
    client.send(page.join("7")).await;
    let rejoined = client.receive().await;
    assert_eq!(rejoined[4]["status"], "ok");
    assert_eq!(rejoined[4]["response"]["rendered"]["0"], "");
    // The other user was told nothing, and the counter is where it was.
    other
        .send(json!(["4", "6", counter.topic, "event", click]))
        .await;
    assert_eq!(
        other.receive().await,
        reply(&counter.topic, "6", json!({"diff": {"1": "7"}}))
    );
}

#[tokio::test]
async fn a_panic_is_logged_with_the_live_view_and_without_its_message() {
    Logs::collect();
    let Joined {
        mut client,
        page: Page { topic, .. },
        ..
    } = joined_journal("panic-logged").await;

    let panic = json!({"type": "click", "event": "panic", "value": {}});
    client.send(json!(["4", "5", topic, "event", panic])).await;
    assert_eq!(client.receive().await, crashed(&topic));

    let logged = Logs::of_this_test();
    let [logged] = logged.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    assert!(logged.starts_with("ERROR "), "{logged}");
    assert!(logged.contains("live_socket::Journal"), "{logged}");
    assert!(logged.contains("panicked"), "{logged}");
    // The panic's message may hold what the state or the browser's payload held.
    assert!(!logged.contains("hunter2"), "{logged}");
}

#[tokio::test]
async fn a_mount_that_panics_at_the_join_ends_that_live_view_and_not_its_socket() {
    let mut client = Client::connect(serve().await).await;
    // The Dead render of this page mounts; its Connected render panics.
    let page = Page::get("/journal/mount-panics").await;

    client.send(page.join("4")).await;

    // The join is not answered: the channel crashed, and the client joins again.
    assert_eq!(client.receive().await, crashed(&page.topic));
    client
        .send(json!([null, "5", "phoenix", "heartbeat", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!([null, "5", "phoenix", "phx_reply", {"status": "ok", "response": {}}])
    );
}

#[tokio::test]
async fn the_tasks_a_live_view_started_are_cancelled_when_it_ends_and_none_remain() {
    for ending in [
        "leave",
        "connection-dropped",
        "joined-again",
        "fail",
        "panic",
    ] {
        let test = format!("tasks-{ending}");
        let mut tasks = listen(&format!("{test}/task"));
        let Joined {
            mut client,
            page,
            told: mut dropped,
            ..
        } = joined_journal(&test).await;
        let topic = &page.topic;
        // The two tasks its mount started are there while the LiveView is.
        tasks.try_recv().expect_err("a task did not last");

        match ending {
            "leave" => client.send(json!(["4", "5", topic, "phx_leave", {}])).await,
            "connection-dropped" => drop(client),
            // Another LiveView takes its place, with two tasks of its own.
            "joined-again" => client.send(page.join("6")).await,
            handler => {
                let event = json!({"type": "click", "event": handler, "value": {}});
                client.send(json!(["4", "5", topic, "event", event])).await;
            }
        }

        // The LiveView has ended, and both tasks with it: none of its tasks remains.
        heard(&mut dropped).await;
        heard(&mut tasks).await;
        heard(&mut tasks).await;
        tasks.try_recv().expect_err("more tasks ended than it had");
    }
}

#[tokio::test]
async fn a_task_started_by_a_handler_reports_back_with_a_message() {
    let Joined {
        mut client,
        page: Page { topic, .. },
        ..
    } = joined_journal("work").await;

    let work = json!({"type": "click", "event": "work", "value": {}});
    client.send(json!(["4", "5", topic, "event", work])).await;

    // The handler does not wait for the task: the Event is answered, with no change,
    assert_eq!(client.receive().await, reply(&topic, "5", json!({})));
    // and what the task found is pushed when it comes.
    assert_eq!(
        client.receive().await,
        json!(["4", null, topic, "diff", {"0": "worked"}])
    );
}

#[tokio::test]
async fn a_task_that_panics_ends_the_live_view_that_started_it() {
    Logs::collect();
    let Joined {
        mut client,
        page: Page { topic, .. },
        told: mut dropped,
        ..
    } = joined_journal("task-panics").await;

    let work = json!({"type": "click", "event": "work_badly", "value": {}});
    client.send(json!(["4", "5", topic, "event", work])).await;

    assert_eq!(client.receive().await, reply(&topic, "5", json!({})));
    // Not silently dropped: the page would wait for a result that never comes.
    assert_eq!(client.receive().await, crashed(&topic));
    heard(&mut dropped).await;
    let logged = Logs::of_this_test();
    let [logged] = logged.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    assert!(logged.starts_with("ERROR "), "{logged}");
    assert!(logged.contains("live_socket::Journal"), "{logged}");
    assert!(!logged.contains("hunter2"), "{logged}");
}

#[tokio::test]
async fn a_send_to_a_live_view_that_has_ended_is_reported_to_the_sender() {
    for ending in ["leave", "connection-dropped", "fail", "panic"] {
        let Joined {
            mut client,
            page: Page { topic, .. },
            journal,
            told: mut dropped,
        } = joined_journal(&format!("ended-{ending}")).await;
        // While the LiveView is there, it takes what is sent.
        assert_eq!(journal.send(JournalMessage::Note("in time")), Ok(()));

        match ending {
            "leave" => client.send(json!(["4", "5", topic, "phx_leave", {}])).await,
            "connection-dropped" => drop(client),
            handler => {
                let event = json!({"type": "click", "event": handler, "value": {}});
                client.send(json!(["4", "5", topic, "event", event])).await;
            }
        }
        heard(&mut dropped).await;

        let late = journal.send(JournalMessage::Note("late"));
        assert_eq!(late, Err(Ended), "{ending}");
    }
}

#[tokio::test]
async fn the_handle_of_a_dead_render_reports_that_nothing_takes_its_messages() {
    Page::get("/journal/dead-render").await;
    let journal = HANDLES.lock().unwrap().remove("dead-render").unwrap();

    assert_eq!(journal.send(JournalMessage::Note("lost")), Err(Ended));
}

#[tokio::test]
async fn joining_again_after_a_handler_error_mounts_clean_state() {
    let Joined {
        mut client, page, ..
    } = joined_journal("error-rejoin").await;
    let event = |name: &str| json!({"type": "click", "event": name, "value": {"text": name}});
    client
        .send(json!(["4", "5", page.topic, "event", event("note")]))
        .await;
    assert_eq!(
        client.receive().await,
        reply(&page.topic, "5", json!({"diff": {"0": "note"}}))
    );
    client
        .send(json!(["4", "6", page.topic, "event", event("fail")]))
        .await;
    assert_eq!(client.receive().await, crashed(&page.topic));

    // What the client does on `phx_error`: the same join, under a new ref.
    client.send(page.join("7")).await;

    let rejoined = client.receive().await;
    assert_eq!((&rejoined[0], &rejoined[1]), (&json!("7"), &json!("7")));
    // From mount: nothing of what the ended one had handled.
    assert_eq!(rejoined[4]["response"]["rendered"]["0"], "");
    client
        .send(json!(["7", "8", page.topic, "event", event("note")]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["7", "8", page.topic, "phx_reply", {"status": "ok", "response": {"diff": {"0": "note"}}}])
    );
}

#[tokio::test]
async fn a_handler_error_is_logged_with_the_live_view_and_the_error() {
    Logs::collect();
    let Joined {
        mut client,
        page: Page { topic, .. },
        ..
    } = joined_journal("error-logged-event").await;
    let fail = json!({"type": "click", "event": "fail", "value": {}});
    client.send(json!(["4", "5", topic, "event", fail])).await;
    assert_eq!(client.receive().await, crashed(&topic));

    let Joined {
        mut client,
        page: Page { topic, .. },
        journal,
        ..
    } = joined_journal("error-logged-message").await;
    journal.send(JournalMessage::Fail).unwrap();
    assert_eq!(client.receive().await, crashed(&topic));

    let logged = Logs::of_this_test();
    let [of_event, of_message] = logged.as_slice() else {
        panic!("not logged twice: {logged:?}");
    };
    for logged in [of_event, of_message] {
        assert!(logged.starts_with("ERROR "), "{logged}");
        assert!(logged.contains("live_socket::Journal"), "{logged}");
        assert!(logged.contains("the ledger is closed"), "{logged}");
    }
    assert!(of_event.contains(r#"event="fail""#), "{of_event}");
    assert!(!of_message.contains("event="), "{of_message}");
}

/// What Phoenix sends when a channel has crashed. The client joins again on it.
fn crashed(topic: &str) -> Value {
    json!(["4", "4", topic, "phx_error", {"reason": "channel_crash"}])
}

#[tokio::test]
async fn an_event_that_cannot_be_decoded_ends_the_live_view_and_the_client_is_told() {
    for (case, payload) in [
        (
            "no Event of that name",
            json!({"type": "click", "event": "nope", "value": {}}),
        ),
        (
            "a field is missing",
            json!({"type": "click", "event": "add", "value": {"value": ""}}),
        ),
        (
            "a field does not parse",
            json!({"type": "click", "event": "add", "value": {"by": "ten"}}),
        ),
        (
            "a field is not text",
            json!({"type": "click", "event": "add", "value": {"by": {"nested": [1]}}}),
        ),
        (
            "the value is not an object",
            json!({"type": "click", "event": "add", "value": "by=10"}),
        ),
        ("no name", json!({"type": "click", "value": {}})),
        ("not an object", json!([])),
        ("not an object", json!("event")),
    ] {
        let (mut client, topic) = joined_counter().await;

        client
            .send(json!(["4", "5", topic, "event", payload]))
            .await;

        // No reply to the Event itself, as when a Phoenix channel crashes.
        assert_eq!(client.receive().await, crashed(&topic), "{case}");
        // The LiveView is gone: its topic no longer takes Events.
        let click = json!({"type": "click", "event": "inc", "value": {}});
        client.send(json!(["4", "6", topic, "event", click])).await;
        assert_eq!(
            client.receive().await,
            json!(["4", "6", topic, "phx_reply", {"status": "error", "response": {"reason": "unmatched topic"}}]),
            "{case}"
        );
    }
}

#[tokio::test]
async fn an_event_that_cannot_be_decoded_drops_the_live_views_state() {
    let (mut client, topic, mut dropped) = joined_watched("undecodable").await;

    let click = json!({"type": "click", "event": "nope", "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;

    assert_eq!(client.receive().await, crashed(&topic));
    heard(&mut dropped).await;
}

#[tokio::test]
async fn joining_again_after_an_event_that_could_not_be_decoded_mounts_afresh() {
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/counter/5").await;
    client.send(page.join("4")).await;
    client.receive().await;
    let click = json!({"type": "click", "event": "inc", "value": {}});
    client
        .send(json!(["4", "5", page.topic, "event", click]))
        .await;
    client.receive().await;
    let undecodable = json!({"type": "click", "event": "nope", "value": {}});
    client
        .send(json!(["4", "6", page.topic, "event", undecodable]))
        .await;
    assert_eq!(client.receive().await, crashed(&page.topic));

    // What the client does on `phx_error`: the same join, under a new ref.
    client.send(page.join("7")).await;

    let rejoined = client.receive().await;
    assert_eq!((&rejoined[0], &rejoined[1]), (&json!("7"), &json!("7")));
    // From mount, not from where the ended one was.
    assert_eq!(rejoined[4]["response"]["rendered"]["1"], "5");
    client
        .send(json!(["7", "8", page.topic, "event", click]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["7", "8", page.topic, "phx_reply", {"status": "ok", "response": {"diff": {"1": "6"}}}])
    );
}

#[tokio::test]
async fn a_live_view_that_ended_does_not_take_the_others_on_its_socket_with_it() {
    let (mut client, topic) = joined_counter().await;
    let other = Page::get("/counter/40").await;
    let mut join = other.join("9");
    join[1] = json!("10");
    client.send(join).await;
    assert_eq!(client.receive().await[4]["status"], "ok");

    let undecodable = json!({"type": "click", "event": "nope", "value": {}});
    client
        .send(json!(["4", "5", topic, "event", undecodable]))
        .await;
    assert_eq!(client.receive().await, crashed(&topic));

    let click = json!({"type": "click", "event": "inc", "value": {}});
    client
        .send(json!(["9", "11", other.topic, "event", click]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["9", "11", other.topic, "phx_reply", {"status": "ok", "response": {"diff": {"1": "41"}}}])
    );
}

/// Everything the tests of this file log, each event as its level and fields, with
/// the thread it was logged on. A test's server runs on the test's own thread.
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

    /// What this thread has logged about a LiveView.
    fn of_this_test() -> Vec<String> {
        let (logged, thread) = (LOGGED.lock().unwrap(), thread::current().id());
        let of_thread = logged.iter().filter(|(logger, _)| *logger == thread);
        let fields = of_thread.map(|(_, fields)| fields.clone());
        fields
            .filter(|fields| fields.contains("live_view="))
            .collect()
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

#[tokio::test]
async fn an_event_that_cannot_be_decoded_is_logged_with_its_name_and_without_its_values() {
    Logs::collect();
    let (mut client, topic) = joined_counter().await;

    let secret = json!({"by": "hunter2", "password": "hunter2"});
    let click = json!({"type": "click", "event": "add", "value": secret});
    client.send(json!(["4", "5", topic, "event", click])).await;
    assert_eq!(client.receive().await, crashed(&topic));

    let logged = Logs::of_this_test();
    let [logged] = logged.as_slice() else {
        panic!("not logged once: {logged:?}");
    };
    assert!(logged.starts_with("ERROR "), "{logged}");
    assert!(logged.contains(r#"event="add""#), "{logged}");
    assert!(logged.contains("live_socket::Counter"), "{logged}");
    let reason = "the field `by` is missing or does not parse";
    assert!(logged.contains(reason), "{logged}");
    assert!(!logged.contains("hunter2"), "{logged}");
}

#[tokio::test]
async fn the_name_of_an_event_is_logged_escaped_and_cut_short() {
    Logs::collect();
    let (mut client, topic) = joined_counter().await;

    // A name made to look like a log line of its own, and far too long.
    let name = format!("x\nERROR forged {}", "é".repeat(5000));
    let click = json!({"type": "click", "event": name, "value": {}});
    client.send(json!(["4", "5", topic, "event", click])).await;
    assert_eq!(client.receive().await, crashed(&topic));

    let logged = &Logs::of_this_test()[0];
    assert!(logged.contains(r#"event="x\nERROR forged é"#), "{logged}");
    assert!(logged.len() < 400, "{} bytes logged", logged.len());
}

#[tokio::test]
async fn each_ref_is_echoed_as_it_was_sent() {
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/counter/5").await;
    let topic = &page.topic;
    let mut join = page.join("11");
    join[1] = json!("12");

    client.send(join).await;
    let joined = client.receive().await;
    assert_eq!((&joined[0], &joined[1]), (&json!("11"), &json!("12")));

    let click = json!({"type": "click", "event": "inc", "value": {}});
    client
        .send(json!(["11", "13", topic, "event", click]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["11", "13", topic, "phx_reply", {"status": "ok", "response": {"diff": {"1": "6"}}}])
    );

    client
        .send(json!(["11", "14", topic, "phx_leave", {}]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["11", "14", topic, "phx_reply", {"status": "ok", "response": {}}])
    );
    // The close carries the join's ref in both places, as Phoenix sends it.
    assert_eq!(
        client.receive().await,
        json!(["11", "11", topic, "phx_close", {}])
    );
}

#[tokio::test]
async fn joining_a_topic_again_mounts_the_live_view_afresh() {
    let mut client = Client::connect(serve().await).await;
    let page = Page::get("/counter/5").await;
    let click = json!({"type": "click", "event": "inc", "value": {}});
    client.send(page.join("4")).await;
    client.receive().await;
    client
        .send(json!(["4", "5", page.topic, "event", click]))
        .await;
    client.receive().await;

    client.send(page.join("6")).await;

    let rejoined = client.receive().await;
    assert_eq!((&rejoined[0], &rejoined[1]), (&json!("6"), &json!("6")));
    assert_eq!(rejoined[4]["response"]["rendered"]["1"], "5");
    client
        .send(json!(["6", "7", page.topic, "event", click]))
        .await;
    assert_eq!(
        client.receive().await,
        json!(["6", "7", page.topic, "phx_reply", {"status": "ok", "response": {"diff": {"1": "6"}}}])
    );
}

#[tokio::test]
async fn a_frame_that_is_not_an_envelope_ends_the_connection() {
    let (mut client, _topic, mut dropped) = joined_watched("not-an-envelope").await;

    client
        .0
        .send(Message::Text("not an envelope".into()))
        .await
        .unwrap();

    // The server hangs up, and the LiveView joined on the socket ends with it.
    let closed = async { while let Some(Ok(_)) = client.0.next().await {} };
    let closed = tokio::time::timeout(Duration::from_secs(5), closed);
    closed.await.expect("the socket stayed open");
    heard(&mut dropped).await;
}

#[tokio::test]
async fn a_heartbeat_is_answered() {
    let mut client = Client::connect(serve().await).await;

    client
        .send(json!([null, "7", "phoenix", "heartbeat", {}]))
        .await;

    assert_eq!(
        client.receive().await,
        json!([null, "7", "phoenix", "phx_reply", {"status": "ok", "response": {}}])
    );
}
