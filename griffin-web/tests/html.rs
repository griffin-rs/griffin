//! The `html!` and `html_file!` macros through what they render. Agreement with Phoenix's JSON is in
//! `conformance`, compiler messages are in `ui`, and the expansion itself is
//! snapshot-tested in `griffin-macros`.

use std::convert::Infallible;
use std::fmt;
use std::fs;
use std::num::ParseIntError;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, PoisonError};

use griffin_web::live::{Event, EventError, Js, Payload};
use griffin_web::template::{Attributes, Component, DiffState, Rendered, Slot, SlotEntries};
use griffin_web::{component, html, html_file};
use serde_json::json;

#[test]
fn text_keeps_the_spacing_it_was_written_with() {
    let name = "Ann";

    let rendered = html! { <p>Hello, {name}!   Nice   day (is it not?)</p> };

    assert_eq!(
        rendered.to_html(),
        "<p>Hello, Ann!   Nice   day (is it not?)</p>"
    );
}

#[test]
fn lines_are_indented_relative_to_the_first_one() {
    let rendered = html! {
        <ul>
            <li>one</li>

            <li>two</li>
        </ul>
    };

    assert_eq!(
        rendered.to_html(),
        "<ul>\n    <li>one</li>\n\n    <li>two</li>\n</ul>"
    );
}

#[test]
fn an_expression_is_any_rust_expression() {
    const BULK: usize = 3;
    struct Cart {
        prices: Vec<u32>,
    }
    impl Cart {
        fn render(&self) -> Rendered {
            // `@` before a pattern stays Rust's own.
            html! { <p>{@prices.len()} items, {@prices.iter().sum::<u32>() / 100} dollars, {
                match @prices.len() { many @ BULK.. => many - 2, _ => 0 }
            } free</p> }
        }
    }

    let cart = Cart {
        prices: vec![250, 250, 500],
    };

    assert_eq!(
        cart.render().to_html(),
        "<p>3 items, 10 dollars, 1 free</p>"
    );
}

#[test]
fn an_assign_is_read_without_moving_it_out_of_the_state() {
    struct Profile {
        name: String,
    }
    impl Profile {
        fn render(&self) -> Rendered {
            html! { <h1>{@name}</h1> }
        }
    }

    let profile = Profile {
        name: "Ann".to_owned(),
    };

    assert_eq!(profile.render().to_html(), "<h1>Ann</h1>");
}

#[test]
fn an_expression_cannot_inject_markup() {
    let comment = r#"<script>alert("x")</script> & 'y'"#;

    let rendered = html! { <p>{comment}</p> };

    assert_eq!(
        rendered.to_html(),
        "<p>&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;y&#39;</p>"
    );
}

#[test]
fn raw_html_is_the_one_way_around_escaping() {
    let trusted = "<b>bold</b>";

    let rendered = html! { <p>{Slot::raw_html(trusted)} and {trusted}</p> };

    assert_eq!(
        rendered.to_html(),
        "<p><b>bold</b> and &lt;b&gt;bold&lt;/b&gt;</p>"
    );
}

#[test]
fn an_attribute_value_cannot_break_out_of_its_quotes() {
    let evil = r#""><script>alert('x')</script>"#;

    let rendered = html! { <a title={evil} class={evil}>x</a> };

    assert_eq!(
        rendered.to_html(),
        "<a title=\"&quot;&gt;&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;\" \
         class=\"&quot;&gt;&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;\">x</a>"
    );
}

#[test]
fn attributes_are_static_dynamic_boolean_optional_or_spread() {
    struct Field {
        name: String,
        required: bool,
        hint: Option<String>,
        rest: Vec<(&'static str, &'static str)>,
    }
    impl Field {
        fn render(&self) -> Rendered {
            html! {
                <input type="text" name={@name} required={@required} title={@hint} {@rest.iter().copied()} />
            }
        }
    }

    let mut field = Field {
        name: "email".to_owned(),
        required: true,
        hint: Some("Work address".to_owned()),
        rest: vec![("id", "email"), ("data-kind", "contact")],
    };
    assert_eq!(
        field.render().to_html(),
        r#"<input type="text" name="email" required title="Work address" id="email" data-kind="contact">"#
    );

    (field.required, field.hint, field.rest) = (false, None, vec![]);
    assert_eq!(
        field.render().to_html(),
        r#"<input type="text" name="email">"#
    );
}

#[test]
fn an_element_with_if_is_only_rendered_when_the_condition_holds() {
    struct Inbox {
        unread: u32,
        muted: bool,
    }
    impl Inbox {
        fn render(&self) -> Rendered {
            html! { <h1>Inbox<b :if={@unread > 0} class="badge">{@unread}</b><hr :if={!@muted}></h1> }
        }
    }

    let html = |unread, muted| Inbox { unread, muted }.render().to_html();

    assert_eq!(html(3, true), r#"<h1>Inbox<b class="badge">3</b></h1>"#);
    assert_eq!(html(0, false), "<h1>Inbox<hr></h1>");
}

#[test]
fn an_element_with_for_is_rendered_once_for_each_item() {
    struct Guests {
        guests: Vec<(u32, String)>,
        tables: Vec<Vec<&'static str>>,
    }
    impl Guests {
        fn render(&self) -> Rendered {
            // A name of the user's own is not hidden by anything the macro declares.
            let entries = "entries";
            html! {
                <ul><li :for={(id, name) in &@guests} :key={id}>{id}: {name}, {entries}</li></ul>
                <ol><li :for={table in &@tables}><b :for={seat in table}>{seat}</b></li></ol>
            }
        }
    }

    let mut guests = Guests {
        guests: vec![(7, "Ann".to_owned()), (9, "<Bob>".to_owned())],
        tables: vec![vec!["a", "b"], vec![], vec!["c"]],
    };
    assert_eq!(
        guests.render().to_html(),
        "<ul><li>7: Ann, entries</li><li>9: &lt;Bob&gt;, entries</li></ul>\n\
         <ol><li><b>a</b><b>b</b></li><li></li><li><b>c</b></li></ol>"
    );

    (guests.guests, guests.tables) = (vec![], vec![]);
    assert_eq!(guests.render().to_html(), "<ul></ul>\n<ol></ol>");
}

#[test]
fn an_if_beside_a_for_leaves_items_out() {
    let numbers = [1, 2, 3, 4, 5, 6];
    let odd_only = true;

    let rendered = html! {
        <p><i :if={!odd_only || n % 2 == 1} :for={n in numbers}>{n}</i></p>
    };

    assert_eq!(rendered.to_html(), "<p><i>1</i><i>3</i><i>5</i></p>");
}

#[test]
fn a_keyed_list_sends_moves_instead_of_content() {
    let list = |guests: &[(u32, &str)]| {
        html! { <ul><li :for={(id, name) in guests} :key={id}>{name}</li></ul> }
    };
    let mut state = DiffState::new();
    state.render(list(&[(1, "Ann"), (2, "Bob"), (3, "Cy")]));

    assert_eq!(
        state.render(list(&[(3, "Cy"), (1, "Anna"), (2, "Bob")])),
        json!({"0": {"k": {"0": 2, "1": [0, {"0": "Anna"}], "2": 1, "kc": 3, "km": true}}})
    );
}

#[test]
fn an_if_without_else_renders_nothing_when_no_branch_is_taken() {
    let badge = |unread: u32| {
        html! {
            <h1>Inbox{if unread > 99 { html! { <b>99+</b> } } else if unread > 0 { html! { <b>{unread}</b> } }}</h1>
        }
        .to_html()
    };

    assert_eq!(badge(120), "<h1>Inbox<b>99+</b></h1>");
    assert_eq!(badge(3), "<h1>Inbox<b>3</b></h1>");
    assert_eq!(badge(0), "<h1>Inbox</h1>");
}

#[test]
fn if_and_match_branch_into_nested_templates() {
    enum Session {
        Guest,
        Member { name: String, admin: bool },
    }
    struct Header {
        session: Session,
    }
    impl Header {
        fn render(&self) -> Rendered {
            html! {
                <header>{match &@session {
                    Session::Guest => html! { <a href="/login">Sign in</a> },
                    Session::Member { name, admin } => html! {
                        <span>{name}{if *admin { html! { <i>admin</i> } } else { html! { <i>member</i> } }}</span>
                    },
                }}</header>
            }
        }
    }
    let member = |name: &str, admin| Session::Member {
        name: name.to_owned(),
        admin,
    };
    let mut header = Header {
        session: Session::Guest,
    };
    let mut state = DiffState::new();

    assert_eq!(
        header.render().to_html(),
        r#"<header><a href="/login">Sign in</a></header>"#
    );
    state.render(header.render());

    // Another branch is another template, so its statics are sent,
    header.session = member("Ann", false);
    assert_eq!(
        header.render().to_html(),
        "<header><span>Ann<i>member</i></span></header>"
    );
    assert_eq!(
        state.render(header.render()),
        json!({
            "0": {"0": "Ann", "1": {"s": 0, "r": 1}, "s": 1, "r": 1},
            "p": {"0": ["<i>member</i>"], "1": ["<span>", "", "</span>"]},
        })
    );
    // and staying in a branch sends only the slots that changed in it.
    header.session = member("Bob", false);
    assert_eq!(state.render(header.render()), json!({"0": {"0": "Bob"}}));
    header.session = member("Bob", true);
    assert_eq!(
        state.render(header.render()),
        json!({"0": {"1": {"s": 0, "r": 1}}, "p": {"0": ["<i>admin</i>"]}})
    );
}

/// A Component written by hand, with no macro: what a capitalised tag expands to is
/// `Default`, one method for each attribute given, and `Component::render`.
#[derive(Default)]
struct Badge<'a> {
    label: &'a str,
    count: u32,
}

impl<'a> Badge<'a> {
    fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }
}

impl Component for Badge<'_> {
    const REQUIRED: &'static [&'static str] = &["label"];

    fn render(self) -> Rendered {
        html! { <span class="badge">{self.label} {self.count}</span> }
    }
}

#[test]
fn a_capitalised_tag_calls_a_component() {
    let unread = 3;

    let rendered = html! { <p>Inbox <Badge label="new" count={unread} /></p> };

    assert_eq!(
        rendered.to_html(),
        r#"<p>Inbox <span class="badge">new 3</span></p>"#
    );
}

#[component]
fn Avatar(name: &str, size: u32) -> Rendered {
    html! { <img class="avatar" alt={name} width={size}> }
}

#[test]
fn a_component_is_a_function_with_typed_parameters() {
    let user = String::from("Ann");

    let rendered = html! { <a href="/me"><Avatar name={&user} size={32} /></a> };

    assert_eq!(
        rendered.to_html(),
        r#"<a href="/me"><img class="avatar" alt="Ann" width="32"></a>"#
    );
}

#[test]
fn an_if_on_a_component_tag_guards_the_call() {
    let render = |user: Option<&str>| {
        // The attribute could not be evaluated for a user who is not there.
        html! { <p><Avatar :if={user.is_some()} name={user.unwrap()} size={16} /></p> }
    };

    assert_eq!(render(None).to_html(), "<p></p>");
    assert_eq!(
        render(Some("Ann")).to_html(),
        r#"<p><img class="avatar" alt="Ann" width="16"></p>"#
    );
}

#[test]
fn a_for_on_a_component_tag_calls_it_once_for_each_item() {
    let users = [("Ann", 16), ("Bob", 32), ("Cy", 0)];

    let rendered = html! {
        <p><Avatar :for={(name, size) in users} :if={size > 0} :key={name} name={name} size={size} /></p>
    };

    assert_eq!(
        rendered.to_html(),
        r#"<p><img class="avatar" alt="Ann" width="16"><img class="avatar" alt="Bob" width="32"></p>"#
    );
}

#[component]
fn Field(r#type: &str, #[default] required: bool) -> Rendered {
    html! { <input type={r#type} required={required}> }
}

#[component]
fn Rule() -> Rendered {
    html! { <hr> }
}

#[test]
fn an_attribute_may_be_named_like_a_rust_keyword_and_a_component_may_have_none() {
    let rendered = html! { <form><Field type="email" required /><Rule /></form> };

    assert_eq!(
        rendered.to_html(),
        r#"<form><input type="email" required><hr></form>"#
    );
}

#[component]
fn Tag(label: &str, #[default(1)] count: u32, #[default] muted: bool) -> Rendered {
    html! { <span class="tag" data-muted={muted}>{label} x{count}</span> }
}

#[test]
fn an_optional_attribute_falls_back_to_its_declared_default() {
    let rendered = html! { <p><Tag label="a" /><Tag label="b" count={5} muted /></p> };

    assert_eq!(
        rendered.to_html(),
        r#"<p><span class="tag">a x1</span><span class="tag" data-muted>b x5</span></p>"#
    );
}

#[component]
fn Button(label: &str, #[global] rest: Attributes) -> Rendered {
    html! { <button {rest}>{label}</button> }
}

#[test]
fn a_component_that_opts_in_forwards_extra_html_attributes_to_its_root() {
    let (busy, id) = (true, 7);

    let rendered = html! {
        <Button label="Save" class="primary" hidden={busy} title={None::<&str>} data-id={id} aria-busy />
    };

    assert_eq!(
        rendered.to_html(),
        r#"<button class="primary" hidden data-id="7" aria-busy>Save</button>"#
    );
}

/// A Component with a default slot, written by hand: content between the tags is a
/// call of `inner_block` with a closure, which is handed the slot to push an entry on.
#[derive(Default)]
struct Card<'a> {
    title: &'a str,
    inner_block: SlotEntries<'a>,
}

impl<'a> Card<'a> {
    fn title(mut self, title: &'a str) -> Self {
        self.title = title;
        self
    }

    fn inner_block(mut self, add: impl FnOnce(&mut SlotEntries<'a>)) -> Self {
        add(&mut self.inner_block);
        self
    }
}

impl Component for Card<'_> {
    const REQUIRED: &'static [&'static str] = &["title"];
    const REQUIRED_SLOTS: &'static [&'static str] = &["inner_block"];

    fn render(self) -> Rendered {
        html! { <section><h2>{self.title}</h2>{self.inner_block}</section> }
    }
}

#[test]
fn content_between_the_tags_of_a_component_is_its_default_slot() {
    let name = "Ann";

    let rendered = html! { <Card title="Guest">Hello, <b>{name}</b>!</Card> };

    assert_eq!(
        rendered.to_html(),
        "<section><h2>Guest</h2>Hello, <b>Ann</b>!</section>"
    );
}

#[component]
fn Layout(title: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <main><h1>{title}</h1>{inner_block}</main> }
}

#[test]
fn a_slot_is_a_parameter_of_the_function_and_a_layout_is_a_component_with_one() {
    struct Page {
        user: String,
    }
    impl Page {
        fn render(&self) -> Rendered {
            html! { <Layout title="Home"><p>Welcome, {@user}.</p><Avatar name={&@user} size={16} /></Layout> }
        }
    }
    let page = Page {
        user: "Ann".to_owned(),
    };

    assert_eq!(
        page.render().to_html(),
        r#"<main><h1>Home</h1><p>Welcome, Ann.</p><img class="avatar" alt="Ann" width="16"></main>"#
    );
}

#[component]
fn Panel(
    #[slot]
    #[default]
    header: SlotEntries<'_>,
    #[slot] inner_block: SlotEntries<'_>,
    #[slot]
    #[default]
    item: SlotEntries<'_>,
    #[slot]
    #[default]
    footer: SlotEntries<'_>,
) -> Rendered {
    html! {
        <div><header>{header}</header>{inner_block}<ul><li :for={item in &item}>{item}</li></ul><footer>{footer}</footer></div>
    }
}

#[test]
fn a_named_slot_renders_where_the_component_places_it_and_one_given_twice_is_iterated() {
    let rendered = html! {
        <Panel>
            <:item>one</:item>
            <:header>News</:header>
            Body
            <:item><b>two</b></:item>
        </Panel>
    };

    assert_eq!(
        rendered.to_html(),
        // Text after a slot entry loses its leading whitespace, as in Phoenix.
        "<div><header>News</header>\n    Body\n    <ul><li>one</li><li><b>two</b></li></ul><footer></footer></div>"
    );
}

struct Guest {
    name: &'static str,
    age: u32,
}

#[component]
fn Roster(guests: &[Guest], #[slot] inner_block: SlotEntries<'_, &Guest>) -> Rendered {
    html! { <ul><li :for={guest in guests}>{inner_block.render(guest)}</li></ul> }
}

#[test]
fn a_component_passes_an_argument_to_a_slot_which_the_tag_binds_with_let() {
    let guests = [
        Guest {
            name: "Ann",
            age: 30,
        },
        Guest {
            name: "Bob",
            age: 40,
        },
    ];
    let unit = "years";

    let rendered = html! {
        <Roster guests={&guests} :let={Guest { name, age }}>{name}, {age} {unit}</Roster>
    };

    assert_eq!(
        rendered.to_html(),
        "<ul><li>Ann, 30 years</li><li>Bob, 40 years</li></ul>"
    );
}

/// The attributes of an entry of `Table`'s slot `col`: a field for each.
#[derive(Default)]
struct Column<'a> {
    label: &'a str,
    numeric: bool,
}

#[component]
fn Table(rows: &[Guest], #[slot] col: SlotEntries<'_, &Guest, Column<'_>>) -> Rendered {
    html! {
        <table>
            <tr><th :for={col in &col} data-numeric={col.numeric}>{col.label}</th></tr>
            <tr :for={row in rows}><td :for={col in &col}>{col.render(row)}</td></tr>
        </table>
    }
}

#[test]
fn a_slot_entry_carries_attributes_of_its_own_which_the_component_reads() {
    let guests = [Guest {
        name: "Ann",
        age: 30,
    }];
    let age = String::from("Age");

    let rendered = html! {
        <Table rows={&guests}>
            <:col label="Name" :let={guest}>{guest.name}</:col>
            <:col label={&age} numeric :let={Guest { age, .. }}>{age}</:col>
        </Table>
    };

    assert_eq!(
        rendered.to_html(),
        "<table>\n    \
             <tr><th>Name</th><th data-numeric>Age</th></tr>\n    \
             <tr><td>Ann</td><td>30</td></tr>\n\
         </table>"
    );
}

#[test]
fn a_comment_and_the_content_of_style_and_script_are_sent_as_written() {
    let rendered = html! {
        <!-- {not} an <expression> -->
        <style>p > b { margin: 0 }</style>
        <script>if (1 < 2) { run() }</script>
    };

    assert_eq!(
        rendered.to_html(),
        "<!-- {not} an <expression> -->\n\
         <style>p > b { margin: 0 }</style>\n\
         <script>if (1 < 2) { run() }</script>"
    );
}

struct Inbox {
    title: &'static str,
    unread: u32,
    muted: bool,
    guests: Vec<(u32, &'static str)>,
}

impl Inbox {
    fn file(&self) -> Rendered {
        html_file!("inbox.html.griffin")
    }

    /// What `templates/inbox.html.griffin` holds.
    fn inline(&self) -> Rendered {
        html! {
            <section class="inbox" data-muted={@muted}>
                <h1>{@title}</h1>
                <p :if={@unread > 0}>{@unread} unread</p>
                <ul>
                    <li :for={(id, name) in &@guests} :key={id}>{name}</li>
                </ul>
                <Layout title="Guests">{@guests.len()} invited</Layout>
            </section>
        }
    }
}

#[test]
fn a_template_file_renders_and_diffs_as_the_same_template_written_inline() {
    let steps = [
        Inbox {
            title: "Inbox",
            unread: 0,
            muted: false,
            guests: vec![(1, "Ann"), (2, "Bob")],
        },
        Inbox {
            title: "Inbox",
            unread: 2,
            muted: true,
            guests: vec![(2, "Bob"), (1, "Ann"), (3, "Cy")],
        },
        Inbox {
            title: "Archive",
            unread: 0,
            muted: true,
            guests: vec![],
        },
    ];
    let (mut file, mut inline) = (DiffState::new(), DiffState::new());

    assert_eq!(
        steps[0].file().to_html(),
        "<section class=\"inbox\">\n    \
             <h1>Inbox</h1>\n    \n    \
             <ul>\n        <li>Ann</li><li>Bob</li>\n    </ul>\n    \
             <main><h1>Guests</h1>2 invited</main>\n\
         </section>"
    );
    for step in &steps {
        assert_eq!(step.file().to_html(), step.inline().to_html());
        assert_eq!(file.render(step.file()), inline.render(step.inline()));
    }
    // They are one template to a client too: after either, the other resends no statics.
    assert_eq!(inline.render(steps[2].file()), json!({}));
}

#[test]
fn a_template_file_is_html_text_and_not_rust_tokens() {
    struct Document {
        title: &'static str,
    }
    impl Document {
        fn render(&self) -> Rendered {
            html_file!("document.html.griffin")
        }
    }

    let document = Document { title: "Ann" };

    // Apostrophes and quotes in text, a doctype, a comment, CSS and JavaScript are
    // sent as written, braces and all. A value in single quotes keeps them only if
    // it needs them, and the blank lines around the template are not part of it.
    assert_eq!(
        document.render().to_html(),
        r#"<!DOCTYPE html>
<!-- Don't remove {this}: it's the <head> of every page -->
<html lang="en">
<head>
    <title>Ann</title>
    <style>
        body > p { margin: 0; }
    </style>
    <script>
        if (1 < 2) { console.log("it's </p> {@title}"); }
    </script>
</head>
<body data-greeting='say "hi"' x-data="{ open: false }">
    <p>It's Ann's page &amp; that's "fine".</p>
</body>
</html>"#
    );
}

#[test]
fn an_expression_in_a_template_file_ends_where_rust_says_it_does() {
    // Without `@`, a template file reads the names in scope where it is used.
    let (open, label) = (true, Some("on"));

    let rendered = html_file!("expressions.html.griffin");

    assert_eq!(rendered.to_html(), r#"<p title="}}">{on</p>"#);
}

#[component]
fn Sheet(title: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html_file!("sheet.html.griffin")
}

#[test]
fn a_template_file_is_the_template_of_a_component() {
    let rendered = html! { <Sheet title="Guest">Hello, {"Ann"}!</Sheet> };

    assert_eq!(
        rendered.to_html(),
        "<article class=\"sheet\">\n    <h2>Guest</h2>\n    Hello, Ann!\n</article>"
    );
}

#[test]
fn a_template_site_keeps_its_fingerprint_and_other_statics_get_another() {
    fn greeting(name: &str) -> Rendered {
        html! { <p>Hello, {name}!</p> }
    }
    fn farewell(name: &str) -> Rendered {
        html! { <p>Goodbye, {name}!</p> }
    }
    let mut state = DiffState::new();
    state.render(greeting("Ann"));

    // The client has these statics, so they are not sent again,
    assert_eq!(state.render(greeting("Bob")), json!({"0": "Bob"}));
    // but one changed static part makes it a template the client has not seen.
    assert_eq!(
        state.render(farewell("Bob")),
        json!({"0": "Bob", "s": 0, "p": {"0": ["<p>Goodbye, ", "!</p>"]}, "r": 1})
    );
}

#[test]
fn the_fingerprint_is_the_same_in_every_compiler_run() {
    let name = "Ann";

    let rendered = html! { <p>Hello, {name}!</p> };

    // FNV-1a over the statics `<p>Hello, ` and `!</p>`, each followed by a 0xff
    // byte, worked out apart from the macro. A fingerprint is otherwise private, and
    // is read off the `Debug` output only to pin it here.
    let debug = format!("{rendered:?}");
    assert!(
        debug.contains("fingerprint: 10701045657905709241,"),
        "{debug}"
    );
}

/// A newtype with a parser: what an Event's field needs is `FromStr` and `Display`.
#[derive(Debug, PartialEq)]
struct UserId(u32);

impl FromStr for UserId {
    type Err = ParseIntError;

    fn from_str(text: &str) -> Result<UserId, ParseIntError> {
        text.strip_prefix("user-").unwrap_or("").parse().map(UserId)
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "user-{}", self.0)
    }
}

#[derive(Event, Debug, PartialEq)]
enum TodoEvent {
    Rename {
        id: u32,
        title: String,
        done: bool,
        ratio: f64,
        note: Option<String>,
        owner: UserId,
    },
    ClearDone,
}

#[test]
fn an_event_bound_to_an_element_is_its_name_and_a_value_attribute_for_each_field() {
    let rename = |note| TodoEvent::Rename {
        id: 7,
        title: "Buy <milk> & \"eggs\"".to_owned(),
        done: true,
        ratio: 0.5,
        note,
        owner: UserId(3),
    };
    let (plain, noted) = (rename(None), rename(Some("soon".to_owned())));

    let rendered = html! {
        <ul>
            <li phx-click={plain}>Plain</li>
            <li phx-click={noted} class="noted">Noted</li>
            <li phx-keydown={TodoEvent::ClearDone} phx-key="Escape">Clear</li>
        </ul>
    };

    // The name is the variant's in snake case. A field that is `None` is left out.
    assert_eq!(
        rendered.to_html(),
        "<ul>\n    \
            <li phx-click=\"rename\" phx-value-id=\"7\" \
                phx-value-title=\"Buy &lt;milk&gt; &amp; &quot;eggs&quot;\" \
                phx-value-done=\"true\" phx-value-ratio=\"0.5\" \
                phx-value-owner=\"user-3\">Plain</li>\n    \
            <li phx-click=\"rename\" phx-value-id=\"7\" \
                phx-value-title=\"Buy &lt;milk&gt; &amp; &quot;eggs&quot;\" \
                phx-value-done=\"true\" phx-value-ratio=\"0.5\" phx-value-note=\"soon\" \
                phx-value-owner=\"user-3\" class=\"noted\">Noted</li>\n    \
            <li phx-keydown=\"clear_done\" phx-key=\"Escape\">Clear</li>\n\
         </ul>"
    );
}

#[test]
fn what_the_client_sends_for_a_bound_event_decodes_to_that_event() {
    // The client reads each `phx-value-*` attribute as text, and adds the element's
    // own `value` when it has one.
    let sent = json!({
        "id": "7", "title": "Buy <milk>", "done": "true", "ratio": "0.5",
        "owner": "user-3", "value": "",
    });

    assert_eq!(
        TodoEvent::decode("rename", &Payload::from(&sent)),
        Ok(TodoEvent::Rename {
            id: 7,
            title: "Buy <milk>".to_owned(),
            done: true,
            ratio: 0.5,
            note: None,
            owner: UserId(3),
        })
    );
}

#[test]
fn an_event_field_that_is_missing_or_does_not_parse_is_an_error_naming_the_field() {
    let rename = |change: fn(&mut serde_json::Value)| {
        let mut sent = json!({
            "id": "7", "title": "Milk", "done": "false", "ratio": "1", "note": "soon",
            "owner": "user-3",
        });
        change(&mut sent);
        TodoEvent::decode("rename", &Payload::from(&sent))
    };

    assert!(rename(|_| ()).is_ok());
    // What a hook pushes need not be text: a number or a boolean is read as its text.
    assert!(rename(|sent| sent["id"] = json!(7)).is_ok());
    assert!(rename(|sent| sent["done"] = json!(true)).is_ok());
    for (field, change) in [
        (
            "id",
            (|sent| sent["id"] = json!("-7")) as fn(&mut serde_json::Value),
        ),
        ("id", |sent| sent["id"] = json!("")),
        ("id", |sent| sent["id"] = json!([7])),
        ("done", |sent| sent["done"] = json!("yes")),
        ("ratio", |sent| sent["ratio"] = json!("half")),
        ("owner", |sent| sent["owner"] = json!("3")),
        ("owner", |sent| sent["owner"] = json!(null)),
        ("title", |sent| {
            drop(sent.as_object_mut().unwrap().remove("title"))
        }),
        // An optional field may be missing, but not be something else than its type.
        ("note", |sent| sent["note"] = json!({"text": "soon"})),
    ] {
        assert_eq!(rename(change), Err(EventError::Field(field)));
    }
}

#[test]
fn an_event_bound_by_its_name_as_plain_text_is_the_same_event() {
    // As in Phoenix, and as a hook pushes one: on the wire it is the name either way.
    let by_name = html! { <button phx-click="clear_done">Clear</button> };
    let by_value = html! { <button phx-click={TodoEvent::ClearDone}>Clear</button> };

    assert_eq!(by_name.to_html(), by_value.to_html());
    assert_eq!(
        TodoEvent::decode("clear_done", &Payload::from(&json!({"value": ""}))),
        Ok(TodoEvent::ClearDone)
    );
    // A name that is no variant's is no Event, whatever its case.
    for name in ["ClearDone", "clear", "", "rename "] {
        let decoded = TodoEvent::decode(name, &Payload::from(&json!({})));
        assert_eq!(decoded, Err(EventError::Unknown), "{name:?}");
    }
}

#[test]
fn a_component_forwards_an_event_bound_on_its_tag() {
    let rendered = html! { <Button label="Clear" phx-click={&TodoEvent::ClearDone} /> };

    assert_eq!(
        rendered.to_html(),
        r#"<button phx-click="clear_done">Clear</button>"#
    );
}

#[test]
fn text_in_commands_bound_to_an_element_cannot_leave_the_attribute() {
    // As a user might have named a todo.
    let hostile = "\"><script>alert('x')</script>&";
    let commands = Js::new()
        .set_attribute("title", hostile, hostile)
        .add_class(hostile, None)
        .push(&TodoEvent::Rename {
            id: 7,
            title: hostile.to_owned(),
            done: false,
            ratio: 1.0,
            note: None,
            owner: UserId(3),
        });

    let html = html! { <button phx-click={commands}>Go</button> }.to_html();

    let value = html.strip_prefix("<button phx-click=\"").expect(&html);
    let value = value.strip_suffix("\">Go</button>").expect(&html);
    // Nothing in the value ends the attribute or opens a tag,
    assert!(!value.contains(['"', '\'', '<', '>']), "{value}");
    // and the browser, which unescapes an attribute's value, reads the text as it was.
    let unescaped = value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&unescaped).unwrap(),
        json!([
            ["set_attr", {"attr": ["title", hostile], "to": hostile}],
            ["add_class", {"names": [hostile]}],
            ["push", {"event": "rename", "value": {
                "id": "7", "title": hostile, "done": "false", "ratio": "1", "owner": "user-3",
            }}],
        ])
    );
}

#[test]
fn a_component_forwards_commands_bound_on_its_tag() {
    let rendered = html! { <Button label="Menu" phx-click={&Js::new().toggle("#menu")} /> };

    assert_eq!(
        rendered.to_html(),
        "<button phx-click=\"[[&quot;toggle&quot;,{&quot;to&quot;:&quot;#menu&quot;}]]\">Menu</button>"
    );
}

#[test]
fn a_live_view_without_events_has_none_to_decode() {
    let decoded = Infallible::decode("anything", &Payload::from(&json!({})));

    assert_eq!(decoded, Err(EventError::Unknown));
}

/// Runs trybuild with the template files of the `ui` cases where their `html_file!`
/// looks for them: trybuild compiles the cases as a crate of its own in the target
/// directory, and a template file is looked up in the `templates` directory of the
/// crate that uses it. `cases` is given that directory.
// Knows where trybuild 1.0 puts that crate. If it moves, the `file_*` cases
// fail with a missing template file.
fn trybuild(cases: impl FnOnce(&Path)) {
    // One run at a time, so that none compiles a file another is still writing.
    static RUNNING: Mutex<()> = Mutex::new(());
    let _running = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);

    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).parent().unwrap();
    let templates = target.join("tests/trybuild/griffin-web/templates");
    fs::create_dir_all(&templates).unwrap();
    for file in fs::read_dir("tests/ui/templates").unwrap() {
        let file = file.unwrap();
        fs::copy(file.path(), templates.join(file.file_name())).unwrap();
    }
    cases(&templates);
}

#[test]
fn syntax_and_type_errors_are_reported_at_the_template() {
    trybuild(|_| trybuild::TestCases::new().compile_fail("tests/ui/*.rs"));
}

#[test]
fn editing_a_template_file_recompiles_the_code_that_uses_it() {
    trybuild(|templates| {
        // The case fails unless what it renders is what the file holds when it runs.
        // Its Rust source is the same both times, so only cargo noticing the edit
        // gets it past the second.
        for text in ["<p>before</p>", "<p>after the edit</p>"] {
            fs::write(templates.join("edited.html.griffin"), text).unwrap();
            trybuild::TestCases::new().pass("tests/ui/pass/edited.rs");
        }
    });
}
