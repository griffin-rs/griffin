//! The two pages of one form: a LiveView that validates as the user types, and a
//! controller that takes a plain post. Both cast the same `Signup` Changeset and show
//! it with the same Components.

use crate::accounts::{self, Memory, Signup, TOPICS};
use crate::components::{Checkboxes, FormErrors, Input};
use griffin_domain::{Action, Changeset, Form};
use griffin_web::axum::extract::State;
use griffin_web::axum::http::StatusCode;
use griffin_web::axum::response::{IntoResponse, Redirect, Response};
use griffin_web::live::{Event, FormParams, LiveView, Socket};
use griffin_web::security::csrf_field;
use griffin_web::session::Session;
use griffin_web::template::Rendered;
use griffin_web::{component, html};
use std::convert::Infallible;
use std::sync::Arc;

/// The fields of the form, in both pages.
#[component]
fn SignupFields(form: &Form) -> Rendered {
    html! {
        <div>
            <FormErrors form={form} />
            <Input form={form} field="email" kind="email" autocomplete="email">Email</Input>
            <Input form={form} field="username" autocomplete="username">Username</Input>
            <Checkboxes form={form} field="topics" choices={&TOPICS}>Topics</Checkboxes>
        </div>
    }
}

/// The form before the user has typed anything: no errors, as no action is set.
fn blank() -> Form {
    let nothing = std::iter::empty::<(&str, &str)>();
    Changeset::<Signup>::cast_form("signup", nothing).to_form()
}

/// The live page. Its form is validated on every change and registered on submit.
pub struct LiveSignup {
    form: Form,
    welcome: Option<String>,
}

/// What the browser asks of the page. Both carry the form's params, as the client
/// serialises them: nested (`signup[email]`) and list (`signup[topics][]`) names.
#[derive(Event)]
pub enum SignupEvent {
    Change {
        #[form]
        params: FormParams,
    },
    Submit {
        #[form]
        params: FormParams,
    },
}

impl LiveView for LiveSignup {
    type Params = ();
    type Event = SignupEvent;
    type Message = Infallible;
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> LiveSignup {
        LiveSignup {
            form: blank(),
            welcome: None,
        }
    }

    async fn handle_event(
        &mut self,
        event: SignupEvent,
        _socket: &mut Socket,
    ) -> Result<(), Self::Error> {
        match event {
            // Stages one and two, with no I/O: this runs on every keystroke.
            SignupEvent::Change { params } => {
                let changeset = Changeset::<Signup>::cast_form("signup", params.pairs());
                self.form = accounts::check(changeset)
                    .with_action(Action::Validate)
                    .to_form();
            }
            // All three stages. The Context may add an error of its own.
            SignupEvent::Submit { params } => {
                let changeset = Changeset::<Signup>::cast_form("signup", params.pairs());
                let changeset = accounts::check(changeset).with_action(Action::Insert);
                match accounts::register(&*accounts::store(), changeset).await {
                    Ok(signup) => {
                        self.welcome = Some(signup.welcome());
                        self.form = blank();
                    }
                    Err(changeset) => {
                        self.welcome = None;
                        self.form = changeset.to_form();
                    }
                }
            }
        }
        Ok(())
    }

    fn render(&self) -> Rendered {
        let form = &self.form;
        let change = SignupEvent::Change {
            params: FormParams::default(),
        };
        let submit = SignupEvent::Submit {
            params: FormParams::default(),
        };
        html! {
            <main>
                <h1>Sign up, live</h1>
                <p :if={@welcome.is_some()} id="welcome" role="status">{@welcome.as_deref().unwrap_or_default()}</p>
                <form id="signup-form" phx-change={change} phx-submit={submit} novalidate>
                    <SignupFields form={form} />
                    <button type="submit" phx-disable-with="Saving...">Sign up</button>
                </form>
            </main>
        }
    }
}

/// `GET /classic`: the same form, posted the plain way.
pub async fn show(session: Session) -> Rendered {
    classic(&blank(), &session)
}

/// What a post of the form comes to: the form again with its errors, or a welcome.
#[derive(Debug)]
pub enum Created {
    Invalid { form: Form, session: Session },
    Welcome,
}

impl IntoResponse for Created {
    fn into_response(self) -> Response {
        match self {
            Created::Invalid { form, session } => {
                (StatusCode::UNPROCESSABLE_ENTITY, classic(&form, &session)).into_response()
            }
            Created::Welcome => Redirect::to("/classic").into_response(),
        }
    }
}

/// `POST /classic`. The Pipeline's CSRF check has already refused a post that did not
/// bring the token of its session.
///
/// The store comes from the application state, where `main` put it.
pub async fn create(
    State(accounts): State<Arc<Memory>>,
    session: Session,
    params: FormParams,
) -> Created {
    let changeset = Changeset::<Signup>::cast_form("signup", params.pairs());
    let changeset = accounts::check(changeset).with_action(Action::Insert);
    match accounts::register(&*accounts, changeset).await {
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
    html! {
        <main>
            <h1>Sign up, classic</h1>
            <form id="classic-form" method="post" action="/classic" novalidate>
                {csrf_field(session)}
                <SignupFields form={form} />
                <button type="submit">Sign up</button>
            </form>
        </main>
    }
}
