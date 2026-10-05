//! The session: what the server remembers of one browser between requests, kept in an
//! encrypted cookie, so there is nothing to store on the server.
//!
//! [`SessionLayer`] goes in the [`Pipeline`](crate::pipeline::Pipeline) of a Scope.
//! A handler of that Scope then takes the [`Session`] as an argument, and a LiveView
//! reads it at mount with [`Socket::session`](crate::live::Socket::session).
//!
//! ```
//! use griffin_web::axum::Router;
//! use griffin_web::axum::response::Redirect;
//! use griffin_web::axum::routing::{get, post};
//! use griffin_web::pipeline::Pipeline;
//! use griffin_web::router::Scope;
//! use griffin_web::session::{Session, SessionLayer};
//!
//! async fn sign_in(session: Session) -> Redirect {
//!     session.insert("user_id", 7);
//!     Redirect::to("/")
//! }
//!
//! async fn home(session: Session) -> String {
//!     match session.get::<u32>("user_id") {
//!         Some(id) => format!("Welcome back, user {id}"),
//!         None => "Welcome".to_owned(),
//!     }
//! }
//!
//! # let secret = "read this from configuration, not from the source";
//! let browser = Pipeline::new().layer(SessionLayer::new(secret)?);
//! let site = Scope::new("/")
//!     .pipe_through(browser)
//!     .route("/", get(home))
//!     .route("/sign-in", post(sign_in));
//! let app: Router = Router::new().merge(site);
//! # Ok::<(), griffin_web::session::SecretTooShort>(())
//! ```
//!
//! # The cookie
//!
//! The session is the cookie `griffin_session`. Its value is the session as JSON,
//! encrypted and authenticated with AES-256-GCM by the [`cookie`] crate's private jar:
//! base64 of a random 96-bit nonce, the ciphertext and the tag, with the cookie's name
//! as associated data. The browser can neither read nor change it. A cookie that does
//! not decrypt, because it was changed, cut short or made with another key, is no
//! session at all: the request gets an empty one.
//!
//! The key is not the secret itself. It is derived from it with HKDF-SHA256 (RFC 5869)
//! under a label of its own, so it is unrelated to the key that signs tokens
//! ([`SigningKey`](crate::token::SigningKey)) even when both come from one secret.
//!
//! The cookie is sent `HttpOnly` (no script reads it), `SameSite=Lax` (it is not sent
//! with another site's form posts, scripts or sockets), `Secure` (HTTPS only) and
//! `Path=/`. It has no `Max-Age`, so a browser keeps it until it is closed. It is only
//! sent when a handler changed the session.
//!
//! # Flash from a LiveView
//!
//! A LiveView has no response to set a cookie with, so a redirect from one
//! ([`Socket::redirect`](crate::live::Socket::redirect)) hands the flash to the browser
//! as a token, which the Phoenix client keeps in the cookie `__phoenix_flash__` for the
//! page it loads. The token is signed by this layer, with a key derived from its secret
//! under a label of its own, and good for a minute (the same machinery as the render
//! tokens, [`SigningKey`]). The layer reads it from the next request, and puts what it
//! holds in the session, where the layout of the page shows it once. A token that is
//! not this layer's, was changed, or is too old is no flash: a browser cannot make a
//! message up. The cookie is dropped by that response, genuine or not. A token can be
//! shown again from a copy of the cookie within the minute, to the one who has it.
//!
//! # Limits
//!
//! - Everything in the session travels with every request, and a browser stores no
//!   more than 4096 bytes per cookie. A response that would set a larger one is
//!   replaced by a 500. Keep an id in the session, not the record.
//! - A LiveView reads the session as it was when its page connected.
//! - Nothing ends a session on the server: a copy of the cookie is good until the
//!   secret changes.

use crate::token::SigningKey;
use axum::Extension;
use axum::extract::rejection::ExtensionRejection;
use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cookie::{Cookie, CookieJar, Key, SameSite};
use hkdf::Hkdf;
use serde::de::DeserializeOwned;
use serde::{Deserialize as _, Serialize};
use serde_json::{Map, Value};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fmt;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;
use subtle::ConstantTimeEq as _;
use tower_layer::Layer;
use tower_service::Service;

const COOKIE_NAME: &str = "griffin_session";

/// Where the session keeps the flash. An application's own keys must not start with
/// an underscore: those are Griffin's.
const FLASH_KEY: &str = "_flash";

/// Where the session keeps the random bytes its CSRF tokens are made of, as base64.
const CSRF_KEY: &str = "_csrf_token";

/// 256 bits: twice what a value that must not be guessed needs.
const CSRF_BYTES: usize = 32;

/// The cookie the Phoenix client leaves a flash token in when a LiveView redirects
/// with flash. A script of the browser's sets it, so it is the browser's word: only a
/// token this layer signed less than a minute ago is read from it.
const FLASH_COOKIE: &str = "__phoenix_flash__";

/// What a flash token is signed for.
const FLASH_PURPOSE: &str = "live flash";

/// How long a flash token is good for: the cookie it travels in lives as long.
const FLASH_MAX_AGE: Duration = Duration::from_secs(60);

/// What a browser is sure to store for one cookie, attributes included (RFC 6265).
const MAX_COOKIE_BYTES: usize = 4096;

/// The session of the request: values by name, kept between the requests of one
/// browser. A handler takes it as an argument, on a route whose Scope is piped through
/// a [`SessionLayer`]. See the [module's example](self).
///
/// A clone is the same session. `Session::default()` is an empty one that belongs to
/// no request, for calling a handler in a test. `Debug` does not print the values.
#[derive(Clone, Default)]
pub struct Session(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    values: Map<String, Value>,
    /// Whether the cookie has to be sent again.
    changed: bool,
    /// What signs the flash a LiveView sends with a redirect: the layer's. A session
    /// no layer gave has none.
    flash_key: Option<SigningKey>,
}

impl Session {
    /// The value under `key`, or `None` if there is none or it is not a `T`.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        T::deserialize(self.state().values.get(key)?).ok()
    }

    /// Puts `value` under `key`, in place of what was there.
    ///
    /// # Panics
    ///
    /// If `value` does not serialize to JSON, as a map whose keys are not strings.
    pub fn insert(&self, key: &str, value: impl Serialize) {
        let value = serde_json::to_value(value).expect("a session value serializes to JSON");
        let mut state = self.state();
        state.values.insert(key.to_owned(), value);
        state.changed = true;
    }

    /// Takes away the value under `key`.
    pub fn remove(&self, key: &str) {
        let mut state = self.state();
        state.changed |= state.values.remove(key).is_some();
    }

    /// Takes away every value, as signing out does, the CSRF token among them: the
    /// pages and sockets opened before are then refused, and the next page gets a new
    /// token. The browser is told to drop the cookie.
    pub fn clear(&self) {
        let mut state = self.state();
        state.changed |= !state.values.is_empty();
        state.values.clear();
    }

    /// Leaves `message` as the flash of the kind `kind` (`"info"`, `"error"`), in
    /// place of one of that kind already there. It is kept in the session until it is
    /// shown, so it is there on the page a redirect leads to. See [`Flash`].
    pub fn put_flash(&self, kind: &str, message: impl Into<String>) {
        let mut state = self.state();
        let flash = state.values.entry(FLASH_KEY).or_insert(Value::Null);
        if !flash.is_object() {
            *flash = Value::Object(Map::new());
        }
        flash[kind] = Value::String(message.into());
        state.changed = true;
    }

    /// Takes the flash out of the session: it is read once. The layout of a Scope does
    /// this for the pages it wraps, so a handler only calls it to show flash itself.
    pub fn take_flash(&self) -> Flash {
        let mut state = self.state();
        let Some(flash) = state.values.remove(FLASH_KEY) else {
            return Flash::default();
        };
        state.changed = true;
        Flash(BTreeMap::deserialize(flash).unwrap_or_default())
    }

    /// The flash a LiveView leaves with a redirect, signed for the browser to bring
    /// back as the cookie [`SessionLayer`] reads. `None` for a session no layer gave,
    /// which has nothing to sign with.
    pub(crate) fn sign_flash(&self, flash: &BTreeMap<String, String>) -> Option<String> {
        let state = self.state();
        Some(state.flash_key.as_ref()?.sign(FLASH_PURPOSE, flash))
    }

    /// The CSRF token of this session, to send to the browser: in a hidden field
    /// `_csrf_token` of a form, or for a script to send back in the header
    /// `x-csrf-token`. [`CsrfLayer`](crate::security::CsrfLayer) refuses a
    /// state-changing request that does not bring it back.
    ///
    /// The session keeps 32 random bytes from the operating system, made the first
    /// time this is called, which sends the cookie. The browser never sees them as
    /// they are: each call gives them masked with a fresh one-time pad,
    /// base64url of `pad | secret XOR pad`, so no two calls give the same text and a
    /// compressed response gives nothing away about them (BREACH). Every one of
    /// these is good for as long as the session keeps its bytes:
    /// [`clear`](Session::clear) drops them, and the next call makes new ones.
    pub fn csrf_token(&self) -> String {
        let secret = {
            let mut state = self.state();
            match csrf_secret(&state.values) {
                Some(secret) => secret,
                None => {
                    let secret = random_bytes();
                    let text = Value::String(URL_SAFE_NO_PAD.encode(secret));
                    state.values.insert(CSRF_KEY.to_owned(), text);
                    state.changed = true;
                    secret
                }
            }
        };
        let pad = random_bytes();
        let masked: [u8; CSRF_BYTES] = std::array::from_fn(|at| secret[at] ^ pad[at]);
        URL_SAFE_NO_PAD.encode([pad, masked].concat())
    }

    /// Whether `token` is one [`csrf_token`](Session::csrf_token) gave for this
    /// session. A session that was never asked for a token has none that matches.
    pub(crate) fn csrf_token_matches(&self, token: &str) -> bool {
        let Some(secret) = csrf_secret(&self.state().values) else {
            return false;
        };
        let Ok(sent) = URL_SAFE_NO_PAD.decode(token) else {
            return false;
        };
        if sent.len() != 2 * CSRF_BYTES {
            return false;
        }
        let (pad, masked) = sent.split_at(CSRF_BYTES);
        let unmasked: Vec<u8> = masked.iter().zip(pad).map(|(m, p)| m ^ p).collect();
        // In constant time: how long a wrong token takes says nothing about the
        // right one.
        unmasked.ct_eq(&secret).into()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while it was held left the values as they were: carry on.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The random bytes a session's CSRF tokens are made of, if it has them.
fn csrf_secret(values: &Map<String, Value>) -> Option<[u8; CSRF_BYTES]> {
    let text = values.get(CSRF_KEY)?.as_str()?;
    URL_SAFE_NO_PAD.decode(text).ok()?.try_into().ok()
}

/// Bytes from the operating system's random number generator.
fn random_bytes() -> [u8; CSRF_BYTES] {
    let mut bytes = [0; CSRF_BYTES];
    getrandom::getrandom(&mut bytes).expect("the operating system gives random bytes");
    bytes
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session(REDACTED)")
    }
}

/// One-time messages for the user, by kind: what [`Session::put_flash`] left, taken
/// out of the session to be shown once.
///
/// A handler leaves a message and redirects. The page the browser lands on is
/// rendered in the layout of its Scope, which is given the flash and shows it, and
/// the page after that has none. A response that is not a page in a layout, such as
/// another redirect or JSON, leaves the flash where it is.
///
/// ```
/// use griffin_web::axum::response::Redirect;
/// use griffin_web::axum::routing::{get, post};
/// use griffin_web::html;
/// use griffin_web::router::Scope;
/// use griffin_web::session::Session;
///
/// async fn save(session: Session) -> Redirect {
///     session.put_flash("info", "Saved");
///     Redirect::to("/")
/// }
///
/// let site: Scope = Scope::new("/")
///     .layout(|page, flash| {
///         html! {
///             <main>
///                 <p :if={flash.get("info").is_some()} class="info">{flash.get("info").unwrap()}</p>
///                 {page}
///             </main>
///         }
///     })
///     .route("/", get(|| async { html! { <h1>Home</h1> } }))
///     .route("/save", post(save));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flash(BTreeMap<String, String>);

impl Flash {
    /// The message of the kind `kind`, if one was left.
    pub fn get(&self, kind: &str) -> Option<&str> {
        self.0.get(kind).map(String::as_str)
    }
}

/// The session comes from the request's extensions, where [`SessionLayer`] left it.
/// Without that Layer the rejection is axum's for a missing extension: a 500.
impl<S: Send + Sync> FromRequestParts<S> for Session {
    type Rejection = ExtensionRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Session, Self::Rejection> {
        let Extension(session) = Extension::from_request_parts(parts, state).await?;
        Ok(session)
    }
}

/// The Layer that gives each request its [`Session`]: it reads the cookie before the
/// handler runs and sends it again afterwards if the session changed. See the
/// [module's documentation](self) for the cookie.
#[derive(Debug, Clone)]
pub struct SessionLayer {
    /// `Debug` does not print it.
    key: Key,
    /// Signs and verifies the flash tokens of redirects from a LiveView.
    flash_key: SigningKey,
    secure: bool,
}

impl SessionLayer {
    /// A Layer whose key is derived from `secret`, which must be at least 32 bytes of
    /// random data read at run time (configuration, an environment variable). Everyone
    /// who has the secret can read and forge sessions.
    pub fn new(secret: impl AsRef<[u8]>) -> Result<SessionLayer, SecretTooShort> {
        let secret = secret.as_ref();
        if secret.len() < 32 {
            return Err(SecretTooShort);
        }
        // Extract, then expand under a label that nothing else uses: the key shares
        // nothing with other keys made from the same secret.
        let mut key = [0; 64];
        Hkdf::<Sha256>::new(None, secret)
            .expand(b"griffin_web cookie session", &mut key)
            .expect("64 bytes is a length HKDF-SHA256 can give");
        let key = Key::from(&key);
        // Another label, so that this key is unrelated to the cookie's.
        let mut flash_key = [0; 32];
        Hkdf::<Sha256>::new(None, secret)
            .expand(b"griffin_web flash token", &mut flash_key)
            .expect("32 bytes is a length HKDF-SHA256 can give");
        let flash_key = SigningKey::new(flash_key).expect("32 bytes is long enough");
        Ok(SessionLayer {
            key,
            flash_key,
            secure: true,
        })
    }

    /// Whether the cookie is `Secure`, which it is unless this is called with `false`.
    /// A browser sends a `Secure` cookie over HTTPS only, and most make an exception
    /// for `localhost`. Pass `false` only to develop over plain HTTP where that
    /// exception does not hold.
    pub fn secure(mut self, secure: bool) -> SessionLayer {
        self.secure = secure;
        self
    }

    /// The session a request's cookies hold: an empty one unless one of them is a
    /// session cookie this key encrypted.
    fn read(&self, headers: &HeaderMap) -> (Session, bool) {
        let cookies = |name: &'static str| {
            let cookies = headers.get_all(header::COOKIE).into_iter();
            let cookies = cookies
                .filter_map(|header| header.to_str().ok())
                .flat_map(Cookie::split_parse)
                .filter_map(Result::ok);
            cookies.filter(move |cookie| cookie.name() == name)
        };
        let values = cookies(COOKIE_NAME).find_map(|cookie| self.open(cookie.into_owned()));
        let state = State {
            values: values.unwrap_or_default(),
            changed: false,
            flash_key: Some(self.flash_key.clone()),
        };
        let session = Session(Arc::new(Mutex::new(state)));
        // A token that is not this layer's, or too old, is no flash, and the cookie is
        // spent either way: a client cannot make a flash up, and sees one once.
        let mut spent = false;
        for cookie in cookies(FLASH_COOKIE) {
            spent = true;
            let flash = self.flash_key.verify::<BTreeMap<String, String>>(
                FLASH_PURPOSE,
                cookie.value(),
                FLASH_MAX_AGE,
            );
            for (kind, message) in flash.into_iter().flatten() {
                session.put_flash(&kind, message);
            }
        }
        (session, spent)
    }

    /// The values in a session cookie, if this key encrypted it.
    fn open(&self, cookie: Cookie<'static>) -> Option<Map<String, Value>> {
        // Nothing of the value is read unless it authenticates.
        let cookie = CookieJar::new().private(&self.key).decrypt(cookie)?;
        serde_json::from_str(cookie.value()).ok()
    }

    /// Sends the session with `response`, if it changed.
    fn write(&self, session: &Session, response: &mut Response) {
        let state = session.state();
        if !state.changed {
            return;
        }
        let json = Value::Object(state.values.clone()).to_string();
        let mut cookie = Cookie::build((COOKIE_NAME, json))
            .path("/")
            .http_only(true)
            .same_site(SameSite::Lax)
            .secure(self.secure)
            .build();
        if state.values.is_empty() {
            cookie.make_removal();
        } else {
            let mut jar = CookieJar::new();
            jar.private_mut(&self.key).add(cookie);
            cookie = jar.get(COOKIE_NAME).expect("it was just added").clone();
        }
        let cookie = cookie.to_string();
        // A browser would drop it without a word, and the session with it.
        if cookie.len() > MAX_COOKIE_BYTES {
            *response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
            return;
        }
        let cookie = HeaderValue::from_str(&cookie).expect("base64 and attributes are ASCII");
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
}

impl<S> Layer<S> for SessionLayer {
    type Service = SessionService<S>;

    fn layer(&self, inner: S) -> SessionService<S> {
        let layer = self.clone();
        SessionService { inner, layer }
    }
}

/// The service a [`SessionLayer`] makes of another.
#[derive(Debug, Clone)]
pub struct SessionService<S> {
    inner: S,
    layer: SessionLayer,
}

impl<S> Service<Request> for SessionService<S>
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

    fn call(&mut self, mut request: Request) -> Self::Future {
        let (session, spent) = self.layer.read(request.headers());
        request.extensions_mut().insert(session.clone());
        let response = self.inner.call(request);
        let layer = self.layer.clone();
        Box::pin(async move {
            let mut response = response.await?;
            layer.write(&session, &mut response);
            if spent {
                // The flash is in the session now: the browser is to drop the cookie.
                let mut cookie = Cookie::build((FLASH_COOKIE, "")).path("/").build();
                cookie.make_removal();
                let cookie = HeaderValue::from_str(&cookie.to_string());
                response
                    .headers_mut()
                    .append(header::SET_COOKIE, cookie.expect("ASCII"));
            }
            Ok(response)
        })
    }
}

/// The secret given to [`SessionLayer::new`] was shorter than 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretTooShort;

impl fmt::Display for SecretTooShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the session secret must be at least 32 bytes long")
    }
}

impl std::error::Error for SecretTooShort {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::now_ms;

    /// The flash cookie is good for a minute: a genuine token a little older is no
    /// flash. (The network tests cannot make an old token: the key is the layer's own.)
    #[test]
    fn a_flash_token_older_than_a_minute_is_no_flash() {
        let layer = SessionLayer::new("a test secret, at least thirty-two bytes long").unwrap();
        let flash = BTreeMap::from([("info".to_owned(), "Saved".to_owned())]);
        let cookie_of = |issued_at: u64| {
            let token = layer.flash_key.sign_at(FLASH_PURPOSE, &flash, issued_at);
            let mut headers = HeaderMap::new();
            let cookie = format!("{FLASH_COOKIE}={token}").parse().unwrap();
            headers.insert(header::COOKIE, cookie);
            headers
        };

        let (fresh, _) = layer.read(&cookie_of(now_ms() - 59_000));
        let (old, spent) = layer.read(&cookie_of(now_ms() - 61_000));

        assert_eq!(fresh.take_flash().get("info"), Some("Saved"));
        assert_eq!(old.take_flash(), Flash::default());
        // The cookie is spent all the same.
        assert!(spent);
    }
}
