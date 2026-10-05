//! The LiveView example through a real socket: the Dead render of `/signup`, then a join
//! with the tokens in it, then a change Event, and the diff that comes back.

use futures_util::{SinkExt as _, StreamExt as _};
use griffin::axum;
use griffin::axum::body::{Body, to_bytes};
use griffin::axum::http::{Request, header};
use griffin::config::Secret;
use griffin::error::ErrorPage;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tower::ServiceExt as _;

use __app___web::app;
use __app___web::config::Config;

fn router() -> axum::Router {
    app(&Config {
        secret_key_base: Secret::new("a secret for tests only: not used anywhere else"),
        port: 0,
        error_page: ErrorPage::Plain,
        secure_cookie: true,
    })
    .expect("the test configuration is complete")
}

/// The value of the attribute `name` in the page's HTML.
fn attribute(html: &str, name: &str) -> String {
    let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
    html[start..start + html[start..].find('"').unwrap()].to_owned()
}

#[tokio::test]
async fn the_signup_live_view_joins_and_answers_a_change_with_its_errors() {
    let app = router();
    // The Dead render: the page, its tokens, and the session cookie.
    let page = app
        .clone()
        .oneshot(Request::get("/signup").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let cookie = page.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html =
        String::from_utf8(to_bytes(page.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    let (id, session, statics, csrf) = (
        attribute(&html, "id"),
        attribute(&html, "data-phx-session"),
        attribute(&html, "data-phx-static"),
        attribute(&html, "data-csrf-token"),
    );
    let topic = format!("lv:{id}");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // As the Phoenix client opens it: the CSRF token in the query, the session cookie and
    // the page's own origin.
    let mut request = format!("ws://{address}/live/websocket?vsn=2.0.0&_csrf_token={csrf}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    request
        .headers_mut()
        .insert(header::ORIGIN, format!("http://{address}").parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();

    let mut exchange = async |frame: Value| -> Value {
        socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
        let reply = socket.next().await.expect("a reply").unwrap();
        let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();
        assert_eq!(reply[3], "phx_reply", "{reply}");
        assert_eq!(reply[4]["status"], "ok", "{reply}");
        reply[4]["response"].clone()
    };

    let joined = exchange(json!(["1", "1", topic, "phx_join", {
        "url": format!("http://{address}/signup"),
        "params": {"_mounts": 0, "_mount_attempts": 0},
        "session": session,
        "static": statics,
        "sticky": false,
    }]))
    .await;
    assert_eq!(joined["liveview_version"], "1.2.12");
    assert!(joined["rendered"].to_string().contains("Sign up"));

    // The user types a bad email: the Changeset's stages one and two run, with no I/O.
    let changed = exchange(json!(["1", "2", topic, "event", {
        "type": "form",
        "event": "change",
        "value": "signup%5Bemail%5D=nope&signup%5Busername%5D=&_target=signup%5Bemail%5D",
    }]))
    .await;
    let diff = changed["diff"].to_string();
    assert!(
        diff.contains("must be an address like name@example.com"),
        "{diff}"
    );
}
