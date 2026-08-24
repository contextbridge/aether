use crate::uri::path_to_uri;
use globset::{Glob, GlobSet, GlobSetBuilder};
use lsp_types::{FileChangeType, FileEvent, FileSystemWatcher, Uri, WatchKind};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Sleep;

/// Messages sent from the handle to the actor.
enum FileWatcherMsg {
    Register { id: String, watchers: Vec<FileSystemWatcher> },
    Unregister { id: String },
}

#[derive(Debug)]
pub(crate) struct FileWatcherBatch {
    pub(crate) forwarded_changes: Vec<FileEvent>,
    pub(crate) discovered_uris: Vec<Uri>,
}

/// Handle that sends messages to the [`FileWatcherActor`] via an mpsc channel.
pub struct FileWatcherHandle {
    msg_tx: mpsc::Sender<FileWatcherMsg>,
    _task: JoinHandle<()>,
}

impl FileWatcherHandle {
    /// Spawn the actor task and return a handle to it.
    pub fn spawn(workspace_root: PathBuf, event_tx: mpsc::Sender<FileWatcherBatch>) -> Self {
        let (bridge_tx, bridge_rx) = mpsc::channel::<Event>(256);
        let watcher = match create_watcher(&workspace_root, bridge_tx) {
            Ok(w) => {
                tracing::debug!("Started file watcher on {}", workspace_root.display());
                Some(w)
            }
            Err(e) => {
                tracing::error!("Failed to start file watcher: {e}");
                None
            }
        };
        Self::spawn_actor(workspace_root, watcher, bridge_rx, event_tx)
    }

    /// Wire the actor up to its channels and run it on a spawned task.
    ///
    /// The OS watcher is injectable so tests can drive the actor through the same
    /// bridge channel a real `notify` watcher produces events on.
    pub(crate) fn spawn_actor(
        workspace_root: PathBuf,
        watcher: Option<RecommendedWatcher>,
        bridge_rx: mpsc::Receiver<Event>,
        event_tx: mpsc::Sender<FileWatcherBatch>,
    ) -> Self {
        let (msg_tx, msg_rx) = mpsc::channel(64);
        let canonical_workspace_root =
            std::fs::canonicalize(&workspace_root).ok().filter(|canonical| canonical != &workspace_root);
        if let Some(canonical) = &canonical_workspace_root {
            tracing::debug!(
                workspace_root = %workspace_root.display(),
                canonical_workspace_root = %canonical.display(),
                "File watcher: using canonical workspace root for path matching"
            );
        }

        let actor = FileWatcherActor {
            _watcher: watcher,
            workspace_root,
            canonical_workspace_root,
            event_tx,
            msg_rx,
            bridge_rx,
            forwarded_pending: HashMap::new(),
            discovered_pending: HashMap::new(),
            registrations: HashMap::new(),
            glob_set: GlobSet::empty(),
            watch_kinds: Vec::new(),
        };

        let task = tokio::spawn(actor.run());

        Self { msg_tx, _task: task }
    }

    /// Register file watchers for a `workspace/didChangeWatchedFiles` registration.
    pub fn register_watchers(&self, id: String, watchers: Vec<FileSystemWatcher>) {
        let _ = self.msg_tx.try_send(FileWatcherMsg::Register { id, watchers });
    }

    /// Unregister file watchers for a given registration ID.
    pub fn unregister(&self, id: String) {
        let _ = self.msg_tx.try_send(FileWatcherMsg::Unregister { id });
    }
}

/// Owns all file-watcher state and processes messages sequentially in a spawned task.
struct FileWatcherActor {
    _watcher: Option<RecommendedWatcher>,
    workspace_root: PathBuf,
    canonical_workspace_root: Option<PathBuf>,
    event_tx: mpsc::Sender<FileWatcherBatch>,
    msg_rx: mpsc::Receiver<FileWatcherMsg>,
    bridge_rx: mpsc::Receiver<notify::Event>,
    forwarded_pending: HashMap<String, (Uri, FileChangeType)>,
    discovered_pending: HashMap<String, Uri>,
    registrations: HashMap<String, Vec<FileSystemWatcher>>,
    glob_set: GlobSet,
    watch_kinds: Vec<WatchKind>,
}

impl FileWatcherActor {
    async fn run(mut self) {
        let debounce = Duration::from_millis(200);
        let mut timer: Option<Pin<Box<Sleep>>> = None;

        loop {
            tokio::select! {
                msg = self.msg_rx.recv() => {
                    let Some(msg) = msg else { break };
                    match msg {
                        FileWatcherMsg::Register { id, watchers } => {
                            self.registrations.insert(id, watchers);
                            self.rebuild_glob_state();
                        }
                        FileWatcherMsg::Unregister { id } => {
                            if self.registrations.remove(&id).is_some() {
                                tracing::debug!("Unregistered file watcher {id}");
                                self.rebuild_glob_state();
                            }
                        }
                    }
                }
                Some(ev) = self.bridge_rx.recv() => {
                    self.accumulate_event(&ev);
                    if self.has_pending() {
                        timer = Some(Box::pin(tokio::time::sleep(debounce)));
                    }
                }
                () = async { match &mut timer { Some(t) => t.as_mut().await, None => std::future::pending().await } } => {
                    timer = None;
                    self.flush_pending().await;
                }
            }
        }

        self.flush_pending().await;
    }

    fn rebuild_glob_state(&mut self) {
        let all_watchers: Vec<&FileSystemWatcher> = self.registrations.values().flat_map(|v| v.iter()).collect();

        let (glob_set, watch_kinds) = build_glob_set(&all_watchers).unwrap_or_else(|| (GlobSet::empty(), Vec::new()));

        self.glob_set = glob_set;
        self.watch_kinds = watch_kinds;
    }

    fn accumulate_event(&mut self, ev: &Event) {
        for path in &ev.paths {
            let rel_from_workspace = path.strip_prefix(&self.workspace_root).ok();
            let rel_from_canonical =
                self.canonical_workspace_root.as_ref().and_then(|root| path.strip_prefix(root).ok());
            let rel = rel_from_workspace.or(rel_from_canonical).unwrap_or(path.as_path());
            let within_workspace = rel_from_workspace.is_some() || rel_from_canonical.is_some();

            // Try relative path first, fall back to absolute
            let matches = self.glob_set.matches(rel);
            let matches = if matches.is_empty() { self.glob_set.matches(path) } else { matches };
            if matches.is_empty() {
                // Keep implicit URI discovery enabled even after watcher registration.
                // Some LSPs register narrow globs (e.g. Cargo files) that do not include
                // all source-file edits needed by all-files diagnostics.
                if !should_track_implicit_path(ev.kind, within_workspace, rel, path) {
                    tracing::trace!(
                        path = %path.display(),
                        kind = ?ev.kind,
                        "File event: implicit discovery ignored path"
                    );
                    continue;
                }
                if map_event_kind(ev.kind, WatchKind::Create | WatchKind::Change | WatchKind::Delete).is_none() {
                    continue;
                }
                let Ok(uri) = path_to_uri(path) else {
                    continue;
                };
                let key = uri.to_string();
                tracing::trace!(
                    path = %path.display(),
                    kind = ?ev.kind,
                    "File event: no glob match, tracking URI discovery only"
                );
                self.discovered_pending.insert(key, uri);
                continue;
            }

            let effective_kinds = matches.iter().fold(WatchKind::empty(), |acc, &i| acc | self.watch_kinds[i]);

            let Some(change_type) = map_event_kind(ev.kind, effective_kinds) else {
                continue;
            };

            let Ok(uri) = path_to_uri(path) else {
                continue;
            };
            let key = uri.to_string();
            tracing::debug!(
                path = %path.display(),
                change_type = ?change_type,
                "File event: accumulated for debounced dispatch"
            );
            self.forwarded_pending.insert(key, (uri, change_type));
        }
    }

    async fn flush_pending(&mut self) {
        if !self.has_pending() {
            return;
        }

        let mut forwarded_changes: Vec<FileEvent> =
            self.forwarded_pending.drain().map(|(_, (uri, typ))| FileEvent { uri, typ }).collect();
        forwarded_changes.sort_by(|a, b| a.uri.as_str().cmp(b.uri.as_str()));

        let mut discovered_uris: Vec<Uri> = self.discovered_pending.drain().map(|(_, uri)| uri).collect();
        discovered_uris.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        tracing::debug!(
            forwarded_changes = forwarded_changes.len(),
            discovered_uris = discovered_uris.len(),
            "Sending file watcher batch"
        );

        let batch = FileWatcherBatch { forwarded_changes, discovered_uris };
        if self.event_tx.send(batch).await.is_err() {
            tracing::debug!("File watcher channel closed");
        }
    }

    fn has_pending(&self) -> bool {
        !self.forwarded_pending.is_empty() || !self.discovered_pending.is_empty()
    }
}

/// Create the OS file watcher that bridges notify events into an mpsc channel.
fn create_watcher(workspace_root: &Path, tx: mpsc::Sender<Event>) -> Result<RecommendedWatcher, notify::Error> {
    let mut watcher = RecommendedWatcher::new(
        move |event: Result<Event, notify::Error>| match event {
            Ok(e) => {
                let _ = tx.blocking_send(e);
            }
            Err(e) => {
                tracing::debug!("File watcher error: {e}");
            }
        },
        Config::default(),
    )?;

    watcher.watch(workspace_root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Build a `GlobSet` paired with per-glob `WatchKind` flags.
///
/// The returned `Vec<WatchKind>` is index-aligned with the globs added to the `GlobSet`,
/// so `GlobSet::matches(path)` indices can be used to look up the corresponding kind.
fn build_glob_set(watchers: &[&FileSystemWatcher]) -> Option<(GlobSet, Vec<WatchKind>)> {
    let mut builder = GlobSetBuilder::new();
    let mut kinds = Vec::new();

    for w in watchers {
        let pattern = match &w.glob_pattern {
            lsp_types::GlobPattern::String(s) => s.as_str(),
            lsp_types::GlobPattern::Relative(rp) => rp.pattern.as_str(),
        };

        match Glob::new(pattern) {
            Ok(g) => {
                builder.add(g);
                kinds.push(w.kind.unwrap_or(WatchKind::Create | WatchKind::Change | WatchKind::Delete));
            }
            Err(e) => {
                tracing::warn!("Invalid glob pattern '{pattern}': {e}");
            }
        }
    }

    builder
        .build()
        .inspect_err(|e| tracing::error!("Failed to build glob set: {e}"))
        .ok()
        .filter(|gs| !gs.is_empty())
        .map(|gs| (gs, kinds))
}

/// Map a `notify::EventKind` to an LSP `FileChangeType`, filtered by requested `WatchKind`.
fn map_event_kind(kind: EventKind, watch_kinds: WatchKind) -> Option<FileChangeType> {
    match kind {
        EventKind::Create(_) if watch_kinds.contains(WatchKind::Create) => Some(FileChangeType::CREATED),
        EventKind::Modify(_) if watch_kinds.contains(WatchKind::Change) => Some(FileChangeType::CHANGED),
        EventKind::Remove(_) if watch_kinds.contains(WatchKind::Delete) => Some(FileChangeType::DELETED),
        _ => None,
    }
}

fn should_track_implicit_path(kind: EventKind, within_workspace: bool, rel_path: &Path, absolute_path: &Path) -> bool {
    // Only discover implicit URIs inside the watched workspace root.
    if !within_workspace {
        return false;
    }

    // Skip noisy build/system directories that can generate thousands of irrelevant
    // changes and binary blobs (which can't be opened as text documents anyway).
    for component in rel_path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let component = name.to_string_lossy();
        if matches!(component.as_ref(), ".git" | "node_modules" | ".next" | "dist" | "build" | "target") {
            return false;
        }
    }

    // Ignore directory changes for implicit discovery; these can't be synced as
    // text documents and only add noise to known_uris.
    if !matches!(kind, EventKind::Remove(_)) && absolute_path.is_dir() {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODIFY_CONTENT: EventKind =
        EventKind::Modify(notify::event::ModifyKind::Data(notify::event::DataChange::Content));
    const CREATE_FILE: EventKind = EventKind::Create(notify::event::CreateKind::File);
    const REMOVE_FILE: EventKind = EventKind::Remove(notify::event::RemoveKind::File);

    fn watcher(pattern: &str, kind: Option<WatchKind>) -> FileSystemWatcher {
        FileSystemWatcher { glob_pattern: lsp_types::GlobPattern::String(pattern.into()), kind }
    }

    /// A file watcher driven entirely through its channels: registrations go
    /// through the public handle, events arrive on the bridge channel a real
    /// `notify` watcher would feed, and assertions read the emitted batches.
    struct WatchedProject {
        dir: tempfile::TempDir,
        handle: FileWatcherHandle,
        bridge_tx: mpsc::Sender<Event>,
        batches: mpsc::Receiver<FileWatcherBatch>,
    }

    impl WatchedProject {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir should be created");
            let (bridge_tx, bridge_rx) = mpsc::channel(8);
            let (event_tx, event_rx) = mpsc::channel(8);
            let handle = FileWatcherHandle::spawn_actor(dir.path().to_path_buf(), None, bridge_rx, event_tx);
            Self { dir, handle, bridge_tx, batches: event_rx }
        }

        /// Register watchers and let the actor apply them before returning.
        async fn register(&self, id: &str, watchers: Vec<FileSystemWatcher>) {
            self.handle.register_watchers(id.into(), watchers);
            tokio::task::yield_now().await;
        }

        async fn unregister(&self, id: &str) {
            self.handle.unregister(id.into());
            tokio::task::yield_now().await;
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.dir.path().join(relative)
        }

        async fn send(&self, kind: EventKind, path: PathBuf) {
            self.bridge_tx
                .send(Event { kind, paths: vec![path], attrs: notify::event::EventAttributes::new() })
                .await
                .expect("bridge channel should accept events");
        }

        async fn next_batch(&mut self) -> FileWatcherBatch {
            self.batches.recv().await.expect("file watcher batch should be emitted")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn matching_glob_events_are_forwarded() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("**/*.rs", Some(WatchKind::Change))]).await;
        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;

        let batch = project.next_batch().await;

        assert_eq!(batch.forwarded_changes.len(), 1);
        assert_eq!(batch.forwarded_changes[0].typ, FileChangeType::CHANGED);
        assert_eq!(
            batch.forwarded_changes[0].uri.as_str(),
            path_to_uri(&project.path("src/main.rs")).unwrap().as_str()
        );
        assert!(batch.discovered_uris.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn events_outside_registered_globs_are_discovered_only() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("**/*.rs", Some(WatchKind::Change))]).await;
        project.send(MODIFY_CONTENT, project.path("src/main.py")).await;

        let batch = project.next_batch().await;

        assert!(batch.forwarded_changes.is_empty());
        assert_eq!(batch.discovered_uris.len(), 1);
        assert_eq!(batch.discovered_uris[0].as_str(), path_to_uri(&project.path("src/main.py")).unwrap().as_str());
    }

    #[tokio::test(start_paused = true)]
    async fn events_are_filtered_by_requested_watch_kinds() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("**/*.rs", Some(WatchKind::Create))]).await;

        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;
        project.send(CREATE_FILE, project.path("src/main.rs")).await;

        let batch = project.next_batch().await;

        assert_eq!(batch.forwarded_changes.len(), 1, "only the create should be forwarded");
        assert_eq!(batch.forwarded_changes[0].typ, FileChangeType::CREATED);
        assert!(batch.discovered_uris.is_empty(), "glob-matched paths are never discovered");
    }

    #[tokio::test(start_paused = true)]
    async fn per_watcher_kinds_are_preserved_across_registrations() {
        let mut project = WatchedProject::new();
        project.register("rust", vec![watcher("**/*.rs", Some(WatchKind::Create))]).await;
        project.register("json", vec![watcher("**/*.json", Some(WatchKind::Delete))]).await;

        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;
        project.send(CREATE_FILE, project.path("src/main.rs")).await;
        project.send(REMOVE_FILE, project.path("data.json")).await;

        let batch = project.next_batch().await;

        let forwarded: Vec<(String, FileChangeType)> =
            batch.forwarded_changes.iter().map(|event| (event.uri.as_str().to_string(), event.typ)).collect();
        assert_eq!(
            forwarded,
            vec![
                (path_to_uri(&project.path("data.json")).unwrap().as_str().to_string(), FileChangeType::DELETED),
                (path_to_uri(&project.path("src/main.rs")).unwrap().as_str().to_string(), FileChangeType::CREATED),
            ],
            "each registration's watch kinds must filter independently"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_glob_patterns_are_skipped_and_valid_ones_still_work() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("[invalid", None), watcher("**/*.rs", Some(WatchKind::Change))]).await;

        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;
        let batch = project.next_batch().await;

        assert_eq!(
            batch.forwarded_changes.len(),
            1,
            "valid globs must survive invalid patterns in the same registration"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn unregister_stops_matching_globs() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("**/*.rs", Some(WatchKind::Change))]).await;

        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;
        let batch = project.next_batch().await;
        assert_eq!(batch.forwarded_changes.len(), 1);

        project.unregister("reg").await;
        project.send(MODIFY_CONTENT, project.path("src/main.rs")).await;
        let batch = project.next_batch().await;
        assert!(batch.forwarded_changes.is_empty());
        assert_eq!(batch.discovered_uris.len(), 1, "after unregistering, paths are only discovered");
    }

    #[tokio::test(start_paused = true)]
    async fn bursts_of_events_debounce_into_one_deduplicated_sorted_batch() {
        let mut project = WatchedProject::new();
        project.register("reg", vec![watcher("**/*.rs", Some(WatchKind::Create | WatchKind::Change))]).await;

        project.send(MODIFY_CONTENT, project.path("src/zeta.rs")).await;
        project.send(CREATE_FILE, project.path("src/alpha.rs")).await;
        project.send(MODIFY_CONTENT, project.path("src/alpha.rs")).await;

        let batch = project.next_batch().await;

        let forwarded: Vec<(String, FileChangeType)> =
            batch.forwarded_changes.iter().map(|event| (event.uri.as_str().to_string(), event.typ)).collect();
        assert_eq!(
            forwarded,
            vec![
                (path_to_uri(&project.path("src/alpha.rs")).unwrap().as_str().to_string(), FileChangeType::CHANGED),
                (path_to_uri(&project.path("src/zeta.rs")).unwrap().as_str().to_string(), FileChangeType::CHANGED),
            ],
            "bursts must flush as one batch, deduplicated per URI and sorted by URI"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn implicit_discovery_ignores_noise_directories_and_paths_outside_the_workspace() {
        let mut project = WatchedProject::new();
        std::fs::create_dir_all(project.path("src")).expect("directory should be created");

        project.send(MODIFY_CONTENT, project.path("target/debug/incremental/dep-graph.bin")).await;
        project.send(MODIFY_CONTENT, project.path("src")).await;
        project.send(MODIFY_CONTENT, PathBuf::from("/tmp/aether-lspd-outside-workspace/other.rs")).await;
        project.send(MODIFY_CONTENT, project.path("targeting/main.rs")).await;
        project.send(MODIFY_CONTENT, project.path("src/keep.rs")).await;

        let batch = project.next_batch().await;

        let discovered: Vec<String> = batch.discovered_uris.iter().map(|uri| uri.as_str().to_string()).collect();
        assert_eq!(
            discovered,
            vec![
                path_to_uri(&project.path("src/keep.rs")).unwrap().as_str().to_string(),
                path_to_uri(&project.path("targeting/main.rs")).unwrap().as_str().to_string(),
            ],
            "only files inside the workspace outside noise directories should be discovered"
        );
        assert!(batch.forwarded_changes.is_empty());
    }
}
