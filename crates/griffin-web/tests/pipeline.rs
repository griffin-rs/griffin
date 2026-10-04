//! Pipelines on Scopes, through a small app driven as a tower service.

use griffin_web::axum::Router;
use griffin_web::axum::body::{Body, to_bytes};
use griffin_web::axum::extract::Request;
use griffin_web::axum::http::{HeaderValue, StatusCode};
use griffin_web::axum::response::Response;
use griffin_web::axum::routing::get;
use griffin_web::pipeline::Pipeline;
use griffin_web::router::Scope;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tower::util::{MapRequestLayer, MapResponseLayer};
use tower::{Layer, Service, ServiceBuilder, ServiceExt as _};

/// What ran, in the order it ran.
type Order = Arc<Mutex<Vec<&'static str>>>;

/// A Layer that records its name when a request passes through it.
fn record(
    order: &Order,
    name: &'static str,
) -> MapRequestLayer<impl Fn(Request) -> Request + Clone + use<>> {
    let order = order.clone();
    MapRequestLayer::new(move |request: Request| {
        order.lock().unwrap().push(name);
        request
    })
}

/// A Pipeline is a plain function that gives the value.
fn browser(order: &Order) -> Pipeline {
    Pipeline::new()
        .layer(record(order, "first"))
        .layer(record(order, "second"))
}

async fn get_page(app: Router, path: &str) -> Response {
    let request = Request::get(path).body(Body::empty()).unwrap();
    app.oneshot(request).await.unwrap()
}

#[tokio::test]
async fn layers_run_top_to_bottom_in_the_order_written() {
    let order = Order::default();
    let handler_order = order.clone();
    let home = move || async move { handler_order.lock().unwrap().push("handler") };
    let scope = Scope::new("/")
        .pipe_through(browser(&order))
        .route("/", get(home));

    let response = get_page(Router::new().merge(scope), "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(*order.lock().unwrap(), ["first", "second", "handler"]);
}

#[tokio::test]
async fn a_scope_passes_through_its_pipelines_in_the_order_given() {
    let order = Order::default();
    let scope = Scope::new("/")
        .pipe_through(Pipeline::new().layer(record(&order, "browser")))
        .pipe_through(Pipeline::new().layer(record(&order, "admin")))
        .route("/", get(|| async {}));

    get_page(Router::new().merge(scope), "/").await;

    assert_eq!(*order.lock().unwrap(), ["browser", "admin"]);
}

#[tokio::test]
async fn a_pipeline_is_only_for_the_routes_of_its_scope() {
    let order = Order::default();
    let site = Scope::new("/")
        .pipe_through(browser(&order))
        .route("/", get(|| async {}));
    let api = Scope::new("/api").route("/health", get(|| async { "ok" }));

    let response = get_page(Router::new().merge(site).merge(api), "/api/health").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(order.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_scope_inside_a_scope_passes_through_the_outer_pipelines_first() {
    let order = Order::default();
    let site = Scope::new("/")
        .pipe_through(Pipeline::new().layer(record(&order, "browser")))
        .route("/", get(|| async {}))
        .scope(
            Scope::new("/admin")
                .pipe_through(Pipeline::new().layer(record(&order, "admin")))
                .route("/", get(|| async {})),
        );
    let app = Router::new().merge(site);

    get_page(app.clone(), "/admin").await;
    assert_eq!(*order.lock().unwrap(), ["browser", "admin"]);

    // The inner Pipeline is for the inner Scope alone.
    order.lock().unwrap().clear();
    get_page(app, "/").await;
    assert_eq!(*order.lock().unwrap(), ["browser"]);
}

/// A tower Layer written by hand, as a crate that knows nothing of Griffin would.
#[derive(Clone)]
struct Stamp(&'static str);

#[derive(Clone)]
struct StampService<S> {
    inner: S,
    value: &'static str,
}

impl<S> Layer<S> for Stamp {
    type Service = StampService<S>;

    fn layer(&self, inner: S) -> StampService<S> {
        let value = self.0;
        StampService { inner, value }
    }
}

impl<S: Service<Request>> Service<Request> for StampService<S> {
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, mut request: Request) -> S::Future {
        let value = HeaderValue::from_static(self.value);
        request.headers_mut().insert("x-stamp", value);
        self.inner.call(request)
    }
}

#[tokio::test]
async fn any_tower_layer_goes_in_a_pipeline() {
    async fn stamp(request: Request) -> String {
        request.headers()["x-stamp"].to_str().unwrap().to_owned()
    }
    let pipeline = Pipeline::new()
        // A Layer written by hand.
        .layer(Stamp("stamped"))
        // A Layer of tower's own.
        .layer(MapResponseLayer::new(|mut response: Response| {
            let value = HeaderValue::from_static("tower");
            response.headers_mut().insert("x-from", value);
            response
        }))
        // A stack of tower Layers, which is a Layer.
        .layer(
            ServiceBuilder::new().layer(MapResponseLayer::new(|mut response: Response| {
                let value = HeaderValue::from_static("a stack");
                response.headers_mut().insert("x-also-from", value);
                response
            })),
        )
        // A Layer whose service answers with something other than axum's response.
        .layer(MapResponseLayer::new(|response: Response| {
            (StatusCode::ACCEPTED, response)
        }));
    let scope = Scope::new("/")
        .pipe_through(pipeline)
        .route("/", get(stamp));

    let response = get_page(Router::new().merge(scope), "/").await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()["x-from"], "tower");
    assert_eq!(response.headers()["x-also-from"], "a stack");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "stamped");
}
