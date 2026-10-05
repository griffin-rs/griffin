//! Changesets for the Griffin web framework: <https://github.com/griffin-rs/griffin>.
//!
//! A [`struct@Changeset`] is the record of one attempt to turn untrusted params into
//! a trusted Command, in three stages (the first two run on every keystroke and cannot touch the database; the third runs on submit and relies on database constraints, as a pre-check query would race):
//!
//! 1. [`Changeset::cast`] parses each field with its type's [`FromStr`], the
//!    smart constructor of a domain newtype.
//! 2. [`Changeset::validate`] runs a pure cross-field rule.
//! 3. A Context function reports what only outside state can tell, such as
//!    uniqueness, with [`Changeset::add_error`].
//!
//! Stages one and two are synchronous and are handed nothing but strings and
//! parsed fields, so they cannot reach a database.
//!
//! ```
//! use std::collections::HashMap;
//!
//! use griffin_domain::{Action, Changeset, Errors};
//!
//! #[derive(Changeset)]
//! struct Booking {
//!     adults: u8,
//!     rooms: u8,
//! }
//!
//! fn a_room_needs_an_adult(booking: &Booking, errors: &mut Errors) {
//!     if booking.rooms > booking.adults {
//!         errors.add("rooms", "needs one adult per room");
//!     }
//! }
//!
//! let params = HashMap::from([
//!     ("adults".to_owned(), "1".to_owned()),
//!     ("rooms".to_owned(), "2".to_owned()),
//! ]);
//! let changeset = Changeset::<Booking>::cast(&params)
//!     .validate(a_room_needs_an_adult)
//!     .with_action(Action::Insert);
//!
//! match changeset.apply() {
//!     Ok(_booking) => unreachable!("two rooms for one adult"),
//!     Err(changeset) => {
//!         let form = changeset.to_form();
//!         assert_eq!(form.fields[1].value, "2");
//!         assert_eq!(form.fields[1].errors, ["needs one adult per room"]);
//!     }
//! }
//! ```
//!
//! `#[derive(Changeset)]` only writes the [`Cast`] impl; see [`Cast`] for the
//! same thing by hand.

use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;

/// Implements [`Cast`] for a struct with named fields, parsing each field
/// from the param of the same name.
pub use griffin_macros::Changeset;

/// Stage one: build `Self` from raw params, one [`Input::field`] per field.
///
/// `#[derive(Changeset)]` expands to exactly this:
///
/// ```
/// use griffin_domain::{Cast, Input};
///
/// struct Booking {
///     adults: u8,
///     rooms: u8,
/// }
///
/// impl Cast for Booking {
///     fn cast(input: &mut Input<'_>) -> Option<Self> {
///         // Read every field before the first `?` so each one reports its error.
///         let adults = input.field("adults");
///         let rooms = input.field("rooms");
///         Some(Self {
///             adults: adults?,
///             rooms: rooms?,
///         })
///     }
/// }
/// ```
pub trait Cast: Sized {
    fn cast(input: &mut Input<'_>) -> Option<Self>;
}

/// What a form can send under one name: text, a list of texts (`tags[]`), or the
/// names nested under it (`user[email]`).
#[derive(Debug)]
enum Param {
    Text(String),
    List(Vec<String>),
    Map(Params),
}

type Params = HashMap<String, Param>;

// What one form may send. They bound the memory and the stack a hostile body can
// use: the pairs past the limit, and a name that is deeper, longer or malformed, are
// ignored, which no form Griffin renders ever needs. A form with more pairs than the
// limit is not read past it, and `cast_form` makes it invalid with a form error, so
// that a shorter form than the user sent never applies: the client sends an `_unused_`
// marker beside each unused input, so 5000 pairs is a form of more than 2500 inputs.
// `griffin-web` keeps one pair more than the limit, so that `cast_form` can see it.
pub const MAX_PAIRS: usize = 5000;
const MAX_DEPTH: usize = 8;
const MAX_NAME_BYTES: usize = 256;

/// Reads a name as Phoenix's client writes it: `user[address][city]` is the path
/// `user`, `address`, `city`, and a trailing `[]` makes the value one of a list.
/// `None` for a name that is empty, malformed, too long or too deep.
fn parse_name(name: &str) -> Option<(Vec<&str>, bool)> {
    if name.len() > MAX_NAME_BYTES {
        return None;
    }
    let (first, mut rest) = name.split_at(name.find('[').unwrap_or(name.len()));
    if first.is_empty() || first.contains(']') {
        return None;
    }
    let mut path = vec![first];
    let mut list = false;
    while !rest.is_empty() {
        let inner = rest.strip_prefix('[')?;
        let (segment, after) = inner.split_once(']')?;
        if segment.contains('[') {
            return None;
        }
        if segment.is_empty() {
            // `[]` ends the name.
            if !after.is_empty() {
                return None;
            }
            list = true;
        } else {
            path.push(segment);
        }
        rest = after;
    }
    (path.len() <= MAX_DEPTH).then_some((path, list))
}

/// Puts `value` at `path`. A later pair replaces what an earlier one left there when
/// they disagree about its shape, as the last of two inputs of one name wins.
fn insert(params: &mut Params, path: &[&str], list: bool, value: String) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        match params.get_mut(*first) {
            Some(Param::List(items)) if list => items.push(value),
            _ if list => {
                params.insert((*first).to_owned(), Param::List(vec![value]));
            }
            _ => {
                params.insert((*first).to_owned(), Param::Text(value));
            }
        }
        return;
    }
    let entry = params
        .entry((*first).to_owned())
        .or_insert_with(|| Param::Map(Params::new()));
    if !matches!(entry, Param::Map(_)) {
        *entry = Param::Map(Params::new());
    }
    if let Param::Map(inner) = entry {
        insert(inner, rest, list, value);
    }
}

/// The raw params during stage one. Only [`Changeset::cast`] and
/// [`Changeset::cast_form`] create one.
pub struct Input<'a> {
    params: &'a Params,
    fields: Vec<FormField>,
    errors: Errors,
}

impl Input<'_> {
    /// Parses the param called `name`, keeping the raw input and, on failure,
    /// the smart constructor's error. A missing param is parsed as `""`.
    // FromStr is the whole field contract. An optional field needs a
    // Griffin-owned trait in its place; add it when a form needs one.
    pub fn field<T>(&mut self, name: &'static str) -> Option<T>
    where
        T: FromStr,
        T::Err: Display,
    {
        let sent = match self.params.get(name) {
            Some(Param::Text(text)) => Some(text.as_str()),
            // A list or a map sent where text is declared is not what the form has.
            _ => None,
        };
        let raw = sent.unwrap_or("");
        self.fields.push(FormField {
            name,
            value: raw.to_owned(),
            values: Vec::new(),
            errors: Vec::new(),
            touched: sent.is_some() && self.used(name),
        });
        raw.parse()
            .map_err(|error: T::Err| self.errors.add(name, error.to_string()))
            .ok()
    }

    /// Parses each item of the list called `name` (`tags[]` in a form), and keeps
    /// them as typed in [`FormField::values`], blank ones included, so the values
    /// line up with the inputs. An item that fails to parse adds an error for the
    /// field; a list that was not sent is empty.
    ///
    /// A browser sends no unchecked box, so a checkbox group writes a hidden empty
    /// item under the list's name, first, and a hidden `_sent_<name>` marker (spelled
    /// like `_unused_<name>`). The item makes the client treat the list as one input
    /// that can be unused (it adds `_unused_<name>` only for names that have a
    /// visible input); the marker says that item is the group's and not user data.
    /// When the marker is present the leading empty item is dropped and the list
    /// counts as sent even with nothing ticked. A list without a marker (text inputs)
    /// keeps every item, blank or not.
    pub fn list<T>(&mut self, name: &'static str) -> Option<Vec<T>>
    where
        T: FromStr,
        T::Err: Display,
    {
        let marked = self.params.contains_key(&format!("_sent_{name}"));
        let (sent, items) = match self.params.get(name) {
            Some(Param::List(items)) => match items.as_slice() {
                [first, rest @ ..] if marked && first.is_empty() => (true, rest.iter().collect()),
                items => (true, items.iter().collect::<Vec<&String>>()),
            },
            _ => (marked, Vec::new()),
        };
        self.fields.push(FormField {
            name,
            value: String::new(),
            values: items.iter().map(|item| (*item).clone()).collect(),
            errors: Vec::new(),
            touched: sent && self.used(name),
        });
        let mut parsed = Vec::with_capacity(items.len());
        for item in &items {
            match item.parse() {
                Ok(item) => parsed.push(item),
                Err(error) => self.errors.add(name, T::Err::to_string(&error)),
            }
        }
        (parsed.len() == items.len()).then_some(parsed)
    }

    /// Phoenix's client sends `_unused_<name>` beside inputs the user has not used yet.
    fn used(&self, name: &str) -> bool {
        !self.params.contains_key(&format!("_unused_{name}"))
    }
}

/// Error messages, in the order they were added: those of one field each, and those
/// of the form as a whole (form errors).
#[derive(Clone, Debug, Default)]
pub struct Errors {
    fields: Vec<(&'static str, String)>,
    form: Vec<String>,
}

impl Errors {
    pub fn add(&mut self, field: &'static str, message: impl Into<String>) {
        self.fields.push((field, message.into()));
    }

    /// An error on the form as a whole, not on one input.
    pub fn add_form(&mut self, message: impl Into<String>) {
        self.form.push(message.into());
    }
}

/// What the caller is attempting; a [`Form`] shows no errors until one is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    Validate,
    Insert,
    Update,
    Delete,
}

/// One attempt to turn params into a `T`: the raw input, the per-field errors
/// and the attempted action.
#[derive(Clone, Debug)]
pub struct Changeset<T> {
    // Boxed so that `Result<_, Changeset<T>>` stays small however large `T` is.
    command: Option<Box<T>>,
    /// The name the params were read from; empty for a flat cast.
    name: String,
    fields: Vec<FormField>,
    errors: Errors,
    action: Option<Action>,
}

impl<T: Cast> Changeset<T> {
    /// Stage one: parses every field of `T` from `params`.
    pub fn cast(params: &HashMap<String, String>) -> Self {
        let params = params
            .iter()
            .map(|(name, value)| (name.clone(), Param::Text(value.clone())))
            .collect();
        Self::from_params(&params)
    }

    /// Stage one for a form as the browser sends it: the pairs of its body, in
    /// order, with names such as `user[email]` and `tags[]`. Only the fields of `T`
    /// are read, from under `name` (`"user"`); anything else sent is ignored.
    ///
    /// The pairs are untrusted. At most 5000 are read, and a name longer than 256
    /// bytes, nested deeper than 8 names or malformed is ignored, so a hostile body
    /// cannot use more memory or stack than a form of that size would. A form with
    /// more than [`MAX_PAIRS`] pairs is invalid: the pairs past the limit are not
    /// read, so the Changeset gets a form error and `apply` and `valid` fail, rather
    /// than reading a shorter form than the user sent.
    ///
    /// ```
    /// use griffin_domain::{Action, Changeset};
    ///
    /// #[derive(Changeset)]
    /// struct Signup {
    ///     email: String,
    ///     tags: Vec<String>,
    /// }
    ///
    /// let body = [
    ///     ("signup[email]", "ada@example.com"),
    ///     ("signup[tags][]", "math"),
    ///     ("signup[tags][]", "engines"),
    /// ];
    /// let changeset = Changeset::<Signup>::cast_form("signup", body);
    /// let signup = changeset.apply().ok().unwrap();
    /// assert_eq!(signup.tags, ["math", "engines"]);
    /// ```
    pub fn cast_form<N, V>(name: &str, pairs: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: AsRef<str>,
        V: Into<String>,
    {
        let mut params = Params::new();
        let mut pairs = pairs.into_iter();
        for (name, value) in pairs.by_ref().take(MAX_PAIRS) {
            if let Some((path, list)) = parse_name(name.as_ref()) {
                insert(&mut params, &path, list, value.into());
            }
        }
        // One more pair means the pairs past the limit were not read.
        let overflow = pairs.next().is_some();
        // An empty name is a flat form: the names are read at the root.
        let grouped = match params.remove(name) {
            _ if name.is_empty() => params,
            Some(Param::Map(grouped)) => grouped,
            _ => Params::new(),
        };
        let changeset = Self {
            name: name.to_owned(),
            ..Self::from_params(&grouped)
        };
        if overflow {
            changeset.add_form_error(
                "The form is too large: some of what was sent was ignored. Please send fewer fields.",
            )
        } else {
            changeset
        }
    }

    fn from_params(params: &Params) -> Self {
        let mut input = Input {
            params,
            fields: Vec::new(),
            errors: Errors::default(),
        };
        Self {
            command: T::cast(&mut input).map(Box::new),
            name: String::new(),
            fields: input.fields,
            errors: input.errors,
            action: None,
        }
    }
}

impl<T> Changeset<T> {
    /// Stage two: runs a cross-field rule over the parsed fields. The rule is
    /// skipped while any field fails to parse, so it only ever sees trusted
    /// values. It is a plain `fn`, not a closure, so it cannot capture a
    /// database handle or any other Capability.
    pub fn validate(mut self, rule: fn(&T, &mut Errors)) -> Self {
        if let Some(command) = &self.command {
            rule(command, &mut self.errors);
        }
        self
    }

    /// Stage three: a Context function records an error that only outside
    /// state could reveal. The Changeset no longer applies.
    pub fn add_error(mut self, field: &'static str, message: impl Into<String>) -> Self {
        self.errors.add(field, message);
        self
    }

    /// An error on the form as a whole, not on one input ("email or password is
    /// incorrect"). Usable at any stage; it makes the Changeset invalid like a field
    /// error does.
    pub fn add_form_error(mut self, message: impl Into<String>) -> Self {
        self.errors.add_form(message);
        self
    }

    /// The errors of the form as a whole, in the order they were added.
    pub fn form_errors(&self) -> &[String] {
        &self.errors.form
    }

    fn is_valid_so_far(&self) -> bool {
        self.errors.fields.is_empty() && self.errors.form.is_empty()
    }

    /// The errors of fields only. Errors of the form as a whole are
    /// [`form_errors`](Self::form_errors).
    pub fn errors(&self) -> &[(&'static str, String)] {
        &self.errors.fields
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    /// The Command if every stage so far passed, without consuming the Changeset: a
    /// Context function reads it, does its work, and can still [`add_error`](Self::add_error)
    /// on the Changeset it holds when the work finds what only outside state shows.
    pub fn valid(&self) -> Option<&T> {
        self.command.as_deref().filter(|_| self.is_valid_so_far())
    }

    /// The Command if every stage passed, otherwise the Changeset back.
    pub fn apply(mut self) -> Result<T, Self> {
        if self.is_valid_so_far()
            && let Some(command) = self.command.take()
        {
            return Ok(*command);
        }
        Err(self)
    }

    /// The template-side view: what the user typed, and the errors once an
    /// action is set.
    pub fn to_form(&self) -> Form {
        let shown = self.errors().iter().filter(|_| self.action.is_some());
        let mut fields = self.fields.clone();
        for (name, message) in shown {
            for field in fields.iter_mut().filter(|field| field.name == *name) {
                field.errors.push(message.clone());
            }
        }
        Form {
            name: self.name.clone(),
            action: self.action,
            form_errors: self
                .errors
                .form
                .iter()
                .filter(|_| self.action.is_some())
                .cloned()
                .collect(),
            fields,
        }
    }
}

/// Plain data for rendering a form; fields are in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    /// The name the inputs are grouped under, `user` for `user[email]`: what
    /// [`Changeset::cast_form`] read the params from. Empty for a flat cast.
    pub name: String,
    pub action: Option<Action>,
    /// Errors on the form as a whole, shown once an action is set, like field errors.
    pub form_errors: Vec<String>,
    pub fields: Vec<FormField>,
}

impl Form {
    /// The field called `name`, if the Changeset declares one.
    pub fn field(&self, name: &str) -> Option<&FormField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormField {
    pub name: &'static str,
    /// What the user typed, valid or not. Empty for a list field.
    pub value: String,
    /// What the user typed in a list field (`tags[]`), item by item, blank
    /// items included. Empty for a field that is not a list.
    pub values: Vec<String>,
    pub errors: Vec<String>,
    /// Whether the user has used this input: it was sent, and without an
    /// `_unused_<name>` marker beside it.
    pub touched: bool,
}
