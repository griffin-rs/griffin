//! The domain side of the signup form: the Changeset, the Capability the Context
//! needs, and the Context function that runs stage three.
//!
//! Nothing here knows about HTTP or LiveView. In a generated project this module would sit in the domain
//! crate, which depends on `griffin-domain` only, so the compiler, not convention, keeps HTTP out.

use griffin_domain::{Changeset, Errors};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

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

/// The form and the Command are one struct: the form's fields are the Command's, so nothing is copied between them. Split them when their shapes differ.
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

/// Stage two: a pure rule across fields. It runs on every change, and cannot reach a
/// database: it is given the parsed fields and nothing else.
pub fn username_is_not_the_address(signup: &Signup, errors: &mut Errors) {
    let name = signup.email.0.split('@').next().unwrap_or_default();
    if name.eq_ignore_ascii_case(&signup.username.0) {
        errors.add("username", "must differ from the start of the email");
    }
}

/// The signup as a Changeset, with stages one and two done.
pub fn check(changeset: Changeset<Signup>) -> Changeset<Signup> {
    changeset.validate(username_is_not_the_address)
}

/// The Capability: what the Context needs of the world outside, as a trait. The
/// Context is written against it, so a test or an example uses a store in memory.
pub trait Accounts: Send + Sync {
    /// Stores the account, unless the username is taken.
    fn register(&self, signup: &Signup) -> impl Future<Output = Result<(), Taken>> + Send;
}

#[derive(Debug)]
pub struct Taken;

/// Stage three: runs on submit only, never on a change. The store is where the
/// uniqueness is decided, as a database constraint would: no check beforehand that
/// another request could race. A taken username comes back as an error on its field.
pub async fn register(
    accounts: &impl Accounts,
    changeset: Changeset<Signup>,
) -> Result<Signup, Changeset<Signup>> {
    let Some(signup) = changeset.valid() else {
        return Err(changeset);
    };
    match accounts.register(signup).await {
        Ok(()) => Ok(signup.clone()),
        Err(Taken) => Err(changeset.add_error("username", "has already been taken")),
    }
}

/// The store of the process. `main` puts it in the application state, from which the
/// controller takes it; the LiveView reaches it here.
// A workaround for LiveViews not yet receiving Capabilities: `mount` is given
// the route's parameters and nothing from the application state. Remove it, and have the
// LiveView take the store from the state like the controller, when it can.
pub fn store() -> Arc<Memory> {
    static STORE: LazyLock<Arc<Memory>> =
        LazyLock::new(|| Arc::new(Memory::new(std::env::var_os("FORMS_GATE").is_some())));
    STORE.clone()
}

/// An in-memory store with one account in it, `ada`.
pub struct Memory {
    usernames: Mutex<HashSet<String>>,
    /// Lets a browser test hold a submit in flight and then release it. See
    /// `FORMS_GATE` in `main.rs`: it is for the tests of this example, not for lifting.
    gate: Option<Notify>,
}

impl Memory {
    pub fn new(gated: bool) -> Memory {
        Memory {
            usernames: Mutex::new(HashSet::from(["ada".to_owned()])),
            gate: gated.then(Notify::new),
        }
    }

    /// Releases the submit of a username that starts with `hold`, now or when it arrives.
    pub fn open_gate(&self) {
        if let Some(gate) = &self.gate {
            gate.notify_one();
        }
    }
}

impl Accounts for Memory {
    async fn register(&self, signup: &Signup) -> Result<(), Taken> {
        if let Some(gate) = &self.gate
            && signup.username.as_str().starts_with("hold")
        {
            gate.notified().await;
        }
        let new = self
            .usernames
            .lock()
            .expect("nothing panics holding it")
            .insert(signup.username.0.clone());
        if new { Ok(()) } else { Err(Taken) }
    }
}
