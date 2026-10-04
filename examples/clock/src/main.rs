//! A clock: a page that changes without the user doing anything. A timer on the server
//! sends the LiveView a Message ten times a second, and each one is pushed to the
//! browser as what changed.
//!
//! ```bash
//! cargo run -p clock
//! ```
//!
//! Then open <http://127.0.0.1:4000>. `PORT` chooses another port.

use griffin_web::axum::http::header::CONTENT_TYPE;
use griffin_web::axum::response::IntoResponse;
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{LiveView, Socket, live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::session::SessionLayer;
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use std::convert::Infallible;
use std::time::Duration;
use tokio::net::TcpListener;

/// An example's key, public in this repository: it protects nothing.
const EXAMPLE_SECRET: &str = "griffin clock example: not a secret";

struct Clock {
    ticks: u64,
}

/// What the timer tells the clock.
struct Tick;

impl LiveView for Clock {
    type Params = ();
    // The browser asks nothing of this page.
    type Event = Infallible;
    type Message = Tick;
    type Error = Infallible;

    async fn mount(_params: (), socket: &mut Socket) -> Clock {
        // Only a connected page ticks: the Dead render is gone once it is sent.
        if socket.connected() {
            let clock = socket.handle::<Clock>();
            // The timer is a task of this LiveView, so it stops when the user leaves.
            socket.spawn(async move {
                let mut timer = tokio::time::interval(Duration::from_millis(100));
                loop {
                    timer.tick().await;
                    if clock.send(Tick).is_err() {
                        break;
                    }
                }
            });
        }
        Clock { ticks: 0 }
    }

    async fn handle_message(
        &mut self,
        _tick: Tick,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        self.ticks += 1;
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! {
            <main>
                <h1>Ticks: <output id="ticks">{@ticks}</output></h1>
            </main>
        }
    }
}

/// The script the page asks for and the two client bundles it imports: the counter
/// example's, compiled in from where they are, not copied.
fn assets() -> Router<SigningKey> {
    fn script(source: &'static str) -> impl IntoResponse {
        ([(CONTENT_TYPE, "text/javascript")], source)
    }
    Router::new()
        .route(
            "/assets/app.js",
            get(|| async { script(include_str!("../../counter/assets/app.js")) }),
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
        .route("/", live::<Clock, _>());
    let pages = Router::new().merge(site);
    let app = pages
        .clone()
        .route("/live/websocket", live_socket(pages))
        .merge(assets())
        .with_state(SigningKey::new(EXAMPLE_SECRET)?);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Clock at http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
