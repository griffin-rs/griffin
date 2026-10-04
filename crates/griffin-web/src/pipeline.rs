//! Pipelines: the Layers every request of a Scope passes through.
//!
//! A Layer is tower's own (Griffin does not duplicate upstream API surface), and any Layer that axum's
//! [`Router::layer`] takes goes in a [`Pipeline`]. A Pipeline is a plain value, so an
//! application names one by writing a function that returns it, and gives it to a
//! Scope with [`Scope::pipe_through`](crate::router::Scope::pipe_through).
//!
//! Layers run top to bottom, in the order written, which is the reverse of chained
//! calls to [`Router::layer`]. A request passes `session` before `log` here, and the
//! response comes back the other way:
//!
//! ```
//! use griffin_web::axum::extract::Request;
//! use griffin_web::axum::middleware::{Next, from_fn};
//! use griffin_web::axum::response::Response;
//! use griffin_web::axum::routing::get;
//! use griffin_web::axum::Router;
//! use griffin_web::pipeline::Pipeline;
//! use griffin_web::router::Scope;
//! use griffin_web::session::SessionLayer;
//!
//! async fn log(request: Request, next: Next) -> Response {
//!     println!("{} {}", request.method(), request.uri());
//!     next.run(request).await
//! }
//!
//! fn browser(session: SessionLayer) -> Pipeline {
//!     Pipeline::new().layer(session).layer(from_fn(log))
//! }
//!
//! # let secret = "read this from configuration, not from the source";
//! let site = Scope::new("/")
//!     .pipe_through(browser(SessionLayer::new(secret)?))
//!     .route("/", get(|| async { "Welcome" }));
//! let app: Router = Router::new().merge(site);
//! # Ok::<(), griffin_web::session::SecretTooShort>(())
//! ```

use crate::security::{CsrfLayer, SecureHeadersLayer};
use crate::session::SessionLayer;
use axum::Router;
use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::Route;
use std::convert::Infallible;
use tower_layer::Layer;
use tower_service::Service;

/// An ordered list of Layers. `S` is the state of the router it is for.
///
/// See the [module's example](self).
#[must_use = "a Pipeline does nothing until a Scope is piped through it"]
pub struct Pipeline<S = ()> {
    /// Each puts one Layer on a router, kept in the order written.
    layers: Vec<AddLayer<S>>,
}

type AddLayer<S> = Box<dyn FnOnce(Router<S>) -> Router<S> + Send>;

impl<S: Clone + Send + Sync + 'static> Pipeline<S> {
    /// A Pipeline with no Layers.
    pub fn new() -> Pipeline<S> {
        Pipeline { layers: Vec::new() }
    }

    /// The Pipeline for the pages a browser visits, with the security defaults on:
    ///
    /// 1. [`SecureHeadersLayer`], first, so that every response has the headers, a
    ///    refusal too;
    /// 2. `session`, which gives each request its [`Session`](crate::session::Session)
    ///    and with it the flash;
    /// 3. [`CsrfLayer`], which refuses a state-changing request without the session's
    ///    token.
    ///
    /// An application's own Layers go after these, with [`layer`](Pipeline::layer).
    /// One that wants a default changed writes the three calls itself and changes
    /// that one, where it can be read and searched for:
    ///
    /// ```
    /// use griffin_web::axum::http::header::REFERRER_POLICY;
    /// use griffin_web::pipeline::Pipeline;
    /// use griffin_web::security::{CsrfLayer, SecureHeadersLayer};
    /// use griffin_web::session::SessionLayer;
    ///
    /// fn browser(session: SessionLayer) -> Pipeline {
    ///     Pipeline::new()
    ///         .layer(SecureHeadersLayer::new().without(REFERRER_POLICY))
    ///         .layer(session)
    ///         .layer(CsrfLayer::new())
    /// }
    /// ```
    pub fn browser(session: SessionLayer) -> Pipeline<S> {
        Pipeline::new()
            .layer(SecureHeadersLayer::new())
            .layer(session)
            .layer(CsrfLayer::new())
    }

    /// Adds a Layer after the ones already there: a request reaches it after them.
    ///
    /// It takes what [`Router::layer`] takes, so a Layer that does not fit is a
    /// compile error at this call. A Layer whose service can fail needs axum's
    /// [`HandleErrorLayer`](axum::error_handling::HandleErrorLayer) before it.
    pub fn layer<L>(mut self, layer: L) -> Pipeline<S>
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.layers.push(Box::new(|router| router.layer(layer)));
        self
    }

    /// Adds the Layers of `other` after the ones already there.
    pub(crate) fn extend(&mut self, other: Pipeline<S>) {
        self.layers.extend(other.layers);
    }

    /// Puts the Layers on `router`. The last Layer put on a router is the first a
    /// request passes, so they go on bottom to top.
    pub(crate) fn apply(self, router: Router<S>) -> Router<S> {
        let layers = self.layers.into_iter().rev();
        layers.fold(router, |router, add_layer| add_layer(router))
    }
}

impl<S: Clone + Send + Sync + 'static> Default for Pipeline<S> {
    fn default() -> Pipeline<S> {
        Pipeline::new()
    }
}
