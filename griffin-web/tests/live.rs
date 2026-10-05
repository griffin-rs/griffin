//! The Dead render of a LiveView, through a small app driven as a tower service:
//! an HTTP request in, a response out.

use griffin_web::axum::Router;
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{Request, StatusCode, header};
use griffin_web::html;
use griffin_web::live::{LiveView, SessionToken, Socket, StaticToken, live};
use griffin_web::template::Rendered;
use griffin_web::token::{SigningKey, SigningKeyTooShort, TokenError};
use std::convert::Infallible;
use std::time::Duration;
use tower::ServiceExt as _;

const SECRET: &str = "a test secret, at least thirty-two bytes long";

struct Counter {
    count: i32,
    connected: bool,
}

impl LiveView for Counter {
    type Params = i32;
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(start: i32, socket: &mut Socket) -> Counter {
        Counter {
            count: start,
            connected: socket.connected(),
        }
    }

    fn render(&self) -> Rendered {
        html! { <p data-connected={@connected.to_string()}>Count: {@count}</p> }
    }
}

const TWO_WEEKS: Duration = Duration::from_secs(14 * 24 * 60 * 60);

fn key() -> SigningKey {
    SigningKey::new(SECRET).unwrap()
}

fn app() -> Router {
    Router::new()
        .route("/counter/{start}", live::<Counter, _>())
        .with_state(key())
}

/// The opening tag of the element the client attaches a LiveView to.
fn container(html: &str) -> &str {
    let session = html.find(" data-phx-session=").expect("no container");
    let start = html[..session].rfind('<').unwrap();
    let end = session + html[session..].find('>').unwrap();
    &html[start..=end]
}

fn attribute<'a>(tag: &'a str, name: &str) -> &'a str {
    let start = tag.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
    &tag[start..start + tag[start..].find('"').unwrap()]
}

async fn get(app: Router, path: &str) -> (StatusCode, String) {
    let request = Request::get(path).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_get_returns_the_rendered_live_view_in_an_html_document() {
    let request = Request::get("/counter/41").body(Body::empty()).unwrap();

    let response = app().oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    // The typed route parameter reached mount, which saw the Dead render.
    assert!(
        html.contains("<p data-connected=\"false\">Count: 41</p>"),
        "{html}"
    );
}

#[tokio::test]
async fn the_container_carries_what_the_client_reads() {
    let (_, html) = get(app(), "/counter/1").await;

    let container = container(&html);
    let id = attribute(container, "id");
    assert!(container.starts_with("<div id=\"phx-"), "{container}");
    assert!(container.contains(" data-phx-main "), "{container}");
    let session = attribute(container, "data-phx-session");
    assert_eq!(
        SessionToken::verify(&key(), session, TWO_WEEKS).unwrap().id,
        id
    );
    let static_token = attribute(container, "data-phx-static");
    assert_eq!(
        StaticToken::verify(&key(), static_token, TWO_WEEKS)
            .unwrap()
            .id,
        id
    );
    // The page is the container's content.
    assert!(
        html.contains(&format!(
            "{container}<p data-connected=\"false\">Count: 1</p></div>"
        )),
        "{html}"
    );
}

/// The token with one character of the part before (`0`) or after (`1`) the dot changed.
fn changed(token: &str, part: usize) -> String {
    let at = if part == 0 { 0 } else { token.len() - 1 };
    let other = if token.as_bytes()[at] == b'A' {
        "B"
    } else {
        "A"
    };
    format!("{}{other}{}", &token[..at], &token[at + 1..])
}

#[tokio::test]
async fn a_token_that_this_key_did_not_sign_as_it_stands_is_refused() {
    let (_, html) = get(app(), "/counter/1").await;
    let session = attribute(container(&html), "data-phx-session");
    let (_, other_html) = get(app(), "/counter/1").await;
    let other_session = attribute(container(&other_html), "data-phx-session");
    let (payload, signature) = session.split_once('.').unwrap();
    let (_, other_signature) = other_session.split_once('.').unwrap();
    let other_key = SigningKey::new("another secret, also thirty-two bytes long").unwrap();
    assert_ne!(signature, other_signature);

    let verify = |key: &SigningKey, token: &str| SessionToken::verify(key, token, TWO_WEEKS);

    assert!(verify(&key(), session).is_ok());
    for forged in [
        changed(session, 0),
        changed(session, 1),
        format!("{payload}.{other_signature}"),
        payload.to_owned(),
        format!("{payload}."),
        String::new(),
    ] {
        assert_eq!(
            verify(&key(), &forged),
            Err(TokenError::Invalid),
            "{forged}"
        );
    }
    assert_eq!(verify(&other_key, session), Err(TokenError::Invalid));
}

#[tokio::test]
async fn a_token_is_only_good_for_what_it_was_signed_for() {
    let (_, html) = get(app(), "/counter/1").await;
    let session = attribute(container(&html), "data-phx-session");

    assert_eq!(
        StaticToken::verify(&key(), session, TWO_WEEKS),
        Err(TokenError::Invalid)
    );
}

#[tokio::test]
async fn a_token_older_than_the_allowed_age_is_refused() {
    let (_, html) = get(app(), "/counter/1").await;
    let container = container(&html);

    assert_eq!(
        SessionToken::verify(
            &key(),
            attribute(container, "data-phx-session"),
            Duration::ZERO
        ),
        Err(TokenError::Expired)
    );
    assert_eq!(
        StaticToken::verify(
            &key(),
            attribute(container, "data-phx-static"),
            Duration::ZERO
        ),
        Err(TokenError::Expired)
    );
}

#[tokio::test]
async fn each_render_gets_its_own_container_id() {
    let (_, first) = get(app(), "/counter/1").await;
    let (_, second) = get(app(), "/counter/1").await;

    assert_ne!(
        attribute(container(&first), "id"),
        attribute(container(&second), "id")
    );
}

#[tokio::test]
async fn a_path_whose_parameter_does_not_parse_is_not_found() {
    let (status, html) = get(app(), "/counter/many").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains("data-phx-session"), "{html}");
}

#[tokio::test]
async fn a_live_view_can_be_served_on_a_route_without_parameters() {
    struct Hello;
    impl LiveView for Hello {
        type Params = ();
        type Event = Infallible;
        type Message = Infallible;
        type Error = Infallible;

        async fn mount(_params: (), _socket: &mut Socket) -> Hello {
            Hello
        }

        fn render(&self) -> Rendered {
            html! { <h1>Hello</h1> }
        }
    }
    let app = Router::new()
        .route("/hello", live::<Hello, _>())
        .with_state(key());

    let (status, html) = get(app, "/hello").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<h1>Hello</h1></div>"), "{html}");
}

#[tokio::test]
#[should_panic(expected = "this Socket is not that of a `live::Counter`")]
async fn a_socket_gives_no_handle_to_another_live_view_than_its_own() {
    struct Mistaken;
    impl LiveView for Mistaken {
        type Params = ();
        type Event = Infallible;
        type Message = Infallible;
        type Error = Infallible;

        async fn mount(_params: (), socket: &mut Socket) -> Mistaken {
            let _ = socket.handle::<Counter>();
            Mistaken
        }

        fn render(&self) -> Rendered {
            html! { <p>Mistaken</p> }
        }
    }
    let app = Router::new()
        .route("/mistaken", live::<Mistaken, _>())
        .with_state(key());

    get(app, "/mistaken").await;
}

#[test]
fn the_signing_key_does_not_show_its_secret() {
    assert_eq!(format!("{:?}", key()), "SigningKey(REDACTED)");
}

#[test]
fn a_secret_shorter_than_32_bytes_is_refused() {
    assert_eq!(
        SigningKey::new("thirty-one bytes is not enough.").unwrap_err(),
        SigningKeyTooShort
    );
    assert!(SigningKey::new("thirty-two bytes are just enough").is_ok());
}
