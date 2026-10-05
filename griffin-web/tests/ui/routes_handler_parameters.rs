use griffin_web::axum::Router;
use griffin_web::axum::extract::State;
use griffin_web::html;
use griffin_web::live::{LiveView, Socket};
use griffin_web::router::Path;
use griffin_web::routes;
use griffin_web::template::Rendered;
use griffin_web::token::SigningKey;
use std::convert::Infallible;

async fn show_user(State(_key): State<SigningKey>, Path(id): Path<u32>) -> String {
    format!("user {id}")
}

async fn list_users() -> &'static str {
    "users"
}

/// With axum's own `Path`.
async fn show_order(griffin_web::axum::extract::Path(id): griffin_web::axum::extract::Path<u32>) -> String {
    format!("order {id}")
}

struct Counter;

impl LiveView for Counter {
    type Params = i32;
    type Event = Infallible;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_start: i32, _socket: &mut Socket) -> Counter {
        Counter
    }

    fn render(&self) -> Rendered {
        html! { <p>Count</p> }
    }
}

routes! {
    fn router() -> Router<SigningKey>;

    // These two fit.
    GET "/users/{id: u32}" => show_user;
    LIVE "/counter/{start: i32}" => Counter;

    // A parameter of another type than the handler takes.
    GET "/names/{name: String}" => show_user;
    // More parameters than the handler takes.
    scope "/teams/{team: u32}" {
        GET "/users/{id: u32}" => show_user;
    }
    // A handler that takes none.
    GET "/lists/{id: u32}" => list_users;
    // axum's `Path` and not Griffin's.
    GET "/orders/{id: u32}" => show_order;

    // A LiveView mounted with another type, and with a parameter the route has not.
    LIVE "/counters/{start: u32}" => Counter;
    LIVE "/counters" => Counter;
}

fn main() {}
