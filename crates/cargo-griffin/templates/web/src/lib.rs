//! The web layer of __App__.

// Griffin's macros (`html!`, `routes!`, `#[component]`, ...) write paths that start
// `::griffin_web::`, the crate's published name, and Cargo.toml names it `griffin`. This
// line makes the published name resolve too. Without it the macros do not compile.
extern crate griffin as griffin_web;

pub mod assets;
pub mod components;
pub mod config;
pub mod layouts;
pub mod live;
pub mod pages;
pub mod routes;
pub mod state;

use config::Config;
use griffin::axum::Router;
use griffin::error::ErrorPages;
use griffin::pipeline::Pipeline;
use griffin::session::SessionLayer;
use griffin::token::SigningKey;
use state::AppState;

/// The whole application as one `Router`.
///
/// The browser Pipeline is secure headers, then the session, then the CSRF check. It
/// is built by the one call that makes those defaults the default, and every route in
/// `routes.rs` goes through it. The socket checks the page's CSRF token and the
/// request's origin.
pub fn app(config: &Config) -> Result<Router, Box<dyn std::error::Error>> {
    let secret = config.secret_key_base.expose();
    let session = SessionLayer::new(secret)?.secure(config.secure_cookie);
    let state = AppState::new(SigningKey::new(secret)?);
    let pages = routes::router(Pipeline::browser(session))
        .merge(assets::router())
        .with_state(state);
    let app = pages.layer(ErrorPages::new(config.error_page).pages(layouts::error_page));
    // Only in a build `cargo griffin dev` made, and then only while it runs the app.
    #[cfg(feature = "dev")]
    let app = griffin::dev::reload(app);
    Ok(app)
}
