use griffin_web::axum::Router;
use griffin_web::router::Path;
use griffin_web::routes;

async fn show_user(Path(id): Path<u32>) -> String {
    format!("user {id}")
}

async fn show_post(Path(slug): Path<String>) -> String {
    slug
}

routes! {
    fn router() -> Router;

    GET "/users/{id: u32}" => show_user as user;
    GET "/posts/{slug: String}" => show_post as post;
    GET "/drafts/{slug: String}" => show_post;
}

fn main() {
    let _: Router = router();

    // A route the table does not name.
    Routes::usr(7);
    Routes::draft("hello");

    // An argument of another type than the parameter.
    Routes::user("seven");
    Routes::post(7);

    // Too few arguments.
    Routes::user();
}
