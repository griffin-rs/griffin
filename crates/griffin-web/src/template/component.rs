//! Components: what a capitalised tag in a template calls.

use std::ops::Deref;

use super::{AttributeValue, Rendered, Slot};

/// One method for each of HTML's global attributes that is a Rust identifier, which
/// is Phoenix's list. A Component that declares an attribute of one of these names
/// has a method of its own, which a tag calls instead.
macro_rules! global_attributes {
    ($($name:ident)*) => {$(
        #[doc = concat!("Takes the HTML attribute `", stringify!($name), "`.")]
        fn $name(self, value: impl Into<AttributeValue>) -> Self {
            self.attribute(stringify!($name), value)
        }
    )*};
}

/// A reusable piece of template with typed attributes, called from another template
/// as a capitalised tag: `<Badge label="new" count={unread} />`.
///
/// The usual way to define one is a function under
/// [`#[component]`](macro@crate::component). This trait is what that expands to, and
/// what a tag is written against (every macro lowers to a public API), so a Component can also be written by
/// hand. A tag expands to
///
/// 1. `Default::default()`, the Component with no attribute given yet,
/// 2. a call of the method named after each attribute, in the order written, with its
///    value: `.label("new")`, then `.count(unread)`. A bare attribute passes `true`.
///    A name that is not a Rust identifier, such as `data-id`, goes to
///    [`GlobalAttributes::attribute`] instead,
/// 3. for each `<:name>` between the tags, in the order written, a call of the method
///    named after the slot, with a closure that is handed the slot and pushes the
///    entry onto it: `.col(|slot| { slot.push(|user| ..); })`, where the inner
///    closure is the entry's content, a template, and its parameter is the pattern
///    of `:let`. The attributes of the entry are assigned to what
///    [`push`](SlotEntries::push) returns. Any other content between the tags is
///    given the same way to the default slot, `inner_block`,
/// 4. [`Component::render`], whose result fills the slot as a nested template,
///
/// and to a compile-time check, [`require`], that every name in
/// [`REQUIRED`](Component::REQUIRED) and [`REQUIRED_SLOTS`](Component::REQUIRED_SLOTS)
/// was given. So a wrong type is a type error at the value, and an attribute or a slot
/// the Component does not have is a missing method at its name. [`SlotEntries`] shows a
/// Component with slots written by hand.
///
/// ```
/// use griffin_web::html;
/// use griffin_web::template::{Component, Rendered};
///
/// #[derive(Default)]
/// struct Badge<'a> {
///     label: &'a str,
///     count: u32,
/// }
///
/// impl<'a> Badge<'a> {
///     fn label(mut self, label: &'a str) -> Self {
///         self.label = label;
///         self
///     }
///
///     fn count(mut self, count: u32) -> Self {
///         self.count = count;
///         self
///     }
/// }
///
/// impl Component for Badge<'_> {
///     const REQUIRED: &'static [&'static str] = &["label"];
///
///     fn render(self) -> Rendered {
///         html! { <span class="badge">{self.label} {self.count}</span> }
///     }
/// }
///
/// let page = html! { <p>Inbox <Badge label="new" count={3} /> Drafts <Badge label="none" /></p> };
/// assert_eq!(
///     page.to_html(),
///     "<p>Inbox <span class=\"badge\">new 3</span> Drafts <span class=\"badge\">none 0</span></p>"
/// );
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a Component",
    label = "a capitalised tag calls a Component",
    note = "define one with `#[griffin_web::component]`, or write lower case for an HTML element"
)]
pub trait Component: Default {
    /// The attributes a tag must give. Leaving one out is a compile error at the tag.
    const REQUIRED: &'static [&'static str] = &[];

    /// The slots a tag must give, the default one being `inner_block`. Leaving one
    /// out is a compile error at the tag.
    const REQUIRED_SLOTS: &'static [&'static str] = &[];

    /// The Component's template, rendered from the attributes given.
    fn render(self) -> Rendered;
}

/// The extra HTML attributes a tag gave to a Component that opts in to them, in the
/// order written. In the Component's template, `<button {rest}>` writes them out.
pub type Attributes = Vec<(&'static str, AttributeValue)>;

/// A [`Component`] that opts in to extra HTML attributes: ones it does not declare,
/// which it usually forwards to its root element, as `attr :rest, :global` does in
/// Phoenix. `#[component]` implements this for a function with a `#[global]` parameter.
///
/// A tag gives an attribute the Component has no method for through this trait: a
/// name that is not a Rust identifier, such as `data-id`, `aria-label` or `phx-click`,
/// to [`attribute`](GlobalAttributes::attribute), and one of HTML's global attributes,
/// such as `class` or `id`, to the method of its name here. Any other name is a
/// compile error, so an attribute that is specific to one element, such as
/// `placeholder`, has to be declared by the Component.
///
/// ```
/// use griffin_web::template::{Attributes, Rendered};
/// use griffin_web::{component, html};
///
/// #[component]
/// fn Button(label: &str, #[global] rest: Attributes) -> Rendered {
///     html! { <button {rest}>{label}</button> }
/// }
///
/// let page = html! { <Button label="Save" class="primary" data-id={7} /> };
/// assert_eq!(page.to_html(), "<button class=\"primary\" data-id=\"7\">Save</button>");
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not take extra HTML attributes",
    label = "not an attribute of this Component",
    note = "a Component opts in with a `#[global]` parameter of type `Attributes`"
)]
pub trait GlobalAttributes: Component {
    /// Takes one extra attribute. The name is written in a template, never user input.
    fn attribute(self, name: &'static str, value: impl Into<AttributeValue>) -> Self;

    global_attributes! {
        accesskey anchor autocapitalize autocorrect autofocus class contenteditable dir
        draggable enterkeyhint exportparts hidden id inert inputmode is itemid itemprop
        itemref itemscope itemtype lang nonce onabort onautocomplete onautocompleteerror
        onblur oncancel oncanplay oncanplaythrough onchange onclick onclose oncontextmenu
        oncuechange ondblclick ondrag ondragend ondragenter ondragleave ondragover
        ondragstart ondrop ondurationchange onemptied onended onerror onfocus oninput
        oninvalid onkeydown onkeypress onkeyup onload onloadeddata onloadedmetadata
        onloadstart onmousedown onmouseenter onmouseleave onmousemove onmouseout
        onmouseover onmouseup onmousewheel onpause onplay onplaying onprogress
        onratechange onreset onresize onscroll onseeked onseeking onselect onshow onsort
        onstalled onsubmit onsuspend ontimeupdate ontoggle onvolumechange onwaiting part
        popover role slot spellcheck style tabindex title translate
        virtualkeyboardpolicy writingsuggestions
    }
}

/// What a tag gave to one slot of a [`Component`]: the caller's own template, for the
/// Component to render where it likes. Not to be confused with a [`Slot`], which is
/// one dynamic position of a rendered template.
///
/// A slot holds an entry for each time the tag gave it: content between the
/// Component's tags is one entry of the default slot, `inner_block`, and each
/// `<:name>` inside them is one entry of the slot `name`. A slot nothing was given to
/// is empty and renders as nothing.
///
/// - `T` is the argument the Component passes to the content when it renders it,
///   which the tag binds with `:let={pattern}`. It is `()` for a slot with none.
/// - `A` holds the attributes of an entry, `<:col label="Name">`: any struct with a
///   [`Default`] and a public field for each attribute, which the tag assigns to.
///   It is `()` for a slot whose entries have none.
///
/// In the Component's template, `{inner_block}` renders a slot without an argument,
/// and `{inner_block.render(argument)}` one with. A slot is also a slice of its
/// entries, for `:for={col in &col}`: each [`SlotEntry`] renders the same two ways,
/// and reads its attributes as fields, `{col.label}`.
///
/// A table whose columns are the entries of a slot, written by hand. With
/// [`#[component]`](macro@crate::component) the slot is a parameter marked `#[slot]`.
///
/// ```
/// use griffin_web::html;
/// use griffin_web::template::{Component, Rendered, SlotEntries};
///
/// struct User {
///     name: &'static str,
///     age: u32,
/// }
///
/// /// The attributes of a column.
/// #[derive(Default)]
/// struct Col<'a> {
///     label: &'a str,
/// }
///
/// #[derive(Default)]
/// struct Table<'a> {
///     rows: &'a [User],
///     col: SlotEntries<'a, &'a User, Col<'a>>,
/// }
///
/// impl<'a> Table<'a> {
///     fn rows(mut self, rows: &'a [User]) -> Self {
///         self.rows = rows;
///         self
///     }
///
///     fn col(mut self, add: impl FnOnce(&mut SlotEntries<'a, &'a User, Col<'a>>)) -> Self {
///         add(&mut self.col);
///         self
///     }
/// }
///
/// impl Component for Table<'_> {
///     const REQUIRED: &'static [&'static str] = &["rows"];
///     const REQUIRED_SLOTS: &'static [&'static str] = &["col"];
///
///     fn render(self) -> Rendered {
///         let (rows, col) = (self.rows, &self.col);
///         html! {
///             <table>
///                 <tr><th :for={col in col}>{col.label}</th></tr>
///                 <tr :for={row in rows}><td :for={col in col}>{col.render(row)}</td></tr>
///             </table>
///         }
///     }
/// }
///
/// let users = [User { name: "Ann", age: 30 }];
/// let by_tag = html! {
///     <Table rows={&users}>
///         <:col label="Name" :let={user}>{user.name}</:col>
///         <:col label="Age" :let={user}>{user.age}</:col>
///     </Table>
/// };
/// // What the tag expands to, without its compile-time checks.
/// let by_hand = Table::default()
///     .rows(&users)
///     .col(|slot| {
///         let attributes = slot.push(|user| html! { {user.name} });
///         attributes.label = "Name";
///     })
///     .col(|slot| {
///         let attributes = slot.push(|user| html! { {user.age} });
///         attributes.label = "Age";
///     })
///     .render();
///
/// assert_eq!(by_tag.to_html(), by_hand.to_html());
/// assert_eq!(
///     by_hand.to_html(),
///     "<table>\n    \
///          <tr><th>Name</th><th>Age</th></tr>\n    \
///          <tr><td>Ann</td><td>30</td></tr>\n\
///      </table>"
/// );
/// ```
pub struct SlotEntries<'a, T = (), A = ()> {
    entries: Vec<SlotEntry<'a, T, A>>,
}

/// One entry of a slot: its content, and the attributes it was given.
/// See [`SlotEntries`].
pub struct SlotEntry<'a, T = (), A = ()> {
    attributes: A,
    // One allocation for each entry on every render. Make the Component
    // generic over the closure if rendering shows up in a profile.
    content: Box<dyn Fn(T) -> Rendered + 'a>,
}

/// The fingerprints of the two templates a slot with several entries renders as one.
/// Fixed, and no hash that `html!` makes of statics is expected to equal either.
const ENTRIES: u64 = 0x736c_6f74_0000_0001;
const ENTRY: u64 = 0x736c_6f74_0000_0002;

impl<'a, T, A> SlotEntries<'a, T, A> {
    /// Adds an entry with the given content, and returns its attributes, which start
    /// as their [`Default`], to assign to. This is what a tag calls for content
    /// between a Component's tags and for each `<:name>`; see [`Component`].
    pub fn push(&mut self, content: impl Fn(T) -> Rendered + 'a) -> &mut A
    where
        A: Default,
    {
        let (attributes, content) = (A::default(), Box::new(content));
        &mut self
            .entries
            .push_mut(SlotEntry {
                attributes,
                content,
            })
            .attributes
    }

    /// Every entry rendered with `argument`, one after the other, as Phoenix's
    /// `render_slot` does: nothing for an empty slot, the entry's template for one
    /// entry, and a comprehension of the entries' templates for several.
    pub fn render(&self, argument: T) -> Slot
    where
        T: Clone,
    {
        match &*self.entries {
            [] => Slot::text(""),
            [entry] => Slot::template(entry.render(argument)),
            entries => {
                let entries = entries
                    .iter()
                    .map(|entry| vec![Slot::template(entry.render(argument.clone()))]);
                let entries = Slot::comprehension(ENTRY, &["", ""], entries);
                Slot::template(Rendered::new(ENTRIES, &["", ""], vec![entries]))
            }
        }
    }
}

impl<T, A> Default for SlotEntries<'_, T, A> {
    fn default() -> Self {
        let entries = Vec::new();
        SlotEntries { entries }
    }
}

impl<'a, T, A> Deref for SlotEntries<'a, T, A> {
    type Target = [SlotEntry<'a, T, A>];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl<'s, 'a, T, A> IntoIterator for &'s SlotEntries<'a, T, A> {
    type Item = &'s SlotEntry<'a, T, A>;
    type IntoIter = std::slice::Iter<'s, SlotEntry<'a, T, A>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

/// What `{slot}` in a Component's template becomes, for a slot without an argument:
/// its entries, as by [`SlotEntries::render`].
impl<A> From<&SlotEntries<'_, (), A>> for Slot {
    fn from(entries: &SlotEntries<'_, (), A>) -> Slot {
        entries.render(())
    }
}

impl<T, A> SlotEntry<'_, T, A> {
    /// The entry's content, rendered with `argument`.
    pub fn render(&self, argument: T) -> Rendered {
        (self.content)(argument)
    }
}

/// The attributes the entry was given: `col.label`.
impl<T, A> Deref for SlotEntry<'_, T, A> {
    type Target = A;

    fn deref(&self) -> &A {
        &self.attributes
    }
}

/// What `{entry}` in a Component's template becomes, for a slot without an argument:
/// the entry's content, as a nested template.
impl<A> From<&SlotEntry<'_, (), A>> for Slot {
    fn from(entry: &SlotEntry<'_, (), A>) -> Slot {
        Slot::template(entry.render(()))
    }
}

/// The same for the reference a `:for` over a slot binds, so that `{entry}` works
/// in `<li :for={entry in &item}>{entry}</li>`.
impl<A> From<&&SlotEntry<'_, (), A>> for Slot {
    fn from(entry: &&SlotEntry<'_, (), A>) -> Slot {
        Slot::from(*entry)
    }
}

/// The compile-time check a Component tag expands to, once for its attributes and
/// once for its slots: panics, which in a constant is a compile error, unless every
/// name in `required` is among those `given`. `what` is the word for one of them in
/// the message, `"attribute"` or `"slot"`.
pub const fn require(what: &str, required: &[&str], given: &[&str]) {
    let mut at = 0;
    while at < required.len() {
        if !contains(given, required[at]) {
            // A panic in a constant takes a `&str` but cannot format one, so the
            // message is put together by hand.
            let parts = ["missing required ", what, " `", required[at], "`"];
            let (mut message, mut length, mut part) = ([0; 128], 0, 0);
            while part < parts.len() {
                let bytes = parts[part].as_bytes();
                let mut byte = 0;
                while byte < bytes.len() && length < message.len() {
                    message[length] = bytes[byte];
                    (length, byte) = (length + 1, byte + 1);
                }
                part += 1;
            }
            match core::str::from_utf8(message.split_at(length).0) {
                Ok(message) => panic!("{}", message),
                // A name too long for the message was cut inside a character.
                Err(_) => panic!("missing a required attribute or slot"),
            }
        }
        at += 1;
    }
}

/// `==` on `str` is not yet usable in a constant.
const fn contains(names: &[&str], name: &str) -> bool {
    let mut at = 0;
    while at < names.len() {
        let (a, b) = (names[at].as_bytes(), name.as_bytes());
        let (mut same, mut byte) = (a.len() == b.len(), 0);
        while same && byte < a.len() {
            same = a[byte] == b[byte];
            byte += 1;
        }
        if same {
            return true;
        }
        at += 1;
    }
    false
}
