//! One signup form, twice: as a LiveView that validates while the user types, and as
//! a plain controller form. The same Changeset backs both, and the same Components
//! (`components.rs`) render it. The Context and its Capability are in `accounts.rs`.
//!
//! ```bash
//! cargo run -p forms
//! ```
//!
//! Then open <http://127.0.0.1:4000/live> and <http://127.0.0.1:4000/classic>. `PORT`
//! chooses another port. The username `ada` is taken.

mod accounts;
mod components;
mod pages;

use accounts::Memory;
use components::Site;
use griffin_web::axum::extract::{FromRef, State};
use griffin_web::axum::http::header::CONTENT_TYPE;
use griffin_web::axum::response::IntoResponse;
use griffin_web::axum::routing::get;
use griffin_web::axum::{self, Router};
use griffin_web::html;
use griffin_web::live::{live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use griffin_web::session::SessionLayer;
use griffin_web::token::SigningKey;
use std::sync::Arc;
use tokio::net::TcpListener;

/// An example's key, public in this repository: it protects nothing.
const EXAMPLE_SECRET: &str = "griffin forms example: not a secret";

/// The application state, built once in `main`: what handlers take with `State`.
#[derive(Clone)]
struct AppState {
    key: SigningKey,
    accounts: Arc<Memory>,
}

/// The LiveViews' routes take the signing key from the state.
impl FromRef<AppState> for SigningKey {
    fn from_ref(state: &AppState) -> SigningKey {
        state.key.clone()
    }
}

impl FromRef<AppState> for Arc<Memory> {
    fn from_ref(state: &AppState) -> Arc<Memory> {
        state.accounts.clone()
    }
}

/// The script the pages ask for and the two client bundles it imports, which are the
/// counter example's vendored copies, compiled in from where they are.
fn assets() -> Router<AppState> {
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
    // Served over plain HTTP, so the cookie is not `Secure`: see the counter example.
    let session = SessionLayer::new(EXAMPLE_SECRET)?.secure(false);
    let site = Scope::new("/")
        .pipe_through(Pipeline::browser(session))
        .layout(|page, flash| html! { <Site flash={flash}>{page}</Site> })
        .route("/live", live::<pages::LiveSignup, _>())
        .route("/classic", get(pages::show).post(pages::create));
    let mut pages = Router::new().merge(site);
    // The browser tests hold a submit in flight and release it here. A route that is
    // only there when the test server asks for it, which a real application never does.
    if std::env::var_os("FORMS_GATE").is_some() {
        pages = pages.route(
            "/test/gate",
            get(|State(accounts): State<Arc<Memory>>| async move { accounts.open_gate() }),
        );
    }
    let app = pages
        .clone()
        .route("/live/websocket", live_socket(pages))
        .merge(assets())
        .with_state(AppState {
            key: SigningKey::new(EXAMPLE_SECRET)?,
            accounts: accounts::store(),
        });

    let port = std::env::var("PORT").unwrap_or_else(|_| "4000".to_owned());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Forms at http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
