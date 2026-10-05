//! Conformance with Phoenix LiveView's diff JSON (the wire protocol is Phoenix's, unchanged). See README.md in this directory.

use std::collections::BTreeSet;

use griffin_web::live::{Event, Js};
use griffin_web::template::{
    AttributeValue, Attributes, Component, DiffState, Rendered, Slot, SlotEntries,
};
use griffin_web::{component, html};
use serde_json::{Value, json};

#[test]
fn single_slot() {
    let diffs = diffs("single_slot", |assigns| {
        Rendered::new(
            1,
            &["<p>", "</p>"],
            vec![Slot::text(text(assigns, "value"))],
        )
        .single_root()
    });

    assert_matches_phoenix("single_slot", &diffs);
}

#[test]
fn single_slot_written_with_the_macro() {
    struct View<'a> {
        value: &'a str,
    }
    impl View<'_> {
        fn render(&self) -> Rendered {
            html! { <p>{@value}</p> }
        }
    }

    let diffs = diffs("single_slot", |assigns| {
        View {
            value: text(assigns, "value"),
        }
        .render()
    });

    assert_matches_phoenix("single_slot", &diffs);
}

#[test]
fn static_only() {
    let diffs = diffs("static_only", |_assigns| {
        Rendered::new(2, &["<p>hello</p>"], vec![]).single_root()
    });

    assert_matches_phoenix("static_only", &diffs);
}

#[test]
fn multiple_slots() {
    let diffs = diffs("multiple_slots", |assigns| {
        Rendered::new(
            3,
            &["<h1>", "</h1><p>", "</p>"],
            vec![
                Slot::text(text(assigns, "title")),
                Slot::text(text(assigns, "body")),
            ],
        )
    });

    assert_matches_phoenix("multiple_slots", &diffs);
}

#[test]
fn escaped_text() {
    let diffs = diffs("escaped_text", |assigns| {
        Rendered::new(
            4,
            &["<p>", "</p>"],
            vec![Slot::text(text(assigns, "value"))],
        )
        .single_root()
    });

    assert_matches_phoenix("escaped_text", &diffs);
}

#[test]
fn raw_html() {
    let diffs = diffs("raw_html", |assigns| {
        Rendered::new(
            5,
            &["<p>", "</p>"],
            vec![Slot::raw_html(text(assigns, "value"))],
        )
        .single_root()
    });

    assert_matches_phoenix("raw_html", &diffs);
}

#[test]
fn void_and_self_closing() {
    let diffs = diffs("void_and_self_closing", |_assigns| {
        html! {
            <div class="box"   id="main">
              <br>
              <input type="text" disabled />
              <span />
              <hr/>
            </div>
        }
    });

    assert_matches_phoenix("void_and_self_closing", &diffs);
}

#[test]
fn dynamic_attribute() {
    let diffs = diffs("dynamic_attribute", |assigns| {
        let href = text(assigns, "href");
        html! { <a href={href}>link</a> }
    });

    assert_matches_phoenix("dynamic_attribute", &diffs);
}

#[test]
fn boolean_attribute() {
    let diffs = diffs("boolean_attribute", |assigns| {
        let checked = assigns["checked"].as_bool().unwrap();
        html! { <input type="checkbox" checked={checked}> }
    });

    assert_matches_phoenix("boolean_attribute", &diffs);
}

#[test]
fn optional_attribute() {
    let diffs = diffs("optional_attribute", |assigns| {
        let title: Option<&str> = assigns["title"].as_str();
        html! { <p title={title}>text</p> }
    });

    assert_matches_phoenix("optional_attribute", &diffs);
}

#[test]
fn class_attribute() {
    let diffs = diffs("class_attribute", |assigns| {
        let (class, style) = (text(assigns, "class"), text(assigns, "style"));
        html! { <p class={class} style={style}>text</p> }
    });

    assert_matches_phoenix("class_attribute", &diffs);
}

#[test]
fn attribute_spread() {
    let diffs = diffs("attribute_spread", |assigns| {
        let rest = assigns["rest"].as_object().unwrap().iter();
        let rest = rest.map(|(name, value)| match value {
            Value::String(value) => (name, AttributeValue::Value(value.clone())),
            Value::Bool(true) => (name, AttributeValue::Bare),
            _ => (name, AttributeValue::Omitted),
        });
        html! { <p {rest}>text</p> }
    });

    assert_matches_phoenix("attribute_spread", &diffs);
}

#[test]
fn conditional() {
    let diffs = diffs("conditional", |assigns| {
        let greeting = if flag(assigns, "show") {
            Slot::template(Rendered::new(
                7,
                &["<p>Hello, ", "!</p>"],
                vec![Slot::text(text(assigns, "name"))],
            ))
        } else {
            Slot::text("")
        };
        Rendered::new(6, &["<div>", "</div>"], vec![greeting]).single_root()
    });

    assert_matches_phoenix("conditional", &diffs);
}

#[test]
fn conditional_written_with_the_macro() {
    let diffs = diffs("conditional", |assigns| {
        let (show, name) = (flag(assigns, "show"), text(assigns, "name"));
        html! { <div><p :if={show}>Hello, {name}!</p></div> }
    });

    assert_matches_phoenix("conditional", &diffs);
}

#[test]
fn conditional_root() {
    let diffs = diffs("conditional_root", |assigns| {
        let (show, name) = (flag(assigns, "show"), text(assigns, "name"));
        html! { <p :if={show}>Hello, {name}!</p> }
    });

    assert_matches_phoenix("conditional_root", &diffs);
}

#[test]
fn nested_three_levels() {
    let diffs = diffs("nested_three_levels", |assigns| {
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| text(assigns, name));
        let [one, two, three] = ["one", "two", "three"].map(|name| flag(assigns, name));
        html! {
            <div>{a}<section :if={one}>{b}<article :if={two}>{c}<p :if={three}>{d}</p></article></section></div>
        }
    });

    assert_matches_phoenix("nested_three_levels", &diffs);
}

#[test]
fn twin_conditionals() {
    let diffs = diffs("twin_conditionals", |assigns| {
        let (first, second) = (flag(assigns, "first"), flag(assigns, "second"));
        html! { <div><p :if={first}>same</p><p :if={second}>same</p></div> }
    });

    assert_matches_phoenix("twin_conditionals", &diffs);
}

#[test]
fn branch_switch() {
    let diffs = diffs("branch_switch", |assigns| {
        let (admin, name) = (flag(assigns, "admin"), text(assigns, "name"));
        html! {
            <div>{if admin { html! { <b>Admin {name}</b> } } else { html! { <i>Guest {name}</i> } }}</div>
        }
    });

    assert_matches_phoenix("branch_switch", &diffs);
}

#[test]
fn branch_switch_written_with_match() {
    let diffs = diffs("branch_switch", |assigns| {
        let (admin, name) = (flag(assigns, "admin"), text(assigns, "name"));
        html! {
            <div>{match admin {
                true => html! { <b>Admin {name}</b> },
                false => html! { <i>Guest {name}</i> },
            }}</div>
        }
    });

    assert_matches_phoenix("branch_switch", &diffs);
}

#[test]
fn if_without_else() {
    let diffs = diffs("if_without_else", |assigns| {
        let (show, name) = (flag(assigns, "show"), text(assigns, "name"));
        html! { <div>{if show { html! { <p>Hello, {name}!</p> } }}</div> }
    });

    assert_matches_phoenix("if_without_else", &diffs);
}

#[test]
fn for_unkeyed() {
    let diffs = diffs("for_unkeyed", |assigns| {
        let items = list(assigns, "items").iter();
        let items = items.map(|item| vec![Slot::text(item.as_str().unwrap())]);
        let items = Slot::comprehension(8, &["<li>", "</li>"], items);
        Rendered::new(9, &["<ul>", "</ul>"], vec![items]).single_root()
    });

    assert_matches_phoenix("for_unkeyed", &diffs);
}

#[test]
fn for_unkeyed_written_with_the_macro() {
    let diffs = diffs("for_unkeyed", |assigns| {
        let items = list(assigns, "items");
        html! { <ul><li :for={item in items}>{item.as_str().unwrap()}</li></ul> }
    });

    assert_matches_phoenix("for_unkeyed", &diffs);
}

#[test]
fn for_keyed() {
    let diffs = diffs("for_keyed", |assigns| {
        let items = list(assigns, "items").iter().map(|item| {
            let name = Slot::text(item["name"].as_str().unwrap());
            (item["id"].as_u64().unwrap(), vec![name])
        });
        let items = Slot::keyed_comprehension(10, &["<li>", "</li>"], items);
        Rendered::new(
            11,
            &["<h1>", "</h1><ul>", "</ul>"],
            vec![Slot::text(text(assigns, "title")), items],
        )
    });

    assert_matches_phoenix("for_keyed", &diffs);
}

#[test]
fn for_keyed_written_with_the_macro() {
    let diffs = diffs("for_keyed", |assigns| {
        let (title, items) = (text(assigns, "title"), list(assigns, "items"));
        html! {
            <h1>{title}</h1><ul><li :for={item in items} :key={item["id"]}>{item["name"].as_str().unwrap()}</li></ul>
        }
    });

    assert_matches_phoenix("for_keyed", &diffs);
}

#[test]
fn for_empty() {
    let diffs = diffs("for_empty", |assigns| {
        let items = list(assigns, "items");
        html! { <ul><li :for={item in items}>{item.as_str().unwrap()}</li></ul> }
    });

    assert_matches_phoenix("for_empty", &diffs);
}

#[test]
fn for_with_if() {
    let diffs = diffs("for_with_if", |assigns| {
        let items = list(assigns, "items");
        html! {
            <ul><li :for={item in items} :if={item["show"] == true}>{item["name"].as_str().unwrap()}</li></ul>
        }
    });

    assert_matches_phoenix("for_with_if", &diffs);
}

#[test]
fn for_nested() {
    let diffs = diffs("for_nested", |assigns| {
        let groups = list(assigns, "groups");
        html! {
            <section :for={group in groups}><h2>{group["name"].as_str().unwrap()}</h2><p :for={item in group["items"].as_array().unwrap()}>{item.as_str().unwrap()}</p></section>
        }
    });

    assert_matches_phoenix("for_nested", &diffs);
}

#[test]
fn for_keyed_nested_if() {
    let diffs = diffs("for_keyed_nested_if", |assigns| {
        let items = list(assigns, "items").iter();
        let items = items.map(|item| (&item["id"], item["name"].as_str().unwrap(), &item["bold"]));
        html! {
            <ul><li :for={(id, name, bold) in items} :key={id}>{name}<b :if={*bold == true}>{name}</b></li></ul>
        }
    });

    assert_matches_phoenix("for_keyed_nested_if", &diffs);
}

#[test]
fn for_same_statics() {
    let diffs = diffs("for_same_statics", |assigns| {
        let (show, items) = (flag(assigns, "show"), list(assigns, "items"));
        html! {
            <ul><li :if={show}>x</li><li :for={_ in items}>x</li><li :for={_ in items}>x</li></ul>
        }
    });

    assert_matches_phoenix("for_same_statics", &diffs);
}

/// The `badge` function component of the `component` cases, written by hand against
/// the API a tag expands to.
#[derive(Default)]
struct HandBadge<'a> {
    label: &'a str,
    count: u64,
}

impl<'a> HandBadge<'a> {
    fn label(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    fn count(mut self, count: u64) -> Self {
        self.count = count;
        self
    }
}

impl Component for HandBadge<'_> {
    const REQUIRED: &'static [&'static str] = &["label"];

    fn render(self) -> Rendered {
        Rendered::new(
            20,
            &["<span class=\"badge\">", " ", "</span>"],
            vec![Slot::text(self.label), Slot::text(self.count)],
        )
        .single_root()
    }
}

#[test]
fn component() {
    let diffs = diffs("component", |assigns| {
        let (label, count) = (text(assigns, "label"), assigns["count"].as_u64().unwrap());
        html! { <p>Inbox <HandBadge label={label} count={count} /> Drafts <HandBadge label={label} /></p> }
    });

    assert_matches_phoenix("component", &diffs);
}

/// The same `badge`, as a function.
#[component]
fn Badge(label: &str, #[default(0)] count: u64) -> Rendered {
    html! { <span class="badge">{label} {count}</span> }
}

#[test]
fn component_written_with_the_macro() {
    let diffs = diffs("component", |assigns| {
        let (label, count) = (text(assigns, "label"), assigns["count"].as_u64().unwrap());
        html! { <p>Inbox <Badge label={label} count={count} /> Drafts <Badge label={label} /></p> }
    });

    assert_matches_phoenix("component", &diffs);
}

#[test]
fn component_root() {
    let diffs = diffs("component_root", |assigns| {
        let label = text(assigns, "label");
        html! { <Badge label={label} /> }
    });

    assert_matches_phoenix("component_root", &diffs);
}

#[test]
fn component_if() {
    let diffs = diffs("component_if", |assigns| {
        let (show, label) = (flag(assigns, "show"), text(assigns, "label"));
        html! { <p><Badge :if={show} label={label} /></p> }
    });

    assert_matches_phoenix("component_if", &diffs);
}

#[test]
fn component_for() {
    let diffs = diffs("component_for", |assigns| {
        let labels = list(assigns, "labels");
        html! { <p><Badge :for={label in labels} label={label.as_str().unwrap()} /></p> }
    });

    assert_matches_phoenix("component_for", &diffs);
}

#[test]
fn component_for_with_if() {
    let diffs = diffs("component_for_with_if", |assigns| {
        let (labels, hidden) = (list(assigns, "labels"), text(assigns, "hidden"));
        html! { <p><Badge :for={label in labels} :if={label != hidden} label={label.as_str().unwrap()} /></p> }
    });

    assert_matches_phoenix("component_for_with_if", &diffs);
}

#[component]
fn Button(label: &str, #[global] rest: Attributes) -> Rendered {
    html! { <button {rest}>{label}</button> }
}

#[test]
fn component_global() {
    let diffs = diffs("component_global", |assigns| {
        let (class, busy) = (text(assigns, "class"), flag(assigns, "busy"));
        // Phoenix keeps the extra attributes in a map, and writes them in the order of
        // its keys, which here puts `hidden` first. Griffin writes them as given.
        html! { <Button label="Save" hidden={busy} class={class} data-id="7" /> }
    });

    assert_matches_phoenix("component_global", &diffs);
}

/// The `card` function component of the `slot_default` case: content between the tags
/// is its default slot.
#[component]
fn Card(title: &str, #[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! { <section><h2>{title}</h2>{inner_block}</section> }
}

#[test]
fn slot_default() {
    let diffs = diffs("slot_default", |assigns| {
        let (title, name) = (text(assigns, "title"), text(assigns, "name"));
        html! { <Card title={title}>Hello, <b>{name}</b>!</Card> }
    });

    assert_matches_phoenix("slot_default", &diffs);
}

/// The same `card`, written by hand against the API a tag expands to.
#[derive(Default)]
struct HandCard<'a> {
    title: &'a str,
    inner_block: SlotEntries<'a>,
}

impl<'a> HandCard<'a> {
    fn title(mut self, title: &'a str) -> Self {
        self.title = title;
        self
    }

    fn inner_block(mut self, add: impl FnOnce(&mut SlotEntries<'a>)) -> Self {
        add(&mut self.inner_block);
        self
    }
}

impl Component for HandCard<'_> {
    const REQUIRED: &'static [&'static str] = &["title"];
    const REQUIRED_SLOTS: &'static [&'static str] = &["inner_block"];

    fn render(self) -> Rendered {
        Rendered::new(
            31,
            &["<section><h2>", "</h2>", "</section>"],
            vec![Slot::text(self.title), self.inner_block.render(())],
        )
        .single_root()
    }
}

#[test]
fn slot_default_written_by_hand() {
    let diffs = diffs("slot_default", |assigns| {
        let (title, name) = (text(assigns, "title"), text(assigns, "name"));
        // What the tag in `slot_default` expands to, without its compile-time checks.
        let card = HandCard::default().title(title).inner_block(|slot| {
            slot.push(|()| Rendered::new(32, &["Hello, <b>", "</b>!"], vec![Slot::text(name)]));
        });
        Rendered::new(30, &["", ""], vec![Slot::template(card.render())])
    });

    assert_matches_phoenix("slot_default", &diffs);
}

/// The `panel` of the `slot_named` case: named slots, each rendered where the
/// Component places it. `item` is given twice and rendered as one, `footer` is not
/// given, and the default slot is passed an argument.
#[component]
fn Panel(
    kind: &str,
    #[slot]
    #[default]
    header: SlotEntries<'_>,
    #[slot] inner_block: SlotEntries<'_, &str>,
    #[slot]
    #[default]
    footer: SlotEntries<'_>,
    #[slot]
    #[default]
    item: SlotEntries<'_>,
) -> Rendered {
    html! {
        <div><header>{header}</header>{inner_block.render(kind)}<ul>{item}</ul><footer>{footer}</footer></div>
    }
}

#[test]
fn slot_named() {
    let diffs = diffs("slot_named", |assigns| {
        let (name, first) = (text(assigns, "name"), text(assigns, "first"));
        html! {
            <Panel :let={kind} kind="news"><:header>For {name}</:header><:item><li>{first}</li></:item>Some {kind}<:item><li>second</li></:item></Panel>
        }
    });

    assert_matches_phoenix("slot_named", &diffs);
}

/// The attributes of an entry of the slot `col` of `Table`.
#[derive(Default)]
struct Col<'a> {
    label: &'a str,
}

/// The `table` of the `slot_argument` case: the Component iterates the entries of a
/// slot given twice, reads the attribute of each, and passes each a row.
#[component]
fn Table(rows: &[Value], #[slot] col: SlotEntries<'_, &Value, Col<'_>>) -> Rendered {
    html! {
        <table><tr><th :for={col in &col}>{col.label}</th></tr><tr :for={row in rows}><td :for={col in &col}>{col.render(row)}</td></tr></table>
    }
}

#[test]
fn slot_argument() {
    let diffs = diffs("slot_argument", |assigns| {
        let (rows, unit) = (list(assigns, "rows"), text(assigns, "unit"));
        html! {
            <Table rows={rows}><:col :let={row} label="Name"><b>{row["name"].as_str().unwrap()}</b></:col><:col :let={row} label={unit}>{row["age"]} {unit}</:col></Table>
        }
    });

    assert_matches_phoenix("slot_argument", &diffs);
}

/// The Events the commands of the `commands` case push.
#[derive(Event)]
enum TodoEvent {
    Rename { id: u32, title: String },
    ClearDone,
}

/// Each client command is the operation list `Phoenix.LiveView.JS` makes of it, which
/// is what the client executes.
#[test]
fn commands() {
    let rename = TodoEvent::Rename {
        id: 7,
        title: "Milk".to_owned(),
    };
    let commands = [
        ("show", Js::new().show("#menu")),
        ("hide", Js::new().hide("#menu")),
        ("toggle", Js::new().toggle("#menu")),
        ("toggle_itself", Js::new().toggle(None)),
        ("add_class", Js::new().add_class("open  active", "#menu")),
        ("remove_class", Js::new().remove_class("open", "#menu")),
        (
            "set_attribute",
            Js::new().set_attribute("aria-expanded", "true", "#menu"),
        ),
        (
            "remove_attribute",
            Js::new().remove_attribute("aria-expanded", "#menu"),
        ),
        (
            "transition",
            Js::new().transition("fade-in duration-300", "#menu"),
        ),
        ("dispatch", Js::new().dispatch("menu:opened", "#menu")),
        ("focus", Js::new().focus("#search")),
        ("focus_itself", Js::new().focus(None)),
        ("push", Js::new().push(&TodoEvent::ClearDone)),
        ("push_with_fields", Js::new().push(&rename)),
        // Chaining appends.
        (
            "chain",
            Js::new()
                .toggle("#menu")
                .add_class("open", "#button")
                .push(&rename),
        ),
    ];

    // What a command writes into an attribute, as the client parses it.
    let operations = commands.iter().map(|(name, command)| {
        let AttributeValue::Value(json) = <AttributeValue as From<&Js>>::from(command) else {
            panic!("{name} is not an attribute with a value");
        };
        ((*name).to_owned(), serde_json::from_str(&json).unwrap())
    });
    assert_same_as_phoenix("commands", &Value::Object(operations.collect()));
}

/// A command bound in a template is the attribute Phoenix writes for it, escaped alike.
#[test]
fn command_attribute() {
    let diffs = diffs("command_attribute", |assigns| {
        let toggle = Js::new().toggle(text(assigns, "to"));
        html! { <button phx-click={toggle}>Menu</button> }
    });

    assert_matches_phoenix("command_attribute", &diffs);
}

#[test]
fn mismatch_names_the_differing_key_path() {
    let phoenix = json!({"0": "first", "p": {"0": ["<p>", "</p>"]}, "s": 0});
    let griffin = json!({"0": "first", "p": {"0": ["<p>", "</div>"]}, "s": 0});

    assert_eq!(
        mismatches(&phoenix, &griffin),
        [r#"$.p.0[1]: Phoenix has "</p>", Griffin has "</div>""#]
    );
    assert!(mismatches(&phoenix, &phoenix).is_empty());
}

#[test]
fn mismatch_names_keys_only_one_side_has() {
    let phoenix = json!([{"0": "second"}]);
    let griffin = json!([{"s": ["<p>", "</p>"]}]);

    assert_eq!(
        mismatches(&phoenix, &griffin),
        [
            r#"$[0].0: Phoenix has "second", Griffin has nothing"#,
            r#"$[0].s: Phoenix has nothing, Griffin has ["<p>","</p>"]"#,
        ]
    );
}

/// Griffin's diff after each step of the case. As in generate.exs, each step sets its
/// assigns on top of the earlier ones and one diff state sees every render. `render` is
/// the case's template, built by hand with the template API.
fn diffs(name: &str, render: impl Fn(&Assigns) -> Rendered) -> Vec<Value> {
    let case = read("cases", name);
    let mut assigns = Assigns::new();
    let mut state = DiffState::new();
    case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            assigns.extend(step.as_object().unwrap().clone());
            state.render(render(&assigns))
        })
        .collect()
}

type Assigns = serde_json::Map<String, Value>;

fn text<'a>(assigns: &'a Assigns, name: &str) -> &'a str {
    assigns[name].as_str().unwrap()
}

fn flag(assigns: &Assigns, name: &str) -> bool {
    assigns[name].as_bool().unwrap()
}

fn list<'a>(assigns: &'a Assigns, name: &str) -> &'a [Value] {
    assigns[name].as_array().unwrap()
}

/// Panics unless `griffin`, one diff per step of the case, is the JSON Phoenix emitted,
/// listing every differing key path.
fn assert_matches_phoenix(name: &str, griffin: &[Value]) {
    assert_same_as_phoenix(name, &Value::from(griffin));
}

/// Panics unless `griffin` is the fixture `name`, listing every differing key path.
fn assert_same_as_phoenix(name: &str, griffin: &Value) {
    let found = mismatches(&read("fixtures", name), griffin);
    assert!(
        found.is_empty(),
        "{name} does not match Phoenix:\n  {}",
        found.join("\n  ")
    );
}

/// `cases/<name>.json` is the case as generate.exs reads it (`template`, `steps`);
/// `fixtures/<name>.json` is the diff Phoenix emitted after each step.
fn read(dir: &str, name: &str) -> Value {
    let path = format!(
        "{}/tests/conformance/{dir}/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Every place where Griffin's JSON differs from Phoenix's, one `path: difference` line each.
fn mismatches(phoenix: &Value, griffin: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk("$".to_owned(), Some(phoenix), Some(griffin), &mut found);
    found
}

fn walk(path: String, phoenix: Option<&Value>, griffin: Option<&Value>, found: &mut Vec<String>) {
    match (phoenix, griffin) {
        (Some(Value::Object(p)), Some(Value::Object(g))) => {
            let keys: BTreeSet<&String> = p.keys().chain(g.keys()).collect();
            for key in keys {
                walk(format!("{path}.{key}"), p.get(key), g.get(key), found);
            }
        }
        (Some(Value::Array(p)), Some(Value::Array(g))) if p.len() == g.len() => {
            for (i, (p, g)) in p.iter().zip(g).enumerate() {
                walk(format!("{path}[{i}]"), Some(p), Some(g), found);
            }
        }
        _ if phoenix != griffin => {
            let show = |side: Option<&Value>| side.map_or("nothing".to_owned(), Value::to_string);
            found.push(format!(
                "{path}: Phoenix has {}, Griffin has {}",
                show(phoenix),
                show(griffin)
            ));
        }
        _ => {}
    }
}
