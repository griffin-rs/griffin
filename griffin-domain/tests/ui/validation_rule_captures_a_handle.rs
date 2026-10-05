//! Stage two cannot be handed a database handle: the rule is a plain `fn`.

use std::collections::HashMap;

use griffin_domain::{Changeset, Errors};

#[derive(Changeset)]
struct Signup {
    email: String,
}

struct Database;

impl Database {
    fn email_taken(&self, _email: &str) -> bool {
        true
    }
}

fn main() {
    let database = Database;
    let _ = Changeset::<Signup>::cast(&HashMap::new()).validate(
        |signup: &Signup, errors: &mut Errors| {
            if database.email_taken(&signup.email) {
                errors.add("email", "has already been taken");
            }
        },
    );
}
