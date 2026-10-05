use griffin_web::router::Path;
use griffin_web::routes;

async fn show_user(Path(id): Path<u32>) -> String {
    format!("user {id}")
}

async fn rename_user(Path(id): Path<u32>) -> String {
    format!("user {id}")
}

routes! {
    fn router() -> griffin_web::axum::Router;

    GET "/users/{id: u32}" => show_user as user;
    // Another verb on the same path is another route.
    PUT "/users/{id: u32}" => rename_user;

    scope "/users" {
        // The same path, whatever its parameter is called.
        GET "/{user: u32}" => rename_user;
        // A LiveView is reached by a GET.
        LIVE "/{id: u32}" => UserLive;
    }
    scope "/accounts" {
        PUT "/{id: u32}" => rename_user as user;
    }
}

fn main() {}
