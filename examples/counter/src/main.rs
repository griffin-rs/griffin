//! A counter: the smallest LiveView. The number lives on the server, and the buttons
//! change it through the unmodified Phoenix client.
//!
//! ```bash
//! cargo run -p counter
//! ```
//!
//! Then open <http://127.0.0.1:4000>. `PORT` chooses another port.

use griffin_web::axum::http::header::CONTENT_TYPE;
use griffin_web::axum::response::IntoResponse;
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{Event, LiveView, Socket, live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::session::SessionLayer;
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use std::convert::Infallible;
use tokio::net::TcpListener;

/// An example's key, public in this repository: it protects nothing. An application
/// reads its key from configuration and never writes it in the source.
const EXAMPLE_SECRET: &str = "griffin counter example: not a secret";

struct Counter {
    count: i64,
}

/// What the browser can ask of the counter. Each button binds one of these, and the
/// handler gets it back as this type: a name the enum does not have does not compile.
#[derive(Event)]
enum CounterEvent {
    Add { by: i64 },
    Reset,
}

impl LiveView for Counter {
    type Params = ();
    type Event = CounterEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> Counter {
        Counter { count: 0 }
    }

    async fn handle_event(
        &mut self,
        event: CounterEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            CounterEvent::Add { by } => self.count += by,
            CounterEvent::Reset => self.count = 0,
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        html! {
            <main>
                <h1>Count: <output id="count">{@count}</output></h1>
                <button phx-click={CounterEvent::Add { by: -1 }}>Decrement</button>
                <button phx-click={CounterEvent::Add { by: 1 }}>Increment</button>
                <button phx-click={CounterEvent::Reset}>Reset</button>
            </main>
        }
    }
}

/// The script the page asks for and the two client bundles it imports, compiled into
/// the binary. See `assets/vendor/README.md` for where the bundles come from.
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
            get(|| async { script(include_str!("../assets/vendor/phoenix.mjs")) }),
        )
        .route(
            "/assets/vendor/phoenix_live_view.esm.js",
            get(|| async { script(include_str!("../assets/vendor/phoenix_live_view.esm.js")) }),
        )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The session holds the CSRF token the socket is opened with. The example is
    // served over plain HTTP, from which not every browser keeps a `Secure` cookie,
    // even on this machine. An application served over HTTPS leaves `.secure(false)` out.
    let session = SessionLayer::new(EXAMPLE_SECRET)?.secure(false);
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(session))
        .route("/", live::<Counter, _>());
    let pages = Router::new().merge(site);
    let app = pages
        .clone()
        // The client opens the path it was given in `app.js`, plus `/websocket`.
        .route("/live/websocket", live_socket(pages))
        .merge(assets())
        .with_state(SigningKey::new(EXAMPLE_SECRET)?);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Counter at http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
