//! Controllers: plain axum handlers. A template value is a response.

use crate::live::{SignupFields, blank};
use crate::routes::Routes;
use crate::state::AppState;
use griffin::axum::extract::State;
use griffin::axum::http::StatusCode;
use griffin::axum::response::{IntoResponse, Redirect, Response};
use griffin::live::FormParams;
use griffin::security::csrf_field;
use griffin::session::Session;
use griffin::template::Rendered;
use griffin::{html, html_file};
use griffin_domain::{Action, Changeset, Form};

use __app__::accounts::{self, Signup};

/// `GET /`: a template file, from `templates/`.
pub async fn home() -> Rendered {
    html_file!("home.html.griffin")
}

/// `GET /signup/classic`: the signup form, posted the plain way.
pub async fn show_signup(session: Session) -> Rendered {
    classic(&blank(), &session)
}

/// What a post of the form comes to: the form again with its errors, or a redirect.
#[derive(Debug)]
pub enum Created {
    Invalid { form: Form, session: Session },
    Welcome,
}

impl IntoResponse for Created {
    fn into_response(self) -> Response {
        match self {
            // A form shown again with errors answers 422.
            Created::Invalid { form, session } => {
                (StatusCode::UNPROCESSABLE_ENTITY, classic(&form, &session)).into_response()
            }
            Created::Welcome => Redirect::to(&Routes::classic()).into_response(),
        }
    }
}

/// `POST /signup/classic`. The Pipeline's CSRF check has already refused a post that
/// did not bring the token of its session. `FormParams` reads the body with the same
/// bounds as a live form, so a hostile body cannot use more memory than a form of its size.
pub async fn create_signup(
    State(app): State<AppState>,
    session: Session,
    params: FormParams,
) -> Created {
    let changeset = Changeset::<Signup>::cast_form("signup", params.pairs());
    let changeset = accounts::check(changeset).with_action(Action::Insert);
    match accounts::register(&app, changeset).await {
        Ok(signup) => {
            session.put_flash("info", signup.welcome());
            Created::Welcome
        }
        Err(changeset) => Created::Invalid {
            form: changeset.to_form(),
            session,
        },
    }
}

fn classic(form: &Form, session: &Session) -> Rendered {
    let action = Routes::classic();
    html! {
        <main>
            <h1>Sign up, classic</h1>
            <form id="classic-form" method="post" action={action.as_str()} novalidate>
                {csrf_field(session)}
                <SignupFields form={form} />
                <button type="submit">Sign up</button>
            </form>
        </main>
    }
}
