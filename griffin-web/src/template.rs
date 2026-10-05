//! The template model: what a template is once it has rendered, as plain data.
//!
//! This is the low-level layer that the [`html!`](crate::html) macro and
//! `.html.griffin` files expand to (every macro lowers to a public API). Everything here can be written by hand,
//! and behaviour is specified and tested on this API; a macro only translates syntax
//! into these calls. A [`Component`] is the same for reusable pieces of template: what
//! a capitalised tag calls, and what [`#[component]`](macro@crate::component) writes.
//!
//! A [`Rendered`] is one render of a template site: the static parts of its HTML, the
//! dynamic [`Slot`]s between them, and a fingerprint naming the site. It becomes either
//! an HTML string, for the Dead render, or diffs in Phoenix's wire protocol (the wire protocol is Phoenix's, unchanged),
//! for a connected LiveView, through a [`DiffState`]:
//!
//! ```
//! use griffin_web::template::{DiffState, Rendered, Slot};
//! use serde_json::json;
//!
//! // What a macro would generate for `<p>Hello, {name}!</p>`.
//! fn greeting(name: &str) -> Rendered {
//!     Rendered::new(0x5eed, &["<p>Hello, ", "!</p>"], vec![Slot::text(name)]).single_root()
//! }
//!
//! assert_eq!(greeting("<Ann>").to_html(), "<p>Hello, &lt;Ann&gt;!</p>");
//!
//! let mut state = DiffState::new();
//! // The first render carries the statics,
//! assert_eq!(
//!     state.render(greeting("Ann")),
//!     json!({"0": "Ann", "s": 0, "p": {"0": ["<p>Hello, ", "!</p>"]}, "r": 1})
//! );
//! // later ones only the slots that changed,
//! assert_eq!(state.render(greeting("Bob")), json!({"0": "Bob"}));
//! // and nothing when nothing did.
//! assert_eq!(state.render(greeting("Bob")), json!({}));
//! ```

mod component;
mod diff;

use std::collections::HashSet;
use std::fmt::{self, Write as _};

pub use component::{Attributes, Component, GlobalAttributes, SlotEntries, SlotEntry, require};
pub use diff::DiffState;

/// One render of a template site: static parts with a dynamic [`Slot`] between each
/// pair, so the HTML is `statics[0]`, `slots[0]`, `statics[1]`, and so on to the last static.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    fingerprint: u64,
    statics: &'static [&'static str],
    slots: Vec<Slot>,
    root: bool,
}

impl Rendered {
    /// A render of the template site named by `fingerprint`.
    ///
    /// The fingerprint stands for the statics: a client that has seen a fingerprint is
    /// not sent its statics again. Every render of one site must pass the same
    /// fingerprint and statics, and two sites with different statics must not share a
    /// fingerprint. A macro derives it from the statics at compile time.
    ///
    /// # Panics
    ///
    /// If there is not exactly one more static than there are slots.
    pub fn new(fingerprint: u64, statics: &'static [&'static str], slots: Vec<Slot>) -> Rendered {
        assert_eq!(
            statics.len(),
            slots.len() + 1,
            "a template has one static before each slot and one after the last"
        );
        Rendered {
            fingerprint,
            statics,
            slots,
            root: false,
        }
    }

    /// Declares that the whole template is exactly one element, with no text, comment
    /// or second element beside it. The client is told (`"r": 1`) and may then skip
    /// patching the element when nothing in it changed. Declaring it for any other
    /// template is a bug.
    pub fn single_root(mut self) -> Rendered {
        self.root = true;
        self
    }

    /// The full HTML of this render.
    pub fn to_html(&self) -> String {
        let mut html = String::new();
        self.write_html(&mut html);
        html
    }

    fn write_html(&self, html: &mut String) {
        html.push_str(self.statics[0]);
        for (slot, after) in self.slots.iter().zip(&self.statics[1..]) {
            match &slot.0 {
                Content::Html(slot) => html.push_str(slot),
                Content::Template(template) => template.write_html(html),
                Content::Comprehension(list) => {
                    list.entries
                        .iter()
                        .for_each(|(_, entry)| entry.write_html(html));
                }
            }
            html.push_str(after);
        }
    }
}

/// The dynamic content at one position of a [`Rendered`] template.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot(Content);

#[derive(Debug, Clone, PartialEq)]
enum Content {
    Html(String),
    Template(Rendered),
    Comprehension(Comprehension),
}

#[derive(Debug, Clone, PartialEq)]
struct Comprehension {
    fingerprint: u64,
    statics: &'static [&'static str],
    /// Each entry under its key, which no other entry has: one render of the
    /// comprehension's template, so all share the fingerprint and statics above.
    // An unkeyed entry's key is its index as a string, allocated on every
    // render. Make the key an enum of index or string if lists show up in a profile.
    entries: Vec<(String, Rendered)>,
}

impl Slot {
    /// Text from any source, trusted or not. `<`, `>`, `&`, `"` and `'` are escaped,
    /// so it is safe as element content and inside a quoted attribute value.
    ///
    /// It is not safe where HTML escaping is not the rule that applies: an unquoted
    /// attribute value, the body of `<script>` or `<style>`, or a URL scheme.
    ///
    /// # Panics
    ///
    /// If `value`'s `Display` implementation returns an error, as `to_string` does.
    pub fn text(value: impl fmt::Display) -> Slot {
        let mut html = String::new();
        write!(Escaped(&mut html), "{value}").expect("a Display implementation returned an error");
        Slot(Content::Html(html))
    }

    /// HTML that is sent to the browser exactly as given, with nothing escaped.
    ///
    /// This is the only way around escaping. The caller vouches that `html` is
    /// well-formed and that no part of it comes from user input that has not been
    /// escaped or sanitised; anything else is a cross-site scripting hole.
    pub fn raw_html(html: impl Into<String>) -> Slot {
        Slot(Content::Html(html.into()))
    }

    /// A nested template: its own statics and slots, with its own fingerprint. While
    /// the slot keeps holding the same template site, a diff carries only the nested
    /// slots that changed; when it holds another one, as after a switch between the
    /// branches of an `if`, the new statics are sent once more.
    ///
    /// ```
    /// use griffin_web::template::{DiffState, Rendered, Slot};
    /// use serde_json::json;
    ///
    /// // What a macro would generate for `<div><p :if={show}>Hello, {name}!</p></div>`.
    /// fn greeting(show: bool, name: &str) -> Rendered {
    ///     let hello = if show {
    ///         Slot::template(Rendered::new(2, &["<p>Hello, ", "!</p>"], vec![Slot::text(name)]))
    ///     } else {
    ///         Slot::text("")
    ///     };
    ///     Rendered::new(1, &["<div>", "</div>"], vec![hello]).single_root()
    /// }
    ///
    /// assert_eq!(greeting(true, "Ann").to_html(), "<div><p>Hello, Ann!</p></div>");
    /// assert_eq!(greeting(false, "Ann").to_html(), "<div></div>");
    ///
    /// let mut state = DiffState::new();
    /// state.render(greeting(false, "Ann"));
    /// // Turning it on sends the nested statics,
    /// assert_eq!(
    ///     state.render(greeting(true, "Ann")),
    ///     json!({"0": {"0": "Ann", "s": 0}, "p": {"0": ["<p>Hello, ", "!</p>"]}})
    /// );
    /// // staying on sends only what changed inside,
    /// assert_eq!(state.render(greeting(true, "Bob")), json!({"0": {"0": "Bob"}}));
    /// // and turning it off sends the empty text.
    /// assert_eq!(state.render(greeting(false, "Bob")), json!({"0": ""}));
    /// ```
    pub fn template(rendered: Rendered) -> Slot {
        Slot(Content::Template(rendered))
    }

    /// A comprehension: one template site rendered once for each entry of a list, as
    /// by `:for`. Every entry has its own slots and all share one set of statics, which
    /// a diff carries once however many entries there are. An empty list is nothing.
    ///
    /// An entry is the one at its position in the previous render: a diff carries the
    /// slots that changed at each position, the entries past the previous end, and the
    /// new length. Use [`Slot::keyed_comprehension`] for a list whose entries move.
    ///
    /// ```
    /// use griffin_web::template::{DiffState, Rendered, Slot};
    /// use serde_json::json;
    ///
    /// // What a macro would generate for `<ul><li :for={name in names}>{name}</li></ul>`.
    /// fn guests(names: &[&str]) -> Rendered {
    ///     let entries = names.iter().map(|name| vec![Slot::text(name)]);
    ///     let list = Slot::comprehension(2, &["<li>", "</li>"], entries);
    ///     Rendered::new(1, &["<ul>", "</ul>"], vec![list]).single_root()
    /// }
    ///
    /// assert_eq!(guests(&["Ann", "Bob"]).to_html(), "<ul><li>Ann</li><li>Bob</li></ul>");
    /// assert_eq!(guests(&[]).to_html(), "<ul></ul>");
    ///
    /// let mut state = DiffState::new();
    /// // The first render carries the statics of an entry once (`"s"`), and each entry
    /// // (`"k"`) with their number (`"kc"`).
    /// assert_eq!(
    ///     state.render(guests(&["Ann", "Bob"])),
    ///     json!({
    ///         "0": {"k": {"0": {"0": "Ann"}, "1": {"0": "Bob"}, "kc": 2}, "s": 0},
    ///         "p": {"0": ["<li>", "</li>"], "1": ["<ul>", "</ul>"]},
    ///         "s": 1,
    ///         "r": 1,
    ///     })
    /// );
    /// // Later ones carry the entries that changed or are new, and the new length.
    /// assert_eq!(
    ///     state.render(guests(&["Ann", "Cy", "Di"])),
    ///     json!({"0": {"k": {"1": {"0": "Cy"}, "2": {"0": "Di"}, "kc": 3}}})
    /// );
    /// assert_eq!(state.render(guests(&["Ann"])), json!({"0": {"k": {"kc": 1}}}));
    /// ```
    ///
    /// # Panics
    ///
    /// If an entry does not have exactly one slot fewer than there are statics.
    pub fn comprehension(
        fingerprint: u64,
        statics: &'static [&'static str],
        entries: impl IntoIterator<Item = Vec<Slot>>,
    ) -> Slot {
        Slot::keyed_comprehension(fingerprint, statics, entries.into_iter().enumerate())
    }

    /// A comprehension whose entries each have a key, as by `:for` with `:key`: an
    /// entry is the one that had its key in the previous render, wherever that was. A
    /// diff then says where each entry moved from and carries only the slots that
    /// changed in it, so a reordered list costs a few numbers and the browser keeps
    /// each entry's elements. Keys are compared as the text they display as.
    ///
    /// ```
    /// use griffin_web::template::{DiffState, Rendered, Slot};
    /// use serde_json::json;
    ///
    /// // What a macro would generate for
    /// // `<ul><li :for={(id, name) in guests} :key={id}>{name}</li></ul>`.
    /// fn guests(guests: &[(u32, &str)]) -> Rendered {
    ///     let entries = guests.iter().map(|(id, name)| (id, vec![Slot::text(name)]));
    ///     let list = Slot::keyed_comprehension(2, &["<li>", "</li>"], entries);
    ///     Rendered::new(1, &["<ul>", "</ul>"], vec![list]).single_root()
    /// }
    ///
    /// let mut state = DiffState::new();
    /// state.render(guests(&[(1, "Ann"), (2, "Bob"), (3, "Cy")]));
    /// // The entry now first was third, the second was first and was renamed, and the
    /// // third was second: `"km"` says that entries moved.
    /// assert_eq!(
    ///     state.render(guests(&[(3, "Cy"), (1, "Anna"), (2, "Bob")])),
    ///     json!({"0": {"k": {"0": 2, "1": [0, {"0": "Anna"}], "2": 1, "kc": 3, "km": true}}})
    /// );
    /// ```
    ///
    /// # Panics
    ///
    /// If two entries have the same key, or an entry does not have exactly one slot
    /// fewer than there are statics.
    pub fn keyed_comprehension<K: fmt::Display>(
        fingerprint: u64,
        statics: &'static [&'static str],
        entries: impl IntoIterator<Item = (K, Vec<Slot>)>,
    ) -> Slot {
        let entries = entries.into_iter();
        let entries: Vec<_> = entries
            .map(|(key, slots)| (key.to_string(), Rendered::new(fingerprint, statics, slots)))
            .collect();
        let mut keys = HashSet::new();
        for (key, _) in &entries {
            assert!(
                keys.insert(key),
                "two entries of a comprehension have the key {key:?}"
            );
        }
        Slot(Content::Comprehension(Comprehension {
            fingerprint,
            statics,
            entries,
        }))
    }

    /// One attribute whose presence or value is dynamic, for a slot inside an opening
    /// tag: `` name="value"`` with the value escaped, the bare `` name``, or nothing,
    /// as the [`AttributeValue`] says. The space before the name is part of the slot.
    ///
    /// ```
    /// use griffin_web::template::Slot;
    ///
    /// // What a macro would generate for the slot in `<input disabled={busy}>`,
    /// // between the statics `<input` and `>`.
    /// let busy = true;
    /// assert_eq!(Slot::attribute("disabled", &busy), Slot::raw_html(" disabled"));
    /// ```
    pub fn attribute(name: &str, value: impl Into<AttributeValue>) -> Slot {
        Slot::attributes([(name, value)])
    }

    /// Any number of attributes decided at run time, each written as by
    /// [`Slot::attribute`], in the order given.
    ///
    /// Names are escaped like values, but escaping cannot make a name safe: whoever
    /// picks the name picks what the attribute does (`onclick`, `href`). Never take
    /// names from user input.
    pub fn attributes<N, V>(attributes: impl IntoIterator<Item = (N, V)>) -> Slot
    where
        N: AsRef<str>,
        V: Into<AttributeValue>,
    {
        fn write(html: &mut String, name: &str, value: Option<&str>) {
            html.push(' ');
            escape(name, html);
            if let Some(value) = value {
                html.push_str("=\"");
                escape(value, html);
                html.push('"');
            }
        }
        let mut html = String::new();
        for (name, value) in attributes {
            let (value, fields) = match value.into() {
                AttributeValue::Omitted => continue,
                AttributeValue::Bare => (None, Vec::new()),
                AttributeValue::Value(value) => (Some(value), Vec::new()),
                AttributeValue::Event { name, values } => (Some(name.to_owned()), values),
            };
            write(&mut html, name.as_ref(), value.as_deref());
            for (field, value) in fields {
                write(&mut html, &format!("phx-value-{field}"), Some(&value));
            }
        }
        Slot(Content::Html(html))
    }
}

/// What a dynamic attribute is in one render. See [`Slot::attribute`].
///
/// Values convert from references, because a template only reads its state: `&bool`
/// is [`Bare`](AttributeValue::Bare) or [`Omitted`](AttributeValue::Omitted),
/// `&Option<T>` is `T`'s value or `Omitted`, and `&str`, `&String`, `&char` and
/// references to the number types are a [`Value`](AttributeValue::Value). They
/// convert from the values themselves too, which is how a tag gives one to a
/// Component ([`GlobalAttributes`]). A reference to an [`Event`](crate::live::Event)
/// is an [`Event`](AttributeValue::Event) here: that is what binds it to an element,
/// as in `phx-click={event}`. A reference to client commands
/// ([`Js`](crate::live::Js)) is a `Value`: the operations as JSON.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeValue {
    /// The attribute is left out of the element.
    Omitted,
    /// The name alone, as in `<input disabled>`.
    Bare,
    /// `name="value"`. The value is escaped when written.
    Value(String),
    /// An [`Event`](crate::live::Event) bound to the element, as the Phoenix client
    /// reads it: the attribute with the Event's name as its value, then a
    /// `phx-value-<field>="value"` attribute for each field. All of it is escaped.
    Event {
        /// The Event's name on the wire.
        name: &'static str,
        /// Each field, with its value as text.
        values: Vec<(&'static str, String)>,
    },
}

impl From<&AttributeValue> for AttributeValue {
    fn from(value: &AttributeValue) -> AttributeValue {
        value.clone()
    }
}

impl From<&bool> for AttributeValue {
    fn from(present: &bool) -> AttributeValue {
        if *present {
            AttributeValue::Bare
        } else {
            AttributeValue::Omitted
        }
    }
}

impl<'a, T> From<&'a Option<T>> for AttributeValue
where
    &'a T: Into<AttributeValue>,
{
    fn from(value: &'a Option<T>) -> AttributeValue {
        value.as_ref().map_or(AttributeValue::Omitted, Into::into)
    }
}

impl From<bool> for AttributeValue {
    fn from(present: bool) -> AttributeValue {
        (&present).into()
    }
}

impl<T: Into<AttributeValue>> From<Option<T>> for AttributeValue {
    fn from(value: Option<T>) -> AttributeValue {
        value.map_or(AttributeValue::Omitted, Into::into)
    }
}

// Not a blanket impl over `Display`: `bool` is `Display` too, and must not come out
// as `disabled="false"`.
macro_rules! attribute_value_from_display {
    ($($type:ty)*) => {$(
        impl From<$type> for AttributeValue {
            fn from(value: $type) -> AttributeValue {
                AttributeValue::Value(value.to_string())
            }
        }
    )*};
}
macro_rules! attribute_value_from_display_and_reference {
    ($($type:ty)*) => { attribute_value_from_display!($($type &$type)*); };
}
attribute_value_from_display!(&str && str);
attribute_value_from_display_and_reference!(String char f32 f64);
attribute_value_from_display_and_reference!(i8 i16 i32 i64 i128 isize u8 u16 u32 u64 u128 usize);

/// What `{expression}` in a template becomes: the value as text, escaped as by
/// [`Slot::text`].
impl<T: fmt::Display + ?Sized> From<&T> for Slot {
    fn from(value: &T) -> Slot {
        Slot::text(value)
    }
}

/// A slot stays what it is, which is how `{Slot::raw_html(trusted)}` in a template
/// gets around escaping.
impl From<&Slot> for Slot {
    fn from(slot: &Slot) -> Slot {
        slot.clone()
    }
}

/// What an `{expression}` whose value is a template becomes, such as an `if` or a
/// `match` with a template in each branch: a nested template, as by [`Slot::template`].
impl From<&Rendered> for Slot {
    // An expression is borrowed, so the nested template is cloned once per
    // level on every render. Have the macro move it if rendering shows up in a profile.
    fn from(rendered: &Rendered) -> Slot {
        Slot::template(rendered.clone())
    }
}

/// Escapes what is written through it: the same five characters, to the same
/// entities, as Phoenix, so escaped text is byte-identical on the wire.
struct Escaped<'a>(&'a mut String);

impl fmt::Write for Escaped<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        escape(text, self.0);
        Ok(())
    }
}

fn escape(text: &str, html: &mut String) {
    for char in text.chars() {
        match char {
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '&' => html.push_str("&amp;"),
            '"' => html.push_str("&quot;"),
            '\'' => html.push_str("&#39;"),
            other => html.push(other),
        }
    }
}
