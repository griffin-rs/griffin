//! The application state: built once at boot, and what handlers take with `State`. It is
//! the composition root: it implements the domain's Capabilities.

use griffin::axum::extract::FromRef;
use griffin::token::SigningKey;
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};

use __app__::capabilities::HasStore;

#[derive(Clone)]
pub struct AppState {
    key: SigningKey,
    store: Arc<Memory>,
}

impl AppState {
    pub fn new(key: SigningKey) -> AppState {
        AppState {
            key,
            store: Memory::shared(),
        }
    }
}

/// The LiveViews' routes take the signing key from the state.
impl FromRef<AppState> for SigningKey {
    fn from_ref(state: &AppState) -> SigningKey {
        state.key.clone()
    }
}

impl HasStore for AppState {
    fn insert(&self, set: &str, key: &str) -> impl Future<Output = bool> + Send {
        self.store.insert(set, key)
    }
}

/// A store in memory, with one username taken: `ada`. It is lost when the process ends.
/// Replace it with a database when the application needs one.
pub struct Memory(Mutex<HashSet<(String, String)>>);

impl Memory {
    /// The store of the process.
    // A LiveView is mounted from its route's parameters alone and is not given
    // the application state, so the one LiveView reaches the store here. Take it from the
    // state like the controller does once LiveViews can.
    pub fn shared() -> Arc<Memory> {
        static STORE: LazyLock<Arc<Memory>> = LazyLock::new(|| {
            let taken = ("usernames".to_owned(), "ada".to_owned());
            Arc::new(Memory(Mutex::new(HashSet::from([taken]))))
        });
        STORE.clone()
    }
}

impl HasStore for Memory {
    async fn insert(&self, set: &str, key: &str) -> bool {
        let entry = (set.to_owned(), key.to_owned());
        self.0
            .lock()
            .expect("nothing panics holding it")
            .insert(entry)
    }
}
