//! Every route of the application, in one table. A `GET` or `POST` names a controller,
//! `LIVE` names a LiveView, and `as name` makes a path helper: `Routes::signup()`.

use crate::state::AppState;
use crate::{layouts, live, pages};
use griffin::axum::Router;
use griffin::pipeline::Pipeline;
use griffin::routes;

routes! {
    pub fn router(browser: Pipeline<AppState>) -> Router<AppState>;

    scope "/" {
        pipe_through [browser];
        layout layouts::site;

        GET "/" => pages::home as home;
        LIVE "/signup" => live::SignupLive as signup;
        GET "/signup/classic" => pages::show_signup as classic;
        POST "/signup/classic" => pages::create_signup;
    }
}
