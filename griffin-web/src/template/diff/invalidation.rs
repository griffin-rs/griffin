//! Which slots to send again: every slot is rendered, and the ones whose output differs
//! from the previous render are sent (diffs compare rendered slots). Compile-time change tracking replaces
//! this module.

use std::collections::HashMap;

use super::{Change, ListChange, SlotChange};
use crate::template::{Comprehension, Content, Rendered, Slot};

/// What takes the client from `previous`, the render it has where `next` goes, if any,
/// to `next`: everything when the fingerprints differ, otherwise the slots that differ.
///
/// `next` is what the client has afterwards. As in Phoenix, an empty comprehension is
/// sent as empty text unless the client has that comprehension there already, and it
/// is made that text in `next`, so that the next render knows the statics were not sent.
pub(super) fn change<'a>(previous: Option<&Rendered>, next: &'a mut Rendered) -> Change<'a> {
    let previous = previous.filter(|previous| previous.fingerprint == next.fingerprint);
    let slots = next
        .slots
        .iter_mut()
        .enumerate()
        .filter_map(|(index, slot)| {
            let before = previous.and_then(|previous| previous.slots.get(index));
            // The comprehension the client has here, when it is the one in the slot.
            let had = match (before, &slot.0) {
                (Some(Slot(Content::Comprehension(before))), Content::Comprehension(list))
                    if before.fingerprint == list.fingerprint =>
                {
                    Some(before)
                }
                _ => None,
            };
            if matches!(&slot.0, Content::Comprehension(list) if list.entries.is_empty())
                && had.is_none()
            {
                *slot = Slot::text("");
            }
            if matches!(slot.0, Content::Html(_)) && before == Some(slot) {
                return None;
            }
            let slot = match &mut slot.0 {
                Content::Html(html) => SlotChange::Html(html),
                Content::Template(template) => {
                    let before = match before {
                        Some(Slot(Content::Template(before))) => Some(before),
                        _ => None,
                    };
                    let change = change(before, template);
                    if !change.new_template && change.slots.is_empty() {
                        return None;
                    }
                    SlotChange::Template(change)
                }
                Content::Comprehension(list) => {
                    let change = list_change(had, list);
                    let same_length = had.map(|had| had.entries.len()) == Some(change.count);
                    if same_length && change.entries.is_empty() {
                        return None;
                    }
                    SlotChange::Comprehension(change)
                }
            };
            Some((index, slot))
        });
    Change {
        fingerprint: next.fingerprint,
        statics: next.statics,
        root: next.root,
        new_template: previous.is_none(),
        slots: slots.collect(),
    }
}

/// What takes the client from `previous`, the comprehension it has where `next` goes,
/// if any, to `next`: an entry whose key was in `previous` is that entry, moved or
/// not, and only its slots that differ are sent; any other entry is sent whole.
fn list_change<'a>(
    previous: Option<&Comprehension>,
    next: &'a mut Comprehension,
) -> ListChange<'a> {
    let previous_entries = previous.map_or(&[][..], |previous| &previous.entries);
    let previous_entries: HashMap<&str, (usize, &Rendered)> = previous_entries
        .iter()
        .enumerate()
        .map(|(index, (key, entry))| (key.as_str(), (index, entry)))
        .collect();
    let count = next.entries.len();
    let entries = next
        .entries
        .iter_mut()
        .enumerate()
        .filter_map(|(index, (key, entry))| {
            let before = previous_entries.get(key.as_str());
            let change = change(before.map(|(_, entry)| *entry), entry);
            let moved_from = before.map(|(from, _)| *from).filter(|from| *from != index);
            let unchanged = before.is_some() && moved_from.is_none() && change.slots.is_empty();
            (!unchanged).then_some((index, moved_from, change))
        });
    ListChange {
        fingerprint: next.fingerprint,
        statics: next.statics,
        new_template: previous.is_none(),
        count,
        entries: entries.collect(),
    }
}
