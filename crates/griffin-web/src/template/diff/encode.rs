//! A [`Change`] as the diff JSON of the pinned Phoenix LiveView client (the wire protocol is Phoenix's, unchanged).

use serde_json::{Map, Value, json};

use super::{Change, ListChange, SlotChange};

/// The statics sent with one diff, each once: Phoenix puts them in `"p"` at the top
/// and points at them by position. A template or comprehension is numbered after the
/// ones nested in it, and those with equal fingerprints share a position.
type Templates = Vec<(u64, &'static [&'static str])>;

pub(super) fn diff(change: &Change) -> Value {
    let mut templates = Templates::new();
    let mut diff = node(change, &mut templates);
    if !templates.is_empty() {
        let templates = templates.iter().enumerate();
        let templates: Map<_, _> = templates
            .map(|(position, (_, statics))| (position.to_string(), Value::from(*statics)))
            .collect();
        diff.insert("p".to_owned(), templates.into());
    }
    Value::Object(diff)
}

fn node(change: &Change, templates: &mut Templates) -> Map<String, Value> {
    let mut diff = slots(&change.slots, templates);
    if change.new_template {
        let position = position(change.fingerprint, change.statics, templates);
        diff.insert("s".to_owned(), position.into());
        if change.root {
            diff.insert("r".to_owned(), 1.into());
        }
    }
    diff
}

fn slots(slots: &[(usize, SlotChange)], templates: &mut Templates) -> Map<String, Value> {
    let slots = slots.iter().map(|(index, slot)| {
        let slot = match slot {
            SlotChange::Html(html) => Value::from(*html),
            SlotChange::Template(nested) => node(nested, templates).into(),
            SlotChange::Comprehension(list) => comprehension(list, templates).into(),
        };
        (index.to_string(), slot)
    });
    slots.collect()
}

/// `"k"` holds the entries by index and `"kc"`, how many there are now. An entry is
/// the slots to merge into the one the client has at that index; or, when the client
/// has it at another index, that index, alone or paired with the slots, and `"km"`
/// says that there are such entries.
fn comprehension(list: &ListChange, templates: &mut Templates) -> Map<String, Value> {
    let mut keyed = Map::new();
    for (index, moved_from, entry) in &list.entries {
        // The statics of an entry are the comprehension's, so only its slots are sent.
        let slots = Value::from(slots(&entry.slots, templates));
        let entry = match moved_from {
            None => slots,
            Some(from) if entry.slots.is_empty() => Value::from(*from),
            Some(from) => json!([from, slots]),
        };
        keyed.insert(index.to_string(), entry);
        if moved_from.is_some() {
            keyed.insert("km".to_owned(), true.into());
        }
    }
    keyed.insert("kc".to_owned(), list.count.into());
    let mut diff = Map::new();
    diff.insert("k".to_owned(), keyed.into());
    if list.new_template {
        let position = position(list.fingerprint, list.statics, templates);
        diff.insert("s".to_owned(), position.into());
    }
    diff
}

fn position(
    fingerprint: u64,
    statics: &'static [&'static str],
    templates: &mut Templates,
) -> usize {
    let known = |(known, _): &_| *known == fingerprint;
    templates.iter().position(known).unwrap_or_else(|| {
        templates.push((fingerprint, statics));
        templates.len() - 1
    })
}
