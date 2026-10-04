//! Navigation: a page that patches its own URL, links to other LiveViews over the open
//! connection, to a LiveView in another live session (which loads as a page), and a
//! button that leaves for a controller page with a flash message. The title follows.
//!
//! ```bash
//! cargo run -p navigation
//! ```
//!
//! Then open <http://127.0.0.1:4000>. `PORT` chooses another port.

use griffin_web::axum::http::header::CONTENT_TYPE;
use griffin_web::axum::response::IntoResponse;
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::live::{BadTarget, Event, LiveView, Socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::session::{Flash, SessionLayer};
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use griffin_web::{html, routes};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::net::TcpListener;

/// An example's key, public in this repository: it protects nothing.
const EXAMPLE_SECRET: &str = "griffin navigation example: not a secret";

/// How many items a page of the list holds, and how many there are.
const PER_PAGE: u32 = 5;
const ITEMS: u32 = 12;

/// A list in pages. The page is in the URL (`/?page=2`), so it can be bookmarked and
/// the back button walks through the pages; the clicks are only in the state, so that
/// they show that a patch keeps it.
struct Items {
    page: u32,
    clicks: u32,
}

#[derive(Event)]
enum ItemsEvent {
    Count,
    /// Patches the URL from the server.
    Next,
    /// Leaves for another page, with a flash.
    Save,
}

/// What the query string may hold. It is the browser's, so a value that does not parse
/// is an empty one, not a failure.
#[derive(Deserialize, Default)]
struct Query {
    page: Option<u32>,
}

fn page_path(page: u32) -> String {
    format!("{}?page={page}", Routes::items())
}

impl LiveView for Items {
    type Params = ();
    type Event = ItemsEvent;
    type Message = Infallible;
    type Error = BadTarget;

    async fn mount(_params: (), _socket: &mut Socket) -> Items {
        Items { page: 1, clicks: 0 }
    }

    // Runs after mount in both renders, and again for each patch, with the state kept.
    async fn handle_params(&mut self, _params: (), socket: &mut Socket) -> Result<(), Self::Error> {
        let query: Query = socket.query().unwrap_or_default();
        self.page = query.page.unwrap_or(1).clamp(1, ITEMS.div_ceil(PER_PAGE));
        socket.set_title(format!("Items, page {}", self.page));
        Ok(())
    }

    async fn handle_event(
        &mut self,
        event: ItemsEvent,
        socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            ItemsEvent::Count => self.clicks += 1,
            ItemsEvent::Next => socket.patch(&page_path(self.page + 1))?,
            ItemsEvent::Save => {
                socket.put_flash("info", "Saved");
                socket.redirect(&Routes::goodbye())?;
            }
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        let first = (self.page - 1) * PER_PAGE + 1;
        let last = (first + PER_PAGE - 1).min(ITEMS);
        html! {
            <main>
                <h1>Items</h1>
                <p>Items <output id="range">{first}-{last}</output>, page <output id="page">{@page}</output></p>
                <nav>
                    <a id="page-1" patch={page_path(1)}>Page 1</a>
                    <a id="page-2" patch={page_path(2)}>Page 2</a>
                    <a id="page-3" patch={page_path(3)}>Page 3</a>
                    <button id="next" phx-click={ItemsEvent::Next}>Next page</button>
                </nav>
                <p>Clicks: <output id="clicks">{@clicks}</output>
                    <button id="count" phx-click={ItemsEvent::Count}>Count</button></p>
                <p>
                    <a id="about-link" navigate={Routes::about()}>About</a>
                    <a id="admin-link" navigate={Routes::admin()}>Admin</a>
                    <button id="save" phx-click={ItemsEvent::Save}>Save and leave</button>
                </p>
            </main>
        }
    }
}

/// A LiveView of the same live session as the list: reaching it keeps the connection.
struct About;

impl LiveView for About {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), socket: &mut Socket) -> About {
        socket.set_title("About");
        About
    }

    fn render(&self) -> Rendered {
        html! {
            <main>
                <h1>About</h1>
                <a id="items-link" navigate={Routes::items()}>Items</a>
            </main>
        }
    }
}

/// A LiveView of another live session: a link to it loads the page.
struct Admin;

impl LiveView for Admin {
    type Params = ();
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), socket: &mut Socket) -> Admin {
        socket.set_title("Admin");
        Admin
    }

    fn render(&self) -> Rendered {
        html! {
            <main>
                <h1>Admin</h1>
                <a id="items-link" navigate={Routes::items()}>Items</a>
            </main>
        }
    }
}

/// The layout of the controller pages: it shows the flash.
fn site(page: Rendered, flash: &Flash) -> Rendered {
    html! {
        <html>
            <head><title>Goodbye</title></head>
            <body>
                <p :if={flash.get("info").is_some()} id="flash">{flash.get("info").unwrap()}</p>
                {page}
            </body>
        </html>
    }
}

async fn goodbye() -> Rendered {
    html! { <h1>Goodbye</h1> }
}

fn browser(session: SessionLayer) -> Pipeline<SigningKey> {
    Pipeline::browser(session)
}

routes! {
    fn router(session: SessionLayer) -> Router<SigningKey>;

    scope "/" {
        pipe_through [browser(session)];

        // A live navigation stays on the open connection within a live session.
        live_session public {
            LIVE "/" => Items as items;
            LIVE "/about" => About as about;
        }
        live_session admin {
            LIVE "/admin" => Admin as admin;
        }

        scope "/" {
            layout site;
            GET "/goodbye" => goodbye as goodbye;
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
    // Plain HTTP, so `.secure(false)`, as in the counter example.
    let session = SessionLayer::new(EXAMPLE_SECRET)?.secure(false);
    let app = router(session)
        .merge(assets())
        .with_state(SigningKey::new(EXAMPLE_SECRET)?);

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Navigation at http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
