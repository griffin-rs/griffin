//! The LiveView: a signup form that is validated as the user types and registered on
//! submit. All the rules are in the domain crate; this is only the page.

use crate::components::{Checkboxes, FormErrors, Input};
use crate::state::Memory;
use griffin::live::{Event, FormParams, LiveView, Socket};
use griffin::template::Rendered;
use griffin::{component, html};
use griffin_domain::{Action, Changeset, Form};
use std::convert::Infallible;

use __app__::accounts::{self, Signup, TOPICS};

/// The fields of the form, shared with the classic page.
#[component]
pub fn SignupFields(form: &Form) -> Rendered {
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
pub fn blank() -> Form {
    let nothing = std::iter::empty::<(&str, &str)>();
    Changeset::<Signup>::cast_form("signup", nothing).to_form()
}

pub struct SignupLive {
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

impl LiveView for SignupLive {
    type Params = ();
    type Event = SignupEvent;
    type Message = Infallible;
    /// Nothing here can fail: a domain rejection is a change to the state, shown in the
    /// form, and not an error.
    type Error = Infallible;

    async fn mount(_params: (), _socket: &mut Socket) -> SignupLive {
        SignupLive {
            form: blank(),
            welcome: None,
        }
    }

    async fn handle_event(
        &mut self,
        event: SignupEvent,
        _socket: &mut Socket,
    ) -> Result<(), Infallible> {
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
                // How a LiveView should reach the Capabilities is open: griffin issue #56.
                match accounts::register(&*Memory::shared(), changeset).await {
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
        // `novalidate`: the Changeset is the validator. An `id` and a `phx-change` let the
        // client recover the form after a reconnect.
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
