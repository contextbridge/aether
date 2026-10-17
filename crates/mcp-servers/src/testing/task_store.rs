use std::path::PathBuf;

use tempfile::TempDir;

use crate::tasks::{Task, TaskId, TaskStore, TaskUpdate};

/// An on-disk [`TaskStore`] rooted in a temporary directory, set up fluently.
pub struct TestTaskStore {
    directory: TempDir,
    store: TaskStore,
}

impl TestTaskStore {
    /// Creates an empty store in a fresh temporary directory.
    pub fn new() -> Self {
        let directory = TempDir::new().expect("temporary tasks directory");
        let mut store = TaskStore::new(store_root(&directory));
        store.init().expect("failed to initialize task store");
        Self { directory, store }
    }

    /// The store root; task tree files live in `active/` beneath it.
    pub fn root(&self) -> PathBuf {
        store_root(&self.directory)
    }

    /// The store under test.
    pub fn store(&self) -> &TaskStore {
        &self.store
    }

    /// The store under test, mutably.
    pub fn store_mut(&mut self) -> &mut TaskStore {
        &mut self.store
    }

    /// Drops the in-memory tasks and reloads the store from disk.
    pub fn reopen(&mut self) {
        let mut store = TaskStore::new(self.root());
        store.init().expect("failed to re-initialize task store");
        self.store = store;
    }

    /// Creates a root task tree with `title`.
    pub fn root_task(&mut self, title: &str) -> Task {
        self.store.create(None, title, TaskUpdate::default()).expect("failed to create root task")
    }

    /// Adds a subtask with `title` under `parent`.
    pub fn subtask(&mut self, parent: &TaskId, title: &str) -> Task {
        self.store.create(Some(parent), title, TaskUpdate::default()).expect("failed to add subtask")
    }
}

impl Default for TestTaskStore {
    fn default() -> Self {
        Self::new()
    }
}

fn store_root(directory: &TempDir) -> PathBuf {
    directory.path().join(".aether-tasks")
}
