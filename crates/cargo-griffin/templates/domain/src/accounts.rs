//! The accounts Context: the signup Changeset and the function that registers one.
//!
//! A Changeset has three stages (parse, validate across fields, then the Context's own
//! checks). Stages one and two do no I/O and run on every keystroke of a live form;
//! stage three runs on submit only.

use crate::capabilities::HasStore;
use griffin_domain::{Changeset, Errors};
use std::str::FromStr;

/// Stage one: each type is a smart constructor, so a `Signup` that exists is valid.
#[derive(Clone, Debug)]
pub struct Email(String);

impl FromStr for Email {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Email, &'static str> {
        match raw.split_once('@') {
            Some((name, host)) if !name.is_empty() && host.contains('.') => {
                Ok(Email(raw.to_owned()))
            }
            _ => Err("must be an address like name@example.com"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Username(String);

impl FromStr for Username {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Username, &'static str> {
        let fits = (3..=20).contains(&raw.chars().count());
        if fits && raw.chars().all(|char| char.is_ascii_alphanumeric()) {
            Ok(Username(raw.to_owned()))
        } else {
            Err("must be 3 to 20 letters or digits")
        }
    }
}

impl Username {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A topic is one of a fixed few: a list of them arrives as `signup[topics][]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topic(&'static str);

pub const TOPICS: [&str; 3] = ["rust", "elixir", "typescript"];

impl FromStr for Topic {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Topic, &'static str> {
        TOPICS
            .iter()
            .find(|topic| **topic == raw)
            .map(|topic| Topic(topic))
            .ok_or("is not a topic we know")
    }
}

/// The form and the Command are one struct.
#[derive(Changeset, Clone, Debug)]
pub struct Signup {
    pub email: Email,
    pub username: Username,
    pub topics: Vec<Topic>,
}

impl Signup {
    /// What to tell the user once the account exists.
    pub fn welcome(&self) -> String {
        let topics: Vec<_> = self.topics.iter().map(|topic| topic.0).collect();
        format!("Welcome, {} ({})", self.username.0, topics.join(", "))
    }
}

/// Stage two: a pure rule across fields. It cannot reach a database: it is given the
/// parsed fields and nothing else.
pub fn username_is_not_the_address(signup: &Signup, errors: &mut Errors) {
    let name = signup.email.0.split('@').next().unwrap_or_default();
    if name.eq_ignore_ascii_case(&signup.username.0) {
        errors.add("username", "must differ from the start of the email");
    }
}

/// The signup as a Changeset with stages one and two done.
pub fn check(changeset: Changeset<Signup>) -> Changeset<Signup> {
    changeset.validate(username_is_not_the_address)
}

/// Stage three: runs on submit only. The store decides whether the username is free,
/// as a unique index would, so no check beforehand can race another request. A taken
/// username comes back as an error on its field, for the form to show.
pub async fn register(
    app: &impl HasStore,
    changeset: Changeset<Signup>,
) -> Result<Signup, Changeset<Signup>> {
    let Some(signup) = changeset.valid() else {
        return Err(changeset);
    };
    if app.insert("usernames", signup.username.as_str()).await {
        Ok(signup.clone())
    } else {
        Err(changeset.add_error("username", "has already been taken"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use griffin_domain::Action;
    use std::collections::HashSet;
    use std::sync::Mutex;

    /// A Capability for a test: a set in memory, no web code anywhere.
    #[derive(Default)]
    struct Fake(Mutex<HashSet<String>>);

    impl HasStore for Fake {
        async fn insert(&self, _set: &str, key: &str) -> bool {
            self.0.lock().unwrap().insert(key.to_owned())
        }
    }

    fn signup(email: &str, username: &str) -> Changeset<Signup> {
        let pairs = [
            ("signup[email]", email),
            ("signup[username]", username),
            ("signup[topics][]", "rust"),
        ];
        check(Changeset::<Signup>::cast_form("signup", pairs)).with_action(Action::Insert)
    }

    fn message(changeset: &Changeset<Signup>, field: &str) -> String {
        let form = changeset.to_form();
        form.field(field).unwrap().errors.join("; ")
    }

    #[test]
    fn stage_one_refuses_what_does_not_parse() {
        let changeset = signup("not-an-email", "ada1");
        assert!(message(&changeset, "email").contains("name@example.com"));
        assert!(changeset.valid().is_none());
    }

    #[test]
    fn stage_two_compares_fields_without_io() {
        let changeset = signup("grace@example.com", "grace");
        assert!(message(&changeset, "username").contains("must differ"));
    }

    #[tokio::test]
    async fn stage_three_asks_the_capability_and_reports_a_taken_name_on_its_field() {
        let store = Fake::default();
        let first = register(&store, signup("a@example.com", "taken1")).await;
        assert!(first.unwrap().welcome().starts_with("Welcome, taken1"));
        let second = register(&store, signup("b@example.com", "taken1")).await;
        let changeset = second.unwrap_err();
        assert_eq!(message(&changeset, "username"), "has already been taken");
    }
}
