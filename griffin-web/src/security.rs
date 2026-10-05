//! The Layers that harden the pages a browser visits. Both are in
//! [`Pipeline::browser`](crate::pipeline::Pipeline::browser), and an application that
//! wants one of them changed builds its own [`Pipeline`](crate::pipeline::Pipeline)
//! and says so there. They answer to a
//! visitor's browser running a page of another site: forged state-changing requests
//! (CSRF) and responses that a browser may frame or
//! sniff.
//!
//! The socket of the LiveViews has its own checks: see
//! [`SocketChecks`](crate::live::SocketChecks).

use crate::html;
use crate::session::Session;
use crate::template::Rendered;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use std::pin::Pin;
use std::task::{Context, Poll};
use tower_layer::Layer;
use tower_service::Service;

/// The hidden field that carries the CSRF token of `session` in a form, for the
/// [`CsrfLayer`] to find when the form is posted: `{csrf_field(&session)}` inside a
/// `<form method="post">`. Every render writes a token masked afresh, as
/// [`Session::csrf_token`] does.
///
/// ```
/// use griffin_web::html;
/// use griffin_web::security::csrf_field;
/// use griffin_web::session::Session;
///
/// let session = Session::default();
/// let form = html! { <form method="post" action="/name">{csrf_field(&session)}</form> };
/// assert!(form.to_html().contains(r#"<input type="hidden" name="_csrf_token" value=""#));
/// ```
pub fn csrf_field(session: &Session) -> Rendered {
    let token = session.csrf_token();
    html! { <input type="hidden" name={CSRF_FIELD} value={token}> }
}

/// The Layer that puts the secure response headers on every response that passes it:
///
/// | Header | Value | What it does |
/// |---|---|---|
/// | `content-security-policy` | `base-uri 'self'; frame-ancestors 'self'` | No other site frames the page (clickjacking), and no injected `<base>` moves its relative URLs |
/// | `referrer-policy` | `strict-origin-when-cross-origin` | Another site is told the origin a visitor came from, not the path and query |
/// | `x-content-type-options` | `nosniff` | The browser does not guess a type other than the one sent |
/// | `x-permitted-cross-domain-policies` | `none` | Flash and PDF clients load nothing across domains |
///
/// These are the headers of Phoenix's `put_secure_browser_headers`. A header the
/// response already has is left as it is, so a handler that sets its own, a stricter
/// `content-security-policy` say, overrides the default for its response. Leaving a
/// default out for a whole Pipeline takes a call to [`without`](Self::without).
///
/// The policy says nothing about where scripts and styles may come from: that depends
/// on the application, which sets its own. `strict-transport-security` is not here
/// either: it belongs to whatever terminates TLS, and sent by a server on `localhost`
/// it would move every other project on that machine to HTTPS.
#[derive(Debug, Clone)]
pub struct SecureHeadersLayer {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SecureHeadersLayer {
    /// The Layer with every default header.
    pub fn new() -> SecureHeadersLayer {
        let header = |name, value| {
            let name = HeaderName::from_static(name);
            (name, HeaderValue::from_static(value))
        };
        let headers = vec![
            header(
                "content-security-policy",
                "base-uri 'self'; frame-ancestors 'self'",
            ),
            header("referrer-policy", "strict-origin-when-cross-origin"),
            header("x-content-type-options", "nosniff"),
            header("x-permitted-cross-domain-policies", "none"),
        ];
        SecureHeadersLayer { headers }
    }

    /// Leaves the default header `name` out: the responses are sent without it unless
    /// a handler sets it.
    pub fn without(mut self, name: HeaderName) -> SecureHeadersLayer {
        self.headers.retain(|(default, _)| *default != name);
        self
    }
}

impl Default for SecureHeadersLayer {
    fn default() -> SecureHeadersLayer {
        SecureHeadersLayer::new()
    }
}

impl<S> Layer<S> for SecureHeadersLayer {
    type Service = SecureHeadersService<S>;

    fn layer(&self, inner: S) -> SecureHeadersService<S> {
        let headers = self.headers.clone();
        SecureHeadersService { inner, headers }
    }
}

/// The service a [`SecureHeadersLayer`] makes of another.
#[derive(Debug, Clone)]
pub struct SecureHeadersService<S> {
    inner: S,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl<S> Service<Request> for SecureHeadersService<S>
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
        let response = self.inner.call(request);
        let headers = self.headers.clone();
        Box::pin(async move {
            let mut response = response.await?;
            for (name, value) in headers {
                response.headers_mut().entry(name).or_insert(value);
            }
            Ok(response)
        })
    }
}

/// The field of a form that carries the CSRF token.
const CSRF_FIELD: &str = "_csrf_token";

/// The header a script sends the CSRF token in.
const CSRF_HEADER: &str = "x-csrf-token";

/// The Layer that refuses a forged request (CSRF): one another site made the browser
/// send, with the session cookie the browser adds by itself.
///
/// Every request whose method is not safe (anything but `GET`, `HEAD`, `OPTIONS` and
/// `TRACE`) must bring back the token of its session,
/// [`Session::csrf_token`](crate::session::Session::csrf_token), which a page of
/// another site cannot read. It is looked for in the header `x-csrf-token`, then in
/// the field `_csrf_token` of a form (`application/x-www-form-urlencoded`). A request
/// without it, with one of another session, or with no session at all is answered
/// `403 Forbidden` and does not reach its handler. So a handler of a safe method must
/// not change anything: nothing protects it.
///
/// It goes after a [`SessionLayer`](crate::session::SessionLayer). There is no switch
/// to turn it off for a route: routes that take no cookie, such as an API with its own
/// credentials, go in a Scope with another Pipeline.
///
/// ```
/// use griffin_web::html;
/// use griffin_web::session::Session;
/// use griffin_web::template::Rendered;
///
/// async fn edit_name(session: Session) -> Rendered {
///     let csrf_token = session.csrf_token();
///     html! {
///         <form method="post" action="/name">
///             <input type="hidden" name="_csrf_token" value={csrf_token}>
///             <input name="name">
///             <button>Save</button>
///         </form>
///     }
/// }
/// ```
///
/// # Limits
///
/// - A form is read up to 1 MiB to find the token, the bound of
///   [`FormParams`](crate::live::FormParams). A larger one is answered
///   `413 Payload Too Large`.
/// - No pairs are kept, only the token is looked for, so the byte limit is the whole
///   bound: the token may come after any number of pairs.
/// - A `multipart/form-data` body is not looked into: send the token in the header.
#[derive(Debug, Clone, Copy, Default)]
pub struct CsrfLayer;

impl CsrfLayer {
    /// The Layer. It has no settings.
    pub fn new() -> CsrfLayer {
        CsrfLayer
    }
}

impl<S> Layer<S> for CsrfLayer {
    type Service = CsrfService<S>;

    fn layer(&self, inner: S) -> CsrfService<S> {
        CsrfService { inner }
    }
}

/// The service a [`CsrfLayer`] makes of another.
#[derive(Debug, Clone)]
pub struct CsrfService<S> {
    inner: S,
}

impl<S> Service<Request> for CsrfService<S>
where
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let safe = [Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE];
        if safe.contains(request.method()) {
            return Box::pin(self.inner.call(request));
        }
        // The one that was polled ready is the one to call.
        let ready = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, ready);
        Box::pin(async move {
            match genuine(request).await {
                Ok(request) => inner.call(request).await,
                Err(refusal) => Ok(refusal.into_response()),
            }
        })
    }
}

/// The request, if it brings the CSRF token of its session, and else the status that
/// refuses it.
async fn genuine(request: Request) -> Result<Request, StatusCode> {
    let (method, path) = (request.method().clone(), request.uri().path().to_owned());
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        // A mistake in the application, and nothing a browser can do about.
        tracing::error!(
            %method,
            path,
            "CsrfLayer found no session and refused the request: put it after a SessionLayer"
        );
        return Err(StatusCode::FORBIDDEN);
    };
    let (request, token) = sent_token(request).await?;
    if token.is_some_and(|token| session.csrf_token_matches(&token)) {
        return Ok(request);
    }
    // Neither token is logged: the one sent may be another session's.
    tracing::warn!(
        %method,
        path,
        "a request was refused: its CSRF token is missing or is not its session's"
    );
    Err(StatusCode::FORBIDDEN)
}

/// The CSRF token a request carries, if any, and the request with its body as it was.
async fn sent_token(request: Request) -> Result<(Request, Option<String>), StatusCode> {
    if let Some(token) = request.headers().get(CSRF_HEADER) {
        let token = token.to_str().ok().map(str::to_owned);
        return Ok((request, token));
    }
    let content_type = request.headers().get(header::CONTENT_TYPE);
    let content_type = content_type.and_then(|value| value.to_str().ok());
    let media_type = content_type.and_then(|value| value.split(';').next());
    let form = "application/x-www-form-urlencoded";
    if !media_type.is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case(form)) {
        return Ok((request, None));
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, crate::live::FORM_MAX_BYTES).await else {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    };
    let mut fields = form_urlencoded::parse(&bytes);
    let token = fields.find(|(name, _)| name == CSRF_FIELD);
    let token = token.map(|(_, token)| token.into_owned());
    Ok((Request::from_parts(parts, Body::from(bytes)), token))
}
