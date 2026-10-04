//! What a page does in the browser without hand-written plumbing: client commands,
//! client hooks with a reply, events pushed from the server, and the client's own key,
//! window, focus, click-away, debounce and throttle bindings.
//!
//! ```bash
//! cargo run -p commands
//! ```
//!
//! Then open <http://127.0.0.1:4000> and <http://127.0.0.1:4000/bindings>. `PORT`
//! chooses another port.

use griffin_web::axum::http::header::CONTENT_TYPE;
use griffin_web::axum::response::IntoResponse;
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{Event, Js, LiveView, Socket, live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::session::SessionLayer;
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use serde_json::json;
use std::convert::Infallible;
use tokio::net::TcpListener;

/// An example's key, public in this repository: it protects nothing.
const EXAMPLE_SECRET: &str = "griffin commands example: not a secret";

/// Commands, hooks, a reply and a pushed event.
struct Menu {
    told: u32,
    told_by: String,
}

#[derive(Event)]
enum MenuEvent {
    /// Pushed by a command, after it toggled the menu in the browser.
    Toggled { by: String },
    /// Pushed by the `Lookup` hook, which waits for the reply.
    Lookup { id: u32 },
    /// Answered with an event pushed to the `Highlight` hook.
    Highlight { id: String },
}

impl LiveView for Menu {
    type Params = ();
    type Event = MenuEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> Menu {
        Menu {
            told: 0,
            told_by: String::new(),
        }
    }

    async fn handle_event(
        &mut self,
        event: MenuEvent,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            MenuEvent::Toggled { by } => {
                self.told += 1;
                self.told_by = by;
            }
            // Text a user could have written: it reaches the hook as JSON, intact.
            MenuEvent::Lookup { id } => {
                socket.reply(json!({"id": id, "name": "Milk & \"eggs\" <fresh>"}));
            }
            MenuEvent::Highlight { id } => socket.push_event("highlight", json!({"id": id})),
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        // Runs in the browser alone: no Event, no round trip.
        let toggle = Js::new().toggle("#menu");
        // One click does both: the browser toggles, and the server is told.
        let toggle_and_tell = Js::new()
            .toggle("#menu")
            .add_class("told", "#toggle-and-tell")
            .push(&MenuEvent::Toggled {
                by: "the second button".to_owned(),
            });
        let highlight = MenuEvent::Highlight {
            id: "item-2".to_owned(),
        };
        html! {
            <main>
                <button id="toggle" phx-click={toggle}>Toggle</button>
                <button id="toggle-and-tell" phx-click={toggle_and_tell}>Toggle and tell</button>
                <ul id="menu" style="display: none">
                    <li id="item-1">Open</li>
                    <li id="item-2">Save</li>
                </ul>
                <p>Told <output id="told">{@told}</output> times, last by <output id="told-by">{@told_by}</output></p>

                <button id="lookup" phx-hook="Lookup" data-id="3">Look up</button>
                <output id="lookup-result" phx-update="ignore"></output>

                <button id="highlight" phx-click={highlight}>Highlight</button>
                <output id="highlighted" phx-hook="Highlight" phx-update="ignore"></output>
            </main>
        }
    }
}

/// The client's own bindings. Their Events are bound by name: the browser supplies
/// the fields (`key`, `value`), so there is no value to bind at render time.
struct Bindings {
    last_key: String,
    escapes: u32,
    searched: String,
    focus: String,
    away: u32,
}

#[derive(Event)]
enum BindingEvent {
    KeyDown { key: String },
    Escaped { key: String },
    Search { value: String },
    Entered,
    Left { value: String },
    ClickedAway,
}

impl LiveView for Bindings {
    type Params = ();
    type Event = BindingEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> Bindings {
        Bindings {
            last_key: String::new(),
            escapes: 0,
            searched: String::new(),
            focus: "nowhere".to_owned(),
            away: 0,
        }
    }

    async fn handle_event(
        &mut self,
        event: BindingEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            BindingEvent::KeyDown { key } => self.last_key = key,
            BindingEvent::Escaped { key } => {
                self.escapes += 1;
                self.last_key = key;
            }
            BindingEvent::Search { value } => self.searched = value,
            BindingEvent::Entered => self.focus = "in the search".to_owned(),
            BindingEvent::Left { value } => self.focus = format!("left with {value}"),
            BindingEvent::ClickedAway => self.away += 1,
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! {
            <main>
                <h1 phx-window-keydown="key_down">Bindings</h1>
                <p phx-window-keyup="escaped" phx-key="Escape">
                    Last key: <output id="last-key">{@last_key}</output>,
                    escapes: <output id="escapes">{@escapes}</output>
                </p>
                <input id="search" phx-keyup="search" phx-debounce="500" phx-focus="entered" phx-blur="left">
                <input id="throttled" phx-keyup="search" phx-throttle="60000">
                <p>Searched: <output id="searched">{@searched}</output></p>
                <p>Focus: <output id="focus">{@focus}</output></p>
                <div id="panel" phx-click-away="clicked_away">Panel</div>
                <p>Clicks away: <output id="away">{@away}</output></p>
            </main>
        }
    }
}

/// This example's script, and the two client bundles it imports: the counter
/// example's vendored copies, compiled in from where they are, not copied.
fn assets() -> Router<SigningKey> {
    fn script(source: &'static str) -> impl IntoResponse {
        ([(CONTENT_TYPE, "text/javascript")], source)
    }
    Router::new()
        .route(
            "/assets/app.js",
            get(|| async { script(include_str!("../assets/app.js")) }),
        )
        .route(
            "/assets/vendor/phoenix.mjs",
            get(|| async { script(include_str!("../../counter/assets/vendor/phoenix.mjs")) }),
        )
        .route(
            "/assets/vendor/phoenix_live_view.esm.js",
            get(|| async {
                script(include_str!(
                    "../../counter/assets/vendor/phoenix_live_view.esm.js"
                ))
            }),
        )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The session holds the CSRF token the socket is opened with. Plain HTTP, so
    // `.secure(false)`, as in the counter example.
    let session = SessionLayer::new(EXAMPLE_SECRET)?.secure(false);
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(session))
        .route("/", live::<Menu, _>())
        .route("/bindings", live::<Bindings, _>());
    let pages = Router::new().merge(site);
    let app = pages
        .clone()
        .route("/live/websocket", live_socket(pages))
        .merge(assets())
        .with_state(SigningKey::new(EXAMPLE_SECRET)?);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Commands at http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
