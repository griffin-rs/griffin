//! The `routes!` table, through the app it becomes: the same requests are sent to the
//! table's router and to the builder code the table stands for.

use futures_util::{SinkExt as _, StreamExt as _};
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use griffin_web::axum::response::{Redirect, Response};
use griffin_web::axum::routing::{get, put};
use griffin_web::axum::{self, Json, Router};
use griffin_web::live::{LiveView, Socket, live, live_socket};
use griffin_web::pipeline::Pipeline;
use griffin_web::router::{Path, Route, Scope};
use griffin_web::session::{Flash, SessionLayer};
use griffin_web::template::{Rendered, SlotEntries};
use griffin_web::token::SigningKey;
use griffin_web::{component, html, routes};
use serde_json::{Value, json};
use std::convert::Infallible;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tower::ServiceExt as _;
use tower::util::MapResponseLayer;

#[component]
fn Site(#[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <html><body class="site">{inner_block}</body></html> }
}

/// A layout is a function of the page and the flash.
fn site(page: Rendered, _flash: &Flash) -> Rendered {
    html! { <Site>{page}</Site> }
}

fn back_office(page: Rendered, _flash: &Flash) -> Rendered {
    html! { <html><body class="admin">{page}</body></html> }
}

/// A Pipeline whose one Layer adds `name` to the `x-through` headers of the response.
fn stamp(name: &'static str) -> Pipeline<SigningKey> {
    Pipeline::new().layer(MapResponseLayer::new(move |mut response: Response| {
        let value = HeaderValue::from_static(name);
        response.headers_mut().append("x-through", value);
        response
    }))
}

/// A Pipeline with the session in it, which the socket of a LiveView asks for: it is
/// opened with the session's CSRF token.
fn session() -> Pipeline<SigningKey> {
    Pipeline::new().layer(SessionLayer::new(SECRET).unwrap())
}

async fn home() -> Rendered {
    html! { <h1>Welcome</h1> }
}

async fn show_order(Path(id): Path<u32>) -> Rendered {
    html! { <h1>Order {id}</h1> }
}

async fn rename_order(Path(id): Path<u32>, Json(name): Json<String>) -> Rendered {
    html! { <h1>Order {id} is now of {name}</h1> }
}

/// A page that links to others by their Path helpers.
async fn orders() -> Rendered {
    let slug = "a b";
    html! { <a href={Routes::order(7)}>Order 7</a><a href={Routes::admin_post(7, slug)}>A post</a> }
}

async fn latest_order() -> Redirect {
    Redirect::to(&Routes::order(7))
}

async fn dashboard() -> Rendered {
    html! { <h1>Dashboard</h1> }
}

/// The `Path` is the last of its arguments, and the parameters are two.
async fn show_post(headers: HeaderMap, Path((user, slug)): Path<(u32, String)>) -> Rendered {
    let agent = headers[header::USER_AGENT].to_str().unwrap_or_default();
    html! { <h1>Post {slug} of user {user}, for {agent}</h1> }
}

async fn file(Path(path): Path<String>) -> String {
    path
}

/// A handler is named by its path.
mod api {
    use super::*;

    pub async fn health() -> Json<Value> {
        Json(json!({"ok": true}))
    }
}

struct Counter {
    count: i32,
}

impl LiveView for Counter {
    type Params = i32;
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(start: i32, _socket: &mut Socket) -> Counter {
        Counter { count: start }
    }

    fn render(&self) -> Rendered {
        html! { <p>Count: {@count}</p> }
    }
}

routes! {
    fn by_table(site_name: &'static str) -> Router<SigningKey>;

    scope "/" {
        pipe_through [stamp(site_name), stamp("browser"), session()];
        layout site;

        GET "/" => home as home;
        GET "/orders" => orders;
        GET "/orders/latest" => latest_order;
        GET "/orders/{id: u32}" => show_order as order;
        PUT "/orders/{id: u32}" => rename_order;
        LIVE "/counter/{start: i32}" => Counter as counter;

        scope "/admin" {
            pipe_through [stamp("admin")];
            layout back_office;

            GET "/" => dashboard as admin_dashboard;
            scope "/users/{user: u32}" {
                GET "/posts/{slug: String}" => show_post as admin_post;
            }
        }
    }

    scope "/api" {
        GET "/health" => api::health;
        GET "/files/{*path: String}" => file as api_file;
    }
}

/// The builder code the table above stands for.
fn by_hand(site_name: &'static str) -> Router<SigningKey> {
    let pages = Router::new().merge(
        Scope::new("/")
            .scope(
                Scope::new("/")
                    .pipe_through(stamp(site_name))
                    .pipe_through(stamp("browser"))
                    .pipe_through(session())
                    .layout(site)
                    .route("/", get(home))
                    .route("/orders", get(orders))
                    .route("/orders/latest", get(latest_order))
                    .route("/orders/{id}", get(show_order))
                    .route("/orders/{id}", put(rename_order))
                    .route("/counter/{start}", live::<Counter, _>())
                    .scope(
                        Scope::new("/admin")
                            .pipe_through(stamp("admin"))
                            .layout(back_office)
                            .route("/", get(dashboard))
                            .scope(
                                Scope::new("/users/{user}").route("/posts/{slug}", get(show_post)),
                            ),
                    ),
            )
            .scope(
                Scope::new("/api")
                    .route("/health", get(api::health))
                    .route("/files/{*path}", get(file)),
            ),
    );
    pages.clone().route("/live/websocket", live_socket(pages))
}

const SECRET: &str = "a test secret, at least thirty-two bytes long";

fn key() -> SigningKey {
    SigningKey::new(SECRET).unwrap()
}

/// The status, the headers that tell the apps apart and the body of the response,
/// with what differs from one Dead render to the next left out.
async fn send(app: Router<SigningKey>, request: Request<Body>) -> (StatusCode, String, String) {
    let response = app.with_state(key()).oneshot(request).await.unwrap();
    let status = response.status();
    let headers = ["content-type", "x-through"].map(|name| {
        let values = response.headers().get_all(name).into_iter();
        let values: Vec<_> = values.map(|value| value.to_str().unwrap()).collect();
        format!("{name}: {}", values.join(", "))
    });
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let mut body = String::from_utf8(body.to_vec()).unwrap();
    let differing = [
        " id=\"",
        " data-phx-session=\"",
        " data-phx-static=\"",
        " data-csrf-token=\"",
    ];
    for attribute in differing {
        if let Some(start) = body.find(attribute).map(|at| at + attribute.len()) {
            let end = start + body[start..].find('"').unwrap();
            body.replace_range(start..end, "...");
        }
    }
    (status, headers.join("; "), body)
}

fn request(method: &str, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::USER_AGENT, "a test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#""a griffin""#))
        .unwrap()
}

#[tokio::test]
async fn a_table_serves_what_the_builder_code_it_stands_for_serves() {
    let requests = [
        ("GET", "/"),
        ("GET", "/orders"),
        ("GET", "/orders/latest"),
        ("GET", "/orders/7"),
        ("PUT", "/orders/7"),
        ("GET", "/orders/seven"),
        ("POST", "/orders/7"),
        ("GET", "/counter/5"),
        ("GET", "/admin"),
        ("GET", "/admin/users/7/posts/hello"),
        ("GET", "/users/7/posts/hello"),
        ("GET", "/api/health"),
        ("GET", "/api/files/a/b.txt"),
        ("GET", "/nowhere"),
        // Not a socket's opening request, so refused, but routed.
        ("GET", "/live/websocket"),
    ];
    for (method, path) in requests {
        let by_table = send(by_table("shop"), request(method, path)).await;
        let by_hand = send(by_hand("shop"), request(method, path)).await;

        assert_eq!(by_table, by_hand, "{method} {path}");
    }
}

#[tokio::test]
async fn a_table_puts_each_route_under_the_prefix_pipelines_and_layout_of_its_scopes() {
    let page = |method, path| send(by_table("shop"), request(method, path));

    assert_eq!(
        page("GET", "/").await,
        (
            StatusCode::OK,
            // The response passes the Layers bottom to top.
            "content-type: text/html; charset=utf-8; x-through: browser, shop".to_owned(),
            "<html><body class=\"site\"><h1>Welcome</h1></body></html>".to_owned(),
        )
    );
    assert_eq!(
        page("PUT", "/orders/7").await.2,
        "<html><body class=\"site\"><h1>Order 7 is now of a griffin</h1></body></html>"
    );
    assert_eq!(
        page("GET", "/admin/users/7/posts/hello").await,
        (
            StatusCode::OK,
            "content-type: text/html; charset=utf-8; x-through: admin, browser, shop".to_owned(),
            "<html><body class=\"admin\">\
             <h1>Post hello of user 7, for a test</h1>\
             </body></html>"
                .to_owned(),
        )
    );
    assert_eq!(
        page("GET", "/api/health").await,
        (
            StatusCode::OK,
            "content-type: application/json; x-through: ".to_owned(),
            r#"{"ok":true}"#.to_owned(),
        )
    );
    let (status, _, counter) = page("GET", "/counter/5").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        counter,
        "<html><body class=\"site\">\
         <div id=\"...\" data-phx-main data-phx-session=\"...\" data-phx-static=\"...\" \
         data-csrf-token=\"...\">\
         <p>Count: 5</p>\
         </div></body></html>"
    );
}

#[tokio::test]
async fn a_live_route_of_a_table_is_joined_through_the_socket_the_table_adds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = by_table("shop").with_state(key());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // What the client reads from the container of the Dead render.
    let page = Request::get("/counter/5").body(Body::empty()).unwrap();
    let page = by_table("shop").with_state(key()).oneshot(page).await;
    let page = page.unwrap();
    // The session the Dead render started, which holds the socket's CSRF token.
    let set_cookie = page.headers()[header::SET_COOKIE].to_str().unwrap();
    let cookie = set_cookie.split_once(';').unwrap().0.to_owned();
    let html = to_bytes(page.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(html.to_vec()).unwrap();
    let attribute = |name: &str| {
        let start = html.find(&format!(" {name}=\"")).expect(name) + name.len() + 3;
        html[start..start + html[start..].find('"').unwrap()].to_owned()
    };
    let topic = format!("lv:{}", attribute("id"));
    let join = json!(["1", "1", topic, "phx_join", {
        "url": "http://localhost/counter/5",
        "params": {"_mounts": 0, "_mount_attempts": 0},
        "session": attribute("data-phx-session"),
        "static": attribute("data-phx-static"),
        "sticky": false,
    }]);

    // As the page's script opens the socket: with the token the Dead render left.
    let csrf_token = attribute("data-csrf-token");
    let url = format!("ws://{address}/live/websocket?vsn=2.0.0&_csrf_token={csrf_token}");
    let mut handshake = url.into_client_request().unwrap();
    let cookie = cookie.parse().unwrap();
    handshake.headers_mut().insert(header::COOKIE, cookie);
    let (mut socket, _) = connect_async(handshake).await.unwrap();
    let frame = Message::Text(join.to_string().into());
    socket.send(frame).await.unwrap();
    let reply = socket.next().await.unwrap().unwrap();
    let reply: Value = serde_json::from_str(reply.to_text().unwrap()).unwrap();

    assert_eq!(reply[4]["status"], "ok", "{reply}");
    assert_eq!(reply[4]["response"]["rendered"]["0"], "5");
}

#[test]
fn the_table_is_listed_as_the_method_path_name_and_target_of_each_route() {
    let route = |method, path, name, target| Route {
        method,
        path,
        name,
        target,
    };
    assert_eq!(
        Routes::LIST,
        [
            route("GET", "/", Some("home"), "home"),
            route("GET", "/orders", None, "orders"),
            route("GET", "/orders/latest", None, "latest_order"),
            route("GET", "/orders/{id}", Some("order"), "show_order"),
            route("PUT", "/orders/{id}", None, "rename_order"),
            route("LIVE", "/counter/{start}", Some("counter"), "Counter"),
            route("GET", "/admin", Some("admin_dashboard"), "dashboard"),
            route(
                "GET",
                "/admin/users/{user}/posts/{slug}",
                Some("admin_post"),
                "show_post"
            ),
            route("GET", "/api/health", None, "api::health"),
            route("GET", "/api/files/{*path}", Some("api_file"), "file"),
        ]
    );
}

#[test]
fn a_named_route_has_a_path_helper_with_typed_parameters() {
    assert_eq!(Routes::home(), "/");
    assert_eq!(Routes::order(7), "/orders/7");
    assert_eq!(Routes::counter(-5), "/counter/-5");
    // The path of a route is under the prefixes of its Scopes, parameters and all.
    assert_eq!(Routes::admin_dashboard(), "/admin");
    assert_eq!(Routes::admin_post(7, "hello"), "/admin/users/7/posts/hello");
    assert_eq!(Routes::api_file("a/b.txt"), "/api/files/a/b.txt");
}

#[test]
fn a_path_helper_encodes_a_parameter_so_that_it_stays_one_segment() {
    assert_eq!(
        Routes::admin_post(7, "a/b?c#d e%f"),
        "/admin/users/7/posts/a%2Fb%3Fc%23d%20e%25f"
    );
    assert_eq!(
        Routes::admin_post(7, "naïve+\"quoted\""),
        "/admin/users/7/posts/na%C3%AFve%2B%22quoted%22"
    );
    // A parameter that takes the rest of the path keeps its slashes and nothing else.
    assert_eq!(Routes::api_file("a b/c?d#e"), "/api/files/a%20b/c%3Fd%23e");
}

#[tokio::test]
async fn the_path_of_a_helper_reaches_its_route_with_the_parameters_it_was_given() {
    for slug in ["hello", "a/b?c#d e%f", "naïve+1"] {
        let path = Routes::admin_post(7, slug);
        let (status, _, page) = send(by_table("shop"), request("GET", &path)).await;

        assert_eq!(status, StatusCode::OK, "{path}");
        let post = html! { <h1>Post {slug} of user 7, for a test</h1> }.to_html();
        assert!(page.contains(&post), "{path}: {page}");
    }
    let path = Routes::api_file("a b/c?d#e");
    let (_, _, file) = send(by_table("shop"), request("GET", &path)).await;
    assert_eq!(file, "a b/c?d#e");
}

#[tokio::test]
async fn a_path_helper_is_used_in_an_href_and_in_a_redirect() {
    let (_, _, page) = send(by_table("shop"), request("GET", "/orders")).await;
    assert_eq!(
        page,
        "<html><body class=\"site\">\
         <a href=\"/orders/7\">Order 7</a>\
         <a href=\"/admin/users/7/posts/a%20b\">A post</a>\
         </body></html>"
    );

    let app = by_table("shop").with_state(key());
    let response = app.oneshot(request("GET", "/orders/latest")).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/orders/7");
}
