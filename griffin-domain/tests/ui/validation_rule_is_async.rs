//! Stage two cannot await: the rule returns `()`, not a future.

use std::collections::HashMap;

use griffin_domain::{Changeset, Errors};

#[derive(Changeset)]
struct Signup {
    email: String,
}

async fn email_is_free(_signup: &Signup, _errors: &mut Errors) {}

fn main() {
    let _ = Changeset::<Signup>::cast(&HashMap::new()).validate(email_is_free);
}
