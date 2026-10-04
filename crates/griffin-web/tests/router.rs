//! Controller pages in Scopes with layouts, through a small app driven as a tower
//! service: an HTTP request in, a response out.

use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::extract::{Query, State};
use griffin_web::axum::http::{HeaderMap, Request, StatusCode, header};
use griffin_web::axum::response::{IntoResponse, Response};
use griffin_web::axum::routing::get;
use griffin_web::axum::{Json, Router};
use griffin_web::live::{LiveView, Socket, live};
use griffin_web::router::{Path, Scope};
use griffin_web::template::{Rendered, SlotEntries};
use griffin_web::token::SigningKey;
use griffin_web::{component, html};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// A layout is a Component with a default slot.
#[component]
fn Site(#[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <html><body class="site">{inner_block}</body></html> }
}

/// Another layout, with an attribute of its own.
#[component]
fn Admin(section: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <html><body class="admin"><nav>{section}</nav>{inner_block}</body></html> }
}

#[derive(Clone)]
struct AppState {
    greeting: &'static str,
}

async fn home() -> Rendered {
    let name = "Ann & <Bob>";
    html! { <h1>Hello, {name}!</h1> }
}

#[derive(Deserialize)]
struct Search {
    q: String,
}

/// A handler is axum's: it takes what any axum handler takes.
async fn search(
    State(app): State<AppState>,
    Query(search): Query<Search>,
    headers: HeaderMap,
) -> Rendered {
    let agent = headers[header::USER_AGENT].to_str().unwrap_or_default();
    html! { <p>{app.greeting}, {agent}: no results for {search.q}</p> }
}

async fn dashboard() -> Rendered {
    html! { <h1>Dashboard</h1> }
}

async fn stats() -> Json<Value> {
    Json(json!({"users": 2}))
}

#[derive(Debug, PartialEq)]
struct User {
    id: u32,
    name: &'static str,
}

/// Every way showing a user can end. A test reads the outcome off the value; only
/// `into_response` knows what each looks like.
#[derive(Debug, PartialEq)]
enum ShowUser {
    Found(User),
    NotFound,
    Forbidden,
}

impl IntoResponse for ShowUser {
    fn into_response(self) -> Response {
        match self {
            ShowUser::Found(user) => html! { <h1>User {user.id}: {user.name}</h1> }.into_response(),
            ShowUser::NotFound => {
                (StatusCode::NOT_FOUND, html! { <h1>No such user</h1> }).into_response()
            }
            ShowUser::Forbidden => StatusCode::FORBIDDEN.into_response(),
        }
    }
}

async fn show_user(Path(id): Path<u32>) -> ShowUser {
    match id {
        7 => ShowUser::Found(User { id, name: "Ann" }),
        13 => ShowUser::Forbidden,
        _ => ShowUser::NotFound,
    }
}

/// What a Context refuses with. It knows nothing of HTTP.
#[derive(Debug)]
enum OrderError {
    NotFound,
    Locked,
}

fn find_order(id: u32) -> Result<&'static str, OrderError> {
    match id {
        1 => Ok("a griffin"),
        2 => Err(OrderError::Locked),
        _ => Err(OrderError::NotFound),
    }
}

/// The application's own error type: every error a handler ends with `?` on.
enum AppError {
    Order(OrderError),
}

impl From<OrderError> for AppError {
    fn from(error: OrderError) -> AppError {
        AppError::Order(error)
    }
}

/// The one place where an error of the application becomes a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Order(OrderError::NotFound) => {
                (StatusCode::NOT_FOUND, html! { <h1>No such order</h1> }).into_response()
            }
            AppError::Order(OrderError::Locked) => {
                (StatusCode::CONFLICT, Json(json!({"error": "locked"}))).into_response()
            }
        }
    }
}

async fn show_order(Path(id): Path<u32>) -> Result<Rendered, AppError> {
    let order = find_order(id)?;
    Ok(html! { <h1>Order of {order}</h1> })
}

/// Another verb on the path of `show_order`, with a request body.
async fn rename_order(Path(id): Path<u32>, Json(name): Json<String>) -> Rendered {
    html! { <h1>Order {id} is now of {name}</h1> }
}

async fn widget() -> Rendered {
    html! { <p>A widget</p> }
}

fn app() -> Router {
    let site = Scope::new("/")
        .layout(|page, _flash| html! { <Site>{page}</Site> })
        .route("/", get(home))
        .route("/search", get(search))
        .route("/orders/{id}", get(show_order).put(rename_order));
    let admin = Scope::new("/admin")
        .layout(|page, _flash| html! { <Admin section="Back office">{page}</Admin> })
        .route("/", get(dashboard))
        .route("/stats", get(stats))
        .route("/users/{id}", get(show_user));
    let embed = Scope::new("/embed").route("/widget", get(widget));
    Router::new()
        .merge(site)
        .merge(admin)
        .merge(embed)
        .with_state(AppState { greeting: "Hello" })
}

async fn send(app: Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = app.oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    // Wrapping a page in a layout changes its length: the header has to follow.
    assert_eq!(headers[header::CONTENT_LENGTH], body.len().to_string());
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

async fn get_page(app: Router, path: &str) -> (StatusCode, HeaderMap, String) {
    send(app, Request::get(path).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn a_handler_returning_a_template_is_rendered_inside_the_layout_of_its_scope() {
    let (status, headers, html) = get_page(app(), "/").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(
        html,
        "<html><body class=\"site\"><h1>Hello, Ann &amp; &lt;Bob&gt;!</h1></body></html>"
    );
}

#[tokio::test]
async fn each_scope_wraps_its_pages_in_its_own_layout() {
    let (_, _, site) = get_page(app(), "/").await;
    let (status, _, admin) = get_page(app(), "/admin").await;

    assert!(
        site.starts_with("<html><body class=\"site\"><h1>"),
        "{site}"
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        admin,
        "<html><body class=\"admin\"><nav>Back office</nav><h1>Dashboard</h1></body></html>"
    );
}

#[tokio::test]
async fn a_scope_without_a_layout_returns_the_bare_page() {
    let (status, headers, html) = get_page(app(), "/embed/widget").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(html, "<p>A widget</p>");
}

#[tokio::test]
async fn a_scope_inside_a_scope_adds_to_its_prefix_and_keeps_its_layout_unless_it_has_its_own() {
    let shop = Scope::new("/shop")
        .layout(|page, _flash| html! { <Site>{page}</Site> })
        .route("/", get(widget))
        .scope(
            Scope::new("/parts")
                .route("/widget", get(widget))
                .scope(Scope::new("/{id}").route("/", get(show_user))),
        )
        .scope(
            Scope::new("/admin")
                .layout(|page, _flash| html! { <Admin section="Shop">{page}</Admin> })
                .route("/", get(dashboard)),
        );
    let app: Router = Router::new().merge(shop);

    let site = "<html><body class=\"site\"><p>A widget</p></body></html>";
    assert_eq!(get_page(app.clone(), "/shop").await.2, site);
    assert_eq!(get_page(app.clone(), "/shop/parts/widget").await.2, site);
    assert_eq!(
        get_page(app.clone(), "/shop/parts/7").await.2,
        "<html><body class=\"site\"><h1>User 7: Ann</h1></body></html>"
    );
    assert_eq!(
        get_page(app.clone(), "/shop/admin").await.2,
        "<html><body class=\"admin\"><nav>Shop</nav><h1>Dashboard</h1></body></html>"
    );
    // The inner Scope's routes are under the outer prefix and nowhere else.
    let request = Request::get("/parts/widget").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_path_parameter_arrives_typed() {
    let (status, _, html) = get_page(app(), "/admin/users/7").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("</nav><h1>User 7: Ann</h1></body>"), "{html}");
}

#[tokio::test]
async fn a_path_whose_parameter_does_not_parse_is_not_found() {
    for path in ["/admin/users/seven", "/admin/users/-1", "/admin/users/7.5"] {
        let (status, _, html) = get_page(app(), path).await;

        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(html, "", "{path}");
    }
}

#[tokio::test]
async fn a_handler_returning_json_is_not_wrapped_in_the_layout() {
    let (status, headers, body) = get_page(app(), "/admin/stats").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(body, r#"{"users":2}"#);
}

#[tokio::test]
async fn an_outcome_is_asserted_on_without_rendering_it() {
    let found = ShowUser::Found(User { id: 7, name: "Ann" });

    assert_eq!(show_user(Path(7)).await, found);
    assert_eq!(show_user(Path(8)).await, ShowUser::NotFound);
    assert_eq!(show_user(Path(13)).await, ShowUser::Forbidden);
}

#[tokio::test]
async fn each_outcome_becomes_its_own_response() {
    let (not_found, _, page) = get_page(app(), "/admin/users/8").await;
    let (forbidden, _, nothing) = get_page(app(), "/admin/users/13").await;

    assert_eq!(not_found, StatusCode::NOT_FOUND);
    // A page is in the layout whatever its status.
    assert!(
        page.contains("</nav><h1>No such user</h1></body>"),
        "{page}"
    );
    assert_eq!(forbidden, StatusCode::FORBIDDEN);
    assert_eq!(nothing, "");
}

#[tokio::test]
async fn an_error_of_the_application_becomes_a_response_in_its_one_mapping() {
    let (ok, _, order) = get_page(app(), "/orders/1").await;
    let (locked, headers, error) = get_page(app(), "/orders/2").await;
    let (not_found, _, page) = get_page(app(), "/orders/3").await;

    assert_eq!(ok, StatusCode::OK);
    assert!(
        order.contains("<body class=\"site\"><h1>Order of a griffin</h1>"),
        "{order}"
    );
    assert_eq!(locked, StatusCode::CONFLICT);
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(error, r#"{"error":"locked"}"#);
    assert_eq!(not_found, StatusCode::NOT_FOUND);
    assert!(
        page.contains("<body class=\"site\"><h1>No such order</h1>"),
        "{page}"
    );
}

#[tokio::test]
async fn the_dead_render_of_a_live_view_is_wrapped_in_the_layout_of_its_scope() {
    struct Clock;
    impl LiveView for Clock {
        type Params = ();
        type Event = std::convert::Infallible;
        type Message = std::convert::Infallible;
        type Error = std::convert::Infallible;

        async fn mount(_params: (), _socket: &mut Socket) -> Clock {
            Clock
        }

        fn render(&self) -> Rendered {
            html! { <p>Tick</p> }
        }
    }
    let pages = Scope::new("/live")
        .layout(|page, _flash| html! { <Site>{page}</Site> })
        .route("/clock", live::<Clock, _>());
    let key = SigningKey::new("a test secret, at least thirty-two bytes long").unwrap();
    let app = Router::new().merge(pages).with_state(key);

    let (status, headers, html) = get_page(app, "/live/clock").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    // The layout is the whole document, and the page is in the client's container.
    assert!(
        html.starts_with("<html><body class=\"site\"><div id=\"phx-"),
        "{html}"
    );
    assert!(
        html.contains("\" data-phx-main data-phx-session=\""),
        "{html}"
    );
    assert!(
        html.ends_with("\"><p>Tick</p></div></body></html>"),
        "{html}"
    );
}

#[tokio::test]
async fn a_route_takes_any_verb_and_a_request_body() {
    let request = Request::put("/orders/1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#""a <gryphon>""#))
        .unwrap();

    let (status, _, html) = send(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<body class=\"site\"><h1>Order 1 is now of a &lt;gryphon&gt;</h1>"),
        "{html}"
    );
}

#[tokio::test]
async fn a_handler_takes_axum_extractors() {
    let request = Request::get("/search?q=%3Cb%3Egold%3C/b%3E")
        .header(header::USER_AGENT, "a test")
        .body(Body::empty())
        .unwrap();

    let (status, _, html) = send(app(), request).await;

    assert_eq!(status, StatusCode::OK);
    // State, the query string and a header, the query escaped as any dynamic content.
    assert_eq!(
        html,
        "<html><body class=\"site\">\
         <p>Hello, a test: no results for &lt;b&gt;gold&lt;/b&gt;</p>\
         </body></html>"
    );
}
