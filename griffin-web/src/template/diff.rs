//! The diff engine (diffs compare rendered slots), as three concerns in three modules:
//!
//! - state transition, here: what the client already has, and what it has after a render;
//! - [`invalidation`]: which slots of a template the client already has must be sent
//!   again, and which entries of a comprehension;
//! - [`encode`]: the Phoenix JSON for a [`Change`].
//!
//! Invalidation and encoding meet only at [`Change`], so compile-time change tracking
//! can replace `invalidation` without touching `encode`.

mod encode;
mod invalidation;

use serde_json::Value;

use super::Rendered;

/// What the client of one connected LiveView has been sent so far. Start a new one
/// for each connection, and pass it every render in order.
#[derive(Debug, Default)]
pub struct DiffState {
    previous: Option<Rendered>,
}

impl DiffState {
    /// The state of a client that has been sent nothing.
    pub fn new() -> DiffState {
        DiffState::default()
    }

    /// The diff that takes the client from the previous render to `rendered`, as the
    /// JSON object Phoenix would send: the statics and every slot when the client does
    /// not have this fingerprint, otherwise only the slots whose output changed, which
    /// is `{}` when none did. The same holds for each nested template at its slot, and
    /// for each entry of a comprehension.
    pub fn render(&mut self, mut rendered: Rendered) -> Value {
        let change = invalidation::change(self.previous.as_ref(), &mut rendered);
        let diff = encode::diff(&change);
        self.previous = Some(rendered);
        diff
    }
}

/// What one render of a template, nested or not, or of one entry of a comprehension,
/// has to tell the client.
struct Change<'a> {
    fingerprint: u64,
    statics: &'static [&'static str],
    root: bool,
    /// The client does not have this template here, so its statics go with the slots.
    new_template: bool,
    /// The slots to send, by index.
    slots: Vec<(usize, SlotChange<'a>)>,
}

enum SlotChange<'a> {
    Html(&'a str),
    Template(Change<'a>),
    Comprehension(ListChange<'a>),
}

/// What one render of a comprehension has to tell the client.
struct ListChange<'a> {
    fingerprint: u64,
    statics: &'static [&'static str],
    /// The client does not have this comprehension here, so the statics go with it.
    new_template: bool,
    /// How many entries there are now. The client drops the ones past the end.
    count: usize,
    /// The entries to send, by index: the index the client has the entry at, when that
    /// is another one, and the slots that changed in it, which may then be none.
    entries: Vec<(usize, Option<usize>, Change<'a>)>,
}
