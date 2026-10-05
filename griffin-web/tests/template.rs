//! The template model through its public API: HTML output, escaping, and the diffs
//! rebuilding that same HTML on the client. Agreement with Phoenix's JSON is in `conformance`.

use griffin_web::template::{AttributeValue, DiffState, Rendered, Slot};
use serde_json::{Value, json};

fn greeting(name: &str) -> Rendered {
    Rendered::new(1, &["<p>Hello, ", "!</p>"], vec![Slot::text(name)]).single_root()
}

fn forecast(day: &str, degrees: i32) -> Rendered {
    Rendered::new(
        2,
        &["<h1>", "</h1>", " degrees"],
        vec![Slot::text(day), Slot::text(degrees)],
    )
}

#[test]
fn html_is_the_statics_with_the_slots_between_them() {
    assert_eq!(
        forecast("Monday", 21).to_html(),
        "<h1>Monday</h1>21 degrees"
    );
}

#[test]
fn a_template_without_slots_is_its_one_static() {
    let rendered = Rendered::new(3, &["<p>hello</p>"], vec![]);

    assert_eq!(rendered.to_html(), "<p>hello</p>");
}

#[test]
fn text_cannot_inject_markup() {
    let rendered = greeting(r#"<script>alert("x")</script> & 'y'"#);

    assert_eq!(
        rendered.to_html(),
        "<p>Hello, &lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;y&#39;!</p>"
    );
}

#[test]
fn a_borrowed_value_converts_to_escaped_text_and_a_borrowed_slot_to_itself() {
    let (text, raw) = (String::from("<b>"), Slot::raw_html("<b>"));

    assert_eq!(Slot::from(&text), Slot::text("<b>"));
    assert_eq!(Slot::from("<b>"), Slot::text("<b>"));
    assert_eq!(Slot::from(&raw), raw);
}

#[test]
fn an_attribute_is_a_bare_name_for_true_and_nothing_for_false_or_none() {
    // By reference, as a template reads its state.
    let input = |checked: &bool, title: &Option<&str>, size: &u8| {
        Rendered::new(
            5,
            &["<input", "", "", ">"],
            vec![
                Slot::attribute("checked", checked),
                Slot::attribute("title", title),
                Slot::attribute("size", size),
            ],
        )
        .to_html()
    };

    assert_eq!(
        input(&true, &Some("tip"), &4),
        r#"<input checked title="tip" size="4">"#
    );
    assert_eq!(input(&false, &None, &4), r#"<input size="4">"#);
}

#[test]
fn an_attribute_value_cannot_break_out_of_its_quotes() {
    let slot = Slot::attribute("title", "\" onclick=\"alert(1)\" <&>'");

    assert_eq!(
        slot,
        Slot::raw_html(r#" title="&quot; onclick=&quot;alert(1)&quot; &lt;&amp;&gt;&#39;""#)
    );
}

#[test]
fn spread_attributes_are_written_in_order_with_names_and_values_escaped() {
    let slot = Slot::attributes([
        ("id", AttributeValue::Value("intro".to_owned())),
        ("hidden", AttributeValue::Bare),
        ("title", AttributeValue::Omitted),
        ("x\"><script>", AttributeValue::Value("<y>".to_owned())),
    ]);

    assert_eq!(
        slot,
        Slot::raw_html(r#" id="intro" hidden x&quot;&gt;&lt;script&gt;="&lt;y&gt;""#)
    );
    assert_eq!(
        Slot::attributes([("id", "a"), ("lang", "th")]),
        Slot::raw_html(r#" id="a" lang="th""#)
    );
}

#[test]
fn text_cannot_break_out_of_a_quoted_attribute() {
    let rendered = Rendered::new(
        4,
        &["<a title=\"", "\" class='", "'>x</a>"],
        vec![
            Slot::text("\" onclick=\"alert(1)"),
            Slot::text("' onclick='alert(1)"),
        ],
    );

    assert_eq!(
        rendered.to_html(),
        "<a title=\"&quot; onclick=&quot;alert(1)\" class='&#39; onclick=&#39;alert(1)'>x</a>"
    );
}

#[test]
fn raw_html_is_sent_as_written() {
    let rendered = Rendered::new(
        5,
        &["<p>", "</p>"],
        vec![Slot::raw_html("<b>bold</b> &amp;")],
    );

    assert_eq!(rendered.to_html(), "<p><b>bold</b> &amp;</p>");
}

#[test]
fn a_different_fingerprint_sends_the_new_statics_and_every_slot() {
    let mut state = DiffState::new();
    state.render(greeting("Ann"));

    let diff = state.render(forecast("Ann", 21));

    assert_eq!(
        diff,
        json!({"0": "Ann", "1": "21", "s": 0, "p": {"0": ["<h1>", "</h1>", " degrees"]}})
    );
}

#[test]
fn a_nested_template_is_its_html_in_place() {
    let badge = Rendered::new(7, &["<b>", "</b>"], vec![Slot::text(3)]);
    let inbox = |badge| Rendered::new(8, &["<h1>Inbox", "</h1>"], vec![badge]);

    assert_eq!(Slot::from(&badge), Slot::template(badge.clone()));
    assert_eq!(
        inbox(Slot::template(badge)).to_html(),
        "<h1>Inbox<b>3</b></h1>"
    );
    assert_eq!(inbox(Slot::text("")).to_html(), "<h1>Inbox</h1>");
}

#[test]
fn a_comprehension_is_its_entries_one_after_the_other() {
    let rows = |names: &[&str]| {
        let entries = names.iter().map(|name| vec![Slot::text(name)]);
        Rendered::new(
            8,
            &["<ul>", "</ul>"],
            vec![Slot::comprehension(9, &["<li>", "</li>"], entries)],
        )
        .to_html()
    };

    assert_eq!(
        rows(&["<Ann>", "Bob"]),
        "<ul><li>&lt;Ann&gt;</li><li>Bob</li></ul>"
    );
    assert_eq!(rows(&[]), "<ul></ul>");
}

#[test]
#[should_panic(expected = "two entries of a comprehension have the key \"7\"")]
fn two_entries_with_one_key_are_refused() {
    Slot::keyed_comprehension(9, &["<li></li>"], [(7, vec![]), (8, vec![]), (7, vec![])]);
}

#[test]
#[should_panic(expected = "one static before each slot and one after the last")]
fn an_entry_whose_slots_do_not_fit_the_statics_is_refused() {
    Slot::comprehension(9, &["<li>", "</li>"], [vec![]]);
}

fn page(title: &str, body: Slot) -> Rendered {
    Rendered::new(10, &["<h1>", "</h1>", ""], vec![Slot::text(title), body])
}

fn note(text: &str, more: Slot) -> Slot {
    Slot::template(Rendered::new(
        11,
        &["<p>", "", "</p>"],
        vec![Slot::text(text), more],
    ))
}

fn badge(count: u32) -> Slot {
    Slot::template(Rendered::new(12, &["<b>", "</b>"], vec![Slot::text(count)]).single_root())
}

fn names(names: &[&str]) -> Slot {
    let entries = names.iter().map(|name| vec![Slot::text(name)]);
    Slot::comprehension(13, &["<li>", "</li>"], entries)
}

/// Each member by id: a name, tags, and a badge when there is a count.
fn members(members: &[(u32, &str, &[&str], u32)]) -> Slot {
    let entries = members.iter().map(|(id, name, tags, count)| {
        let badge = if *count > 0 {
            badge(*count)
        } else {
            Slot::text("")
        };
        (id, vec![Slot::text(name), names(tags), badge])
    });
    Slot::keyed_comprehension(14, &["<div>", "<ul>", "</ul>", "</div>"], entries)
}

#[test]
fn the_client_rebuilds_the_html_from_the_diffs() {
    let nothing = || Slot::text("");
    let renders = [
        greeting("Ann"),
        greeting("Ann"),
        greeting("<Bob>"),
        forecast("Monday", 21),
        forecast("Monday", 19),
        forecast("Tuesday", 19),
        Rendered::new(3, &["<p>hello</p>"], vec![]),
        greeting("Bob"),
        // A nested template turning on, changing inside, and nesting deeper,
        page("Notes", nothing()),
        page("Notes", note("one", nothing())),
        page("Notes", note("two", nothing())),
        page("Notes", note("two", note("three", badge(1)))),
        page("Notes", note("two", note("three", badge(2)))),
        page("More notes", note("two", note("three", badge(2)))),
        page("More notes", note("two", note("three", nothing()))),
        // another template taking its slot, and the slot turning off.
        page("More notes", badge(3)),
        page("More notes", badge(3)),
        page("More notes", note("four", badge(3))),
        page("More notes", nothing()),
        // A list that starts empty, grows, changes in place, shrinks, empties and refills,
        page("Team", names(&[])),
        page("Team", names(&["Ann"])),
        page("Team", names(&["Ann", "Bob", "Cy"])),
        page("Team", names(&["Ann", "Bo", "Cy"])),
        page("Team", names(&["Bo", "Cy"])),
        page("Team", names(&[])),
        page("Team", names(&[])),
        page("Team", names(&["Di", "Ed"])),
        // a keyed one taking its slot, with a list and a template in each entry:
        // inserts, moves with and without changes inside, deletes, and emptying,
        page(
            "Team",
            members(&[(1, "Ann", &["a"], 0), (2, "Bob", &[], 2)]),
        ),
        page(
            "Team",
            members(&[
                (1, "Ann", &["a"], 0),
                (3, "Cy", &["c"], 1),
                (2, "Bob", &[], 2),
            ]),
        ),
        page(
            "Team",
            members(&[
                (2, "Bob", &[], 2),
                (3, "Cy", &["c"], 1),
                (1, "Ann", &["a"], 0),
            ]),
        ),
        page(
            "Team",
            members(&[
                (3, "Cy", &["c", "d"], 0),
                (2, "Bobby", &["b"], 3),
                (1, "Ann", &[], 4),
            ]),
        ),
        page(
            "Team",
            members(&[(1, "Ann", &[], 4), (3, "Cy", &["c", "d"], 0)]),
        ),
        page(
            "Team",
            members(&[(4, "Di", &["e"], 5), (1, "Ann", &["a"], 4)]),
        ),
        page("Team", members(&[(1, "Ann", &["a"], 4)])),
        page("Team", members(&[])),
        page("Team", members(&[(5, "Ed", &["f"], 6)])),
        // and a list inside a nested template that is replaced.
        page("Team", note("five", names(&["x", "y"]))),
        page("Team", note("five", names(&["y"]))),
        page("Team", names(&["y"])),
    ];
    let mut state = DiffState::new();
    let mut client = Value::Null;

    for rendered in renders {
        let html = rendered.to_html();
        merge(&mut client, state.render(rendered));
        assert_eq!(client_html(&mut client), html);
    }
}

/// `mergeDiff` of the pinned client (rendered.js), for text, nested templates and
/// comprehensions: a diff carrying statics replaces what is at its place in the tree,
/// any other is merged into it, a comprehension entry by entry and the rest key by key.
fn merge(tree: &mut Value, diff: Value) {
    let Value::Object(mut diff) = diff else {
        unreachable!("a diff is an object")
    };
    if diff.contains_key("s") {
        *tree = diff.into();
        return;
    }
    if let Some(Value::Object(keyed)) = diff.remove("k") {
        return merge_keyed(&mut tree["k"], keyed);
    }
    for (key, value) in diff {
        if value.is_object() && value.get("s").is_none() && tree[&key].is_object() {
            merge(&mut tree[&key], value);
        } else {
            tree[&key] = value;
        }
    }
}

/// `mergeKeyed` of the pinned client: an entry is a diff to merge into the entry at
/// its index, the index to copy the entry from as it was before this merge, or that
/// index and a diff to merge into the copy. Entries past the new count are dropped.
fn merge_keyed(entries: &mut Value, keyed: serde_json::Map<String, Value>) {
    let before = entries.clone();
    for (index, entry) in keyed {
        match entry {
            _ if index == "km" => {}
            count if index == "kc" => {
                for dropped in count.as_u64().unwrap()..before["kc"].as_u64().unwrap() {
                    entries
                        .as_object_mut()
                        .unwrap()
                        .remove(&dropped.to_string());
                }
                entries["kc"] = count;
            }
            Value::Number(from) => entries[&index] = before[from.to_string()].clone(),
            Value::Array(moved) => {
                entries[&index] = before[moved[0].to_string()].clone();
                merge(&mut entries[&index], moved[1].clone());
            }
            diff => {
                if entries[&index].is_null() {
                    entries[&index] = json!({});
                }
                merge(&mut entries[&index], diff);
            }
        }
    }
}

/// `toString` of the pinned client: the shared statics (`"p"`) are taken out of the
/// tree, and each template is its statics with the slot at each index between them. A
/// comprehension is that once for each of its entries, all with its statics.
fn client_html(tree: &mut Value) -> String {
    let templates = tree.as_object_mut().unwrap().remove("p");
    let mut html = String::new();
    write_html(tree, &templates.unwrap_or_default(), &mut html);
    html
}

fn write_html(node: &mut Value, templates: &Value, html: &mut String) {
    // A position is resolved for good: later diffs number their statics afresh.
    if let Some(position) = node["s"].as_u64() {
        node["s"] = templates[position.to_string()].clone();
    }
    let statics = node["s"].as_array().unwrap().clone();
    match node["k"]["kc"].as_u64() {
        Some(count) => (0..count).for_each(|index| {
            write_slots(&mut node["k"][index.to_string()], &statics, templates, html);
        }),
        None => write_slots(node, &statics, templates, html),
    }
}

fn write_slots(node: &mut Value, statics: &[Value], templates: &Value, html: &mut String) {
    for (index, part) in statics.iter().enumerate() {
        if index > 0 {
            match &mut node[(index - 1).to_string()] {
                Value::String(text) => html.push_str(text),
                nested => write_html(nested, templates, html),
            }
        }
        html.push_str(part.as_str().unwrap());
    }
}

#[test]
#[should_panic(expected = "one static before each slot and one after the last")]
fn statics_and_slots_that_do_not_interleave_are_refused() {
    Rendered::new(6, &["<p>", "</p>"], vec![]);
}
