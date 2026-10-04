mod nested {
    griffin_web::routes! {
        fn router() -> Router;

        live_session outer {
            live_session inner {
                LIVE "/" => Dashboard;
            }
        }
    }
}

mod twice {
    griffin_web::routes! {
        fn router() -> Router;

        live_session admin {
            LIVE "/" => Dashboard;
        }
        live_session admin {
            LIVE "/other" => Other;
        }
    }
}

fn main() {}
