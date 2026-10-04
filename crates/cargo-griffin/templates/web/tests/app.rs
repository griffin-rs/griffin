//! The application through its network boundary: requests in, responses out.

use griffin::axum::Router;
use griffin::axum::body::{Body, to_bytes};
use griffin::axum::http::{Request, Response, StatusCode, header};
use griffin::config::Secret;
use griffin::error::ErrorPage;
use tower::ServiceExt as _;

use __app___web::app;
use __app___web::config::Config;

fn router() -> Router {
    let config = Config {
        secret_key_base: Secret::new("a secret for tests only: not used anywhere else"),
        port: 0,
        error_page: ErrorPage::Plain,
        secure_cookie: true,
    };
    app(&config).expect("the test configuration is complete")
}

async fn send(app: &Router, request: Request<Body>) -> Response<Body> {
    app.clone().oneshot(request).await.unwrap()
}

async fn text(response: Response<Body>) -> String {
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).body(Body::empty()).unwrap()
}

fn post(path: &str, cookie: &str, body: String) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

/// The session cookie a response sets, as a request sends it back.
fn cookie(response: &Response<Body>) -> String {
    let set = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a session cookie");
    set.to_str().unwrap().split(';').next().unwrap().to_owned()
}

/// The value of `name="_csrf_token"` in a page.
fn csrf_token(html: &str) -> String {
    let at = html
        .find("name=\"_csrf_token\"")
        .expect("the form has a token");
    let rest = &html[at..];
    let start = rest.find("value=\"").unwrap() + 7;
    rest[start..].split('"').next().unwrap().to_owned()
}

fn form(token: &str, email: &str, username: &str) -> String {
    format!(
        "_csrf_token={token}&signup%5Bemail%5D={email}&signup%5Busername%5D={username}&signup%5Btopics%5D%5B%5D=rust"
    )
}

#[tokio::test]
async fn the_home_page_is_served_with_secure_headers() {
    let response = send(&router(), get("/")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().contains_key("content-security-policy"));
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(text(response).await.contains("Welcome to __App__"));
}

#[tokio::test]
async fn a_static_file_and_an_unmatched_route_carry_the_secure_headers_too() {
    for (path, status) in [
        ("/assets/app.css", StatusCode::OK),
        ("/no/such/page", StatusCode::NOT_FOUND),
    ] {
        let response = send(&router(), get(path)).await;
        assert_eq!(response.status(), status, "{path}");
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(policy.contains("frame-ancestors"), "{path}: {policy}");
        assert!(response.headers().contains_key("referrer-policy"), "{path}");
        assert_eq!(
            response.headers()["x-content-type-options"],
            "nosniff",
            "{path}"
        );
    }
}

#[tokio::test]
async fn the_live_page_carries_the_csrf_token_the_script_sends_at_connect() {
    let html = text(send(&router(), get("/signup")).await).await;
    assert!(html.contains("data-phx-main"));
    assert!(html.contains("data-csrf-token="));
}

#[tokio::test]
async fn an_unknown_path_gets_the_application_error_page() {
    let response = send(&router(), get("/nothing-here")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await.contains("Back to the home page"));
}

#[tokio::test]
async fn a_post_without_the_csrf_token_is_refused() {
    let app = router();
    let page = send(&app, get("/signup/classic")).await;
    let cookie = cookie(&page);
    let body = form("", "new%40example.com", "refused1");
    let response = send(&app, post("/signup/classic", &cookie, body)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_classic_form_shows_errors_then_registers_then_refuses_a_taken_name() {
    let app = router();
    let page = send(&app, get("/signup/classic")).await;
    let cookie = cookie(&page);
    let token = csrf_token(&text(page).await);

    let invalid = post("/signup/classic", &cookie, form(&token, "nope", "classic1"));
    let response = send(&app, invalid).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(response)
            .await
            .contains("must be an address like name@example.com")
    );

    let valid = post(
        "/signup/classic",
        &cookie,
        form(&token, "a%40example.com", "classic1"),
    );
    assert_eq!(send(&app, valid).await.status(), StatusCode::SEE_OTHER);

    let taken = post(
        "/signup/classic",
        &cookie,
        form(&token, "b%40example.com", "classic1"),
    );
    let response = send(&app, taken).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(response).await.contains("has already been taken"));
}
