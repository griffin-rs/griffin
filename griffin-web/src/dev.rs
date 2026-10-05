//! What `cargo griffin dev` asks of the application, compiled only with the `dev`
//! feature, which only `cargo griffin dev` turns on (it builds with `--features dev`).
//! A release build has neither this module nor the crates behind it. Never build a
//! release with `--all-features`: it would turn this on.
//!
//! - [`inherited_listener`]: the socket `dev` holds open across restarts, handed to each
//!   new process as the `listenfd` crate reads it (`LISTEN_FDS`, `LISTEN_PID`, fd 3).
//! - [`reload`]: injects a script that reloads the browser when the server restarts.
//!   It does nothing unless `GRIFFIN_DEV` is set, which `dev` sets and nothing else does.

use axum::Router;
use tokio::net::TcpListener;

/// The environment variable `cargo griffin dev` sets in the app it runs.
pub const ENV: &str = "GRIFFIN_DEV";

/// The listening socket handed over by `dev`, or `None` if the app was started some
/// other way.
pub fn inherited_listener() -> std::io::Result<Option<TcpListener>> {
    let Some(listener) = listenfd::ListenFd::from_env().take_tcp_listener(0)? else {
        return Ok(None);
    };
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener).map(Some)
}

/// `router` with the browser reload added, if the app runs under `cargo griffin dev`.
pub fn reload<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    if std::env::var_os(ENV).is_some() {
        router.layer(tower_livereload::LiveReloadLayer::new())
    } else {
        router
    }
}
