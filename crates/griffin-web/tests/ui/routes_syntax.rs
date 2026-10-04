use griffin_web::routes;

// A syntax error ends the reading of a table, so each table here has one.

mod no_function {
    griffin_web::routes! {
        GET "/" => home;
    }
}

mod split_across_files {
    griffin_web::routes! {
        fn router() -> Router;

        mount users::routes;
    }
}

mod live_session {
    griffin_web::routes! {
        fn router() -> Router;

        live_session "admin" {
            LIVE "/" => Dashboard;
        }
    }
}

mod not_a_verb {
    griffin_web::routes! {
        fn router() -> Router;

        get "/" => home;
    }
}

mod no_arrow {
    griffin_web::routes! {
        fn router() -> Router;

        GET "/", home;
    }
}

mod handler_is_not_a_path {
    griffin_web::routes! {
        fn router() -> Router;

        GET "/" => || async { "home" };
    }
}

// The errors of what a table says are all reported.
routes! {
    fn router() -> Router;

    GET "users/{id: u32}" => show_user;
    GET "/users/{id}" => show_user;
    GET "/users/{id: }" => show_user;
    GET "/users/{id: u32" => show_user;
    GET "/files/{*: String}" => show_file;

    scope "/teams/{id: u32}" {
        layout site;
        layout admin;

        GET "/users/{id: u32}" => show_user;
    }
}

fn main() {}
