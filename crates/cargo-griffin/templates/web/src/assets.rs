//! The files the pages ask for: everything in `static/`, served from disk as it is when
//! asked, so a rebuilt stylesheet or script needs no restart. `static/assets/app.js` and
//! `static/assets/app.css` are built from `assets/` by esbuild and Tailwind, which
//! `cargo griffin dev` runs in watch mode. Nothing outside `static/` is served.

use crate::state::AppState;
use griffin::axum::Router;
use griffin::security::SecureHeadersLayer;

/// The files every page asks for. A build that was never made fails at start.
pub fn check() -> Result<(), String> {
    griffin::static_files::check_built(
        std::path::Path::new(DIR),
        &["assets/app.js", "assets/app.css"],
        "run `cargo griffin dev`, which builds them with esbuild and Tailwind",
    )
}

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/static");

pub fn router() -> Router<AppState> {
    // The directory is fixed when the crate is compiled; a deployment that moves
    // the files needs a setting for it.
    // The fallback of the app: static files and the 404 of a path no route has. They are
    // outside every Pipeline, so they get the secure headers here. The routes of the
    // table keep their own Pipeline's choice (a Scope may drop a header on purpose), which
    // a layer around the whole app would override.
    griffin::static_files::serve(DIR).layer(SecureHeadersLayer::new())
}
