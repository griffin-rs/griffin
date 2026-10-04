//! Error pages: what a browser is shown when a request fails.
//!
//! [`ErrorPages`] is a Layer for the whole router, put on after every Scope is merged
//! and after the route of the socket. It gives a page to:
//!
//! - **An unexpected failure**: an error the application did not provide for, which
//!   its one error mapping (one place maps errors to responses) turns into an [`Unexpected`], or a panic in a
//!   handler. The status is 500, and the page is the one the configuration chose
//!   with [`ErrorPage`].
//! - **An error status with no body**: no route for the path (404), no route for the
//!   method (405), a status a handler or a Layer returned bare. The page says what
//!   the status says, or is the application's own, given with [`ErrorPages::pages`].
//!
//! A response with a body of its own is never touched, so a handler that returns a
//! page with a status, or JSON, sends what it returned. That holds for axum's own
//! rejections too: a handler that takes an extension nothing put there, or a `Path`
//! type its route does not fit, answers 500 with a line of text that names the Rust
//! type. It is a bug in the wiring, seen on the first request, and it holds nothing
//! of the application's data.
//!
//! ```no_run
//! use griffin_web::axum::Router;
//! use griffin_web::axum::http::StatusCode;
//! use griffin_web::axum::response::{IntoResponse, Response};
//! use griffin_web::axum::routing::get;
//! use griffin_web::config::{self, ConfigError};
//! use griffin_web::error::{ErrorPage, ErrorPages, Unexpected};
//! use griffin_web::html;
//! use griffin_web::router::Scope;
//! use griffin_web::template::Rendered;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Config {
//!     /// `error_page = "development"` in the file, or `SHOP_ERROR_PAGE=development`.
//!     /// Without either it is the page that reveals nothing.
//!     #[serde(default)]
//!     error_page: ErrorPage,
//! }
//!
//! /// The application's own error type: every error a handler ends with `?` on.
//! enum AppError {
//!     NoSuchOrder,
//!     Unexpected(Unexpected),
//! }
//!
//! impl From<std::io::Error> for AppError {
//!     /// The caller is the `?` of the handler, which is then where it failed.
//!     #[track_caller]
//!     fn from(error: std::io::Error) -> AppError {
//!         AppError::Unexpected(Unexpected::new(error))
//!     }
//! }
//!
//! impl IntoResponse for AppError {
//!     fn into_response(self) -> Response {
//!         match self {
//!             AppError::NoSuchOrder => StatusCode::NOT_FOUND.into_response(),
//!             AppError::Unexpected(failure) => failure.into_response(),
//!         }
//!     }
//! }
//!
//! async fn show_order() -> Result<Rendered, AppError> {
//!     let order = std::fs::read_to_string("orders/1.txt")?;
//!     Ok(html! { <h1>{order}</h1> })
//! }
//!
//! /// The pages the application has of its own. Griffin's are used for the rest.
//! fn error_page(status: StatusCode) -> Option<Rendered> {
//!     match status {
//!         StatusCode::NOT_FOUND => Some(html! { <h1>We have no such page</h1> }),
//!         _ => None,
//!     }
//! }
//!
//! fn main() -> Result<(), ConfigError> {
//!     let config: Config = config::load("config.toml", "SHOP")?;
//!     let site = Scope::new("/").route("/orders/1", get(show_order));
//!     let app: Router = Router::new()
//!         .merge(site)
//!         .layer(ErrorPages::new(config.error_page).pages(error_page));
//!     Ok(())
//! }
//! ```
//!
//! # The two pages
//!
//! [`ErrorPage::Plain`] is the default. For an unexpected failure it sends the page
//! of the status 500 and nothing of the failure: no message, no type, no file, no
//! line, in the body or in a header. The failure is in the log.
//!
//! [`ErrorPage::Development`] is for the developer's own machine, and has to be asked
//! for in the configuration. It shows:
//!
//! - the type of the error, or `panic`;
//! - the error chain: the message of the error, then that of each
//!   [`source`](std::error::Error::source) under it;
//! - the error as `Debug` prints it;
//! - where it failed. For an error this is where [`Unexpected::new`] was called,
//!   which with `#[track_caller]` on the application's `From` is the `?` in the
//!   handler. A Rust error does not record where it was made. For a panic it is the
//!   file, line and column of the panic itself.
//!
//! It shows nothing of the request, the session or the configuration, and there is
//! no stack trace on it.
//!
//! Whichever page is sent, the failure is logged once through `tracing`, at the
//! error level, with its type, its chain and where it failed.
//!
//! # What is redacted, and what is not
//!
//! Redaction is by type. [`Secret`](crate::config::Secret),
//! [`Session`](crate::session::Session), [`SessionLayer`](crate::session::SessionLayer)
//! and [`SigningKey`](crate::token::SigningKey) print `REDACTED` for `Debug` and have
//! no `Display`, so an error that holds one, or a configuration with one in it, shows
//! no secret and no session value on the page or in the log.
//!
//! Nothing looks for a secret in text. A secret the application took out with
//! [`Secret::expose`](crate::config::Secret::expose) and wrote into a message, a
//! secret kept in a plain `String`, and whatever the error of another crate prints
//! (a database URL with its password, the body of a request) are shown and logged
//! as they are. That is why the development page is not for a server others can
//! reach.
//!
//! Everything on the development page is HTML-escaped: a message can quote what a
//! user sent.
//!
//! # Panics
//!
//! The Layer catches a panic raised while a handler, an extractor or a Layer inside
//! it runs, and answers 500. The first [`ErrorPages::new`] chains a
//! [panic hook](std::panic::set_hook) in front of the one that is set, to record
//! where each panic is raised; the hook that was set still runs, so the panic is
//! still printed. A hook set later without chaining takes the place away, and the
//! page then says so. A program built with `panic = "abort"` ends at the panic, and
//! nothing can answer.
//!
//! A LiveView that fails after it connected has no response to give a page to: that
//! is the socket's to handle.

use crate::template::{Rendered, Slot};
use axum::body::{Body, HttpBody as _};
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe, Location};
use std::pin::Pin;
use std::sync::Once;
use std::task::{Context, Poll};
use tower_layer::Layer;
use tower_service::Service;

/// Which page answers an unexpected failure: a setting of the application's
/// configuration, read when it starts, and not a flag of the build. In a file it is
/// `error_page = "plain"` or `error_page = "development"`, for a field of that name.
///
/// See [the two pages](self#the-two-pages).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPage {
    /// The page of the status 500, which reveals nothing of the failure. The default.
    #[default]
    Plain,
    /// The page for the developer: what failed and where. Never for a server others
    /// can reach.
    Development,
}

/// An unexpected failure, as a response: what the application's error mapping
/// returns for an error it did not provide for. See the [module's example](self).
///
/// It holds what the error printed when it was made, not the error.
#[derive(Debug, Clone)]
pub struct Unexpected {
    /// The type of the error, or `panic`.
    kind: &'static str,
    /// The message of the error, then that of each error under it.
    chain: Vec<String>,
    /// The error as `Debug` prints it. A panic has none.
    debug: Option<String>,
    location: String,
}

impl Unexpected {
    /// The failure that `error` is, at the place this is called from: the caller of
    /// the function this is in, if that function is marked `#[track_caller]`.
    #[track_caller]
    pub fn new<E: std::error::Error>(error: E) -> Unexpected {
        Unexpected {
            kind: std::any::type_name::<E>(),
            chain: chain(&error),
            debug: Some(format!("{error:#?}")),
            location: Location::caller().to_string(),
        }
    }
}

/// The message of `error`, then that of each error under it. It is logged as `Debug`
/// prints it (`chain = ?...`), one escaped line whatever the messages hold.
// A chain is cut at 32 causes, so one that loops cannot hang a request.
pub(crate) fn chain(error: &dyn std::error::Error) -> Vec<String> {
    let causes = std::iter::successors(error.source(), |cause| cause.source()).take(32);
    std::iter::once(error.to_string())
        .chain(causes.map(ToString::to_string))
        .collect()
}

/// A 500 with no body, and the failure in the log. [`ErrorPages`] gives it a page.
impl IntoResponse for Unexpected {
    fn into_response(self) -> Response {
        // Logged here, so that it is logged with or without the Layer. The messages
        // are logged as `Debug` prints them: one line, whatever they hold.
        tracing::error!(
            kind = self.kind,
            chain = ?self.chain,
            location = self.location,
            "a request failed unexpectedly"
        );
        let mut response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
        response.extensions_mut().insert(self);
        response
    }
}

/// The Layer that gives a failed request its page. It goes on the router itself, so
/// that it is outside every Scope and sees a request no route matched. See the
/// [module's documentation](self).
#[derive(Debug, Clone, Copy)]
pub struct ErrorPages {
    page: ErrorPage,
    pages: fn(StatusCode) -> Option<Rendered>,
}

impl ErrorPages {
    /// A Layer that answers an unexpected failure with `page`, which the application
    /// reads from its configuration. `ErrorPage::default()` reveals nothing.
    ///
    /// The first call chains a panic hook: see [panics](self#panics).
    pub fn new(page: ErrorPage) -> ErrorPages {
        record_where_panics_are_raised();
        let pages = |_| None;
        ErrorPages { page, pages }
    }

    /// Sends the application's own page for a status where `pages` returns one, and
    /// Griffin's where it returns `None`.
    ///
    /// The page is the whole document: the Layer is outside every Scope, so no
    /// layout wraps it, and the application calls its layout Component itself. It is
    /// given the status and nothing of a failure, so it has nothing to reveal. For
    /// an unexpected failure it is asked for the page of 500, unless the
    /// configuration chose the development page.
    ///
    /// Nothing catches a panic of `pages` itself: the connection then ends without a
    /// response.
    pub fn pages(mut self, pages: fn(StatusCode) -> Option<Rendered>) -> ErrorPages {
        self.pages = pages;
        self
    }

    /// The response with an error page for its body, if it is one that needs it.
    fn finish(self, mut response: Response) -> Response {
        let status = response.status();
        let of_status = || (self.pages)(status).unwrap_or_else(|| plain_page(status));
        let is_error = status.is_client_error() || status.is_server_error();
        let page = match response.extensions_mut().remove::<Unexpected>() {
            Some(failure) if self.page == ErrorPage::Development => development_page(&failure),
            Some(_) => of_status(),
            None if is_error && response.body().is_end_stream() => of_status(),
            None => return response,
        };
        let html = page.to_html();
        let headers = response.headers_mut();
        let html_type = HeaderValue::from_static("text/html; charset=utf-8");
        headers.insert(header::CONTENT_TYPE, html_type);
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(html.len()));
        *response.body_mut() = Body::from(html);
        response
    }
}

/// The page that says what the status says and nothing else. Like the development
/// page it is never diffed, so its fingerprint is never read.
fn plain_page(status: StatusCode) -> Rendered {
    let reason = status.canonical_reason().unwrap_or("Error");
    Rendered::new(
        0,
        &[
            "<!doctype html>\n\
             <html lang=\"en\">\n\
             <head>\n\
             <meta charset=\"utf-8\">\n\
             <title>",
            " ",
            "</title>\n\
             </head>\n\
             <body>\n\
             <h1>",
            "</h1>\n\
             </body>\n\
             </html>\n",
        ],
        vec![
            Slot::text(status.as_u16()),
            Slot::text(reason),
            Slot::text(reason),
        ],
    )
}

/// The page for the developer. Everything on it is text someone else wrote, an error
/// message can quote what a user sent, so all of it goes through [`Slot::text`].
fn development_page(failure: &Unexpected) -> Rendered {
    let causes = failure.chain.iter().map(|cause| vec![Slot::text(cause)]);
    let debug = match &failure.debug {
        Some(debug) => {
            let statics = &["<h2>Debug</h2>\n<pre>", "</pre>\n"];
            Slot::template(Rendered::new(0, statics, vec![Slot::text(debug)]))
        }
        None => Slot::text(""),
    };
    Rendered::new(
        0,
        &[
            "<!doctype html>\n\
             <html lang=\"en\">\n\
             <head>\n\
             <meta charset=\"utf-8\">\n\
             <title>Unexpected failure: ",
            "</title>\n\
             </head>\n\
             <body>\n\
             <h1>",
            "</h1>\n\
             <p>at <code>",
            "</code></p>\n\
             <h2>Error chain</h2>\n\
             <ol>",
            "</ol>\n",
            "<p>This is the development error page, chosen in the configuration.</p>\n\
             </body>\n\
             </html>\n",
        ],
        vec![
            Slot::text(failure.kind),
            Slot::text(failure.kind),
            Slot::text(&failure.location),
            Slot::comprehension(0, &["<li><pre>", "</pre></li>"], causes),
            debug,
        ],
    )
}

impl<S> Layer<S> for ErrorPages {
    type Service = ErrorPagesService<S>;

    fn layer(&self, inner: S) -> ErrorPagesService<S> {
        let pages = *self;
        ErrorPagesService { inner, pages }
    }
}

/// The service an [`ErrorPages`] makes of another.
#[derive(Debug, Clone)]
pub struct ErrorPagesService<S> {
    inner: S,
    pages: ErrorPages,
}

impl<S> Service<Request> for ErrorPagesService<S>
where
    S: Service<Request, Response = Response>,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let pages = self.pages;
        // What is left half done by a panic is dropped with the request, and the
        // session, which is shared, takes a poisoned lock as it is.
        let response = catch(AssertUnwindSafe(|| self.inner.call(request)));
        Box::pin(async move {
            let response = match response {
                Ok(response) => {
                    let mut response = std::pin::pin!(response);
                    let poll = |context: &mut Context<'_>| match catch(AssertUnwindSafe(|| {
                        response.as_mut().poll(context)
                    })) {
                        Ok(poll) => poll.map(Ok),
                        Err(panic) => Poll::Ready(Err(panic)),
                    };
                    std::future::poll_fn(poll).await
                }
                Err(panic) => Err(panic),
            };
            match response {
                Ok(response) => Ok(pages.finish(response?)),
                Err(panic) => Ok(pages.finish(panic.into_response())),
            }
        })
    }
}

thread_local! {
    /// Where the last panic of this thread was raised: left by the panic hook for the
    /// Layer, which catches the panic on the thread it was raised on.
    static PANICKED_AT: Cell<Option<String>> = const { Cell::new(None) };
}

/// Makes every panic leave its place in [`PANICKED_AT`] before the hook that was
/// there runs. The payload of a caught panic does not say where it was raised.
fn record_where_panics_are_raised() {
    static HOOKED: Once = Once::new();
    HOOKED.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let location = info.location().map(ToString::to_string);
            // A thread that is ending has nowhere to leave it.
            let _ = PANICKED_AT.try_with(|at| at.set(location));
            previous(info);
        }));
    });
}

/// What `run` returns, or the panic it ended with as an unexpected failure.
fn catch<T>(run: AssertUnwindSafe<impl FnOnce() -> T>) -> Result<T, Unexpected> {
    // A panic raised again with `resume_unwind` passes no hook: without this it would
    // be given the place of the panic before it.
    PANICKED_AT.set(None);
    panic::catch_unwind(run).map_err(|payload| {
        let message = if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "a panic whose payload is not text".to_owned()
        };
        let location = PANICKED_AT.take();
        Unexpected {
            kind: "panic",
            chain: vec![message],
            debug: None,
            location: location.unwrap_or_else(|| "a place the panic hook did not see".to_owned()),
        }
    })
}

#[cfg(test)]
mod chain_tests {
    use super::chain;
    use std::fmt;

    #[derive(Debug)]
    struct Outer(Inner);
    #[derive(Debug)]
    struct Inner;
    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer\nWARN forged line")
        }
    }
    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("inner")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }
    impl std::error::Error for Inner {}

    #[derive(Debug)]
    struct Loop;
    static LOOP: Loop = Loop;
    impl fmt::Display for Loop {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("again")
        }
    }
    impl std::error::Error for Loop {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&LOOP)
        }
    }

    #[test]
    fn a_chain_logs_as_one_escaped_line() {
        let chain = chain(&Outer(Inner));
        assert_eq!(chain, ["outer\nWARN forged line", "inner"]);
        assert!(!format!("{chain:?}").contains('\n'));
    }

    #[test]
    fn a_chain_that_loops_is_cut() {
        assert_eq!(chain(&Loop).len(), 33);
    }
}
