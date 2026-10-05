//! Capabilities: what the Contexts need from the world outside, as traits the domain
//! owns. The web crate's application state implements them, and is the one place that
//! knows what is behind them: today a store in memory, later a database.

/// A store of sets of unique keys.
pub trait HasStore: Send + Sync {
    /// Adds `key` to the set called `set`. Returns `false`, and changes nothing, if it
    /// was already there. The store decides, as a unique index would, so two requests
    /// cannot both win.
    fn insert(&self, set: &str, key: &str) -> impl Future<Output = bool> + Send;
}
