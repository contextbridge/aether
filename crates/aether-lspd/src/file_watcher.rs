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
        let (msg_tx, msg_rx) = mpsc::channel(64);
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
    const ALL_KINDS: WatchKind =
        WatchKind::from_bits_truncate(WatchKind::Create.bits() | WatchKind::Change.bits() | WatchKind::Delete.bits());

    fn watcher(pattern: &str, kind: Option<WatchKind>) -> FileSystemWatcher {
        FileSystemWatcher { glob_pattern: lsp_types::GlobPattern::String(pattern.into()), kind }
    }

    fn build(watchers: &[FileSystemWatcher]) -> Option<(GlobSet, Vec<WatchKind>)> {
        let refs: Vec<&FileSystemWatcher> = watchers.iter().collect();
        build_glob_set(&refs)
    }

    fn matched_kind(gs: &GlobSet, kinds: &[WatchKind], path: &str) -> WatchKind {
        gs.matches(path).iter().fold(WatchKind::empty(), |acc, &i| acc | kinds[i])
    }

    fn notify_event(kind: EventKind, paths: Vec<&str>) -> Event {
        Event {
            kind,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            attrs: notify::event::EventAttributes::new(),
        }
    }

    fn test_actor(workspace_root: &str) -> (FileWatcherActor, mpsc::Receiver<FileWatcherBatch>) {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (_, msg_rx) = mpsc::channel(8);
        let (_, bridge_rx) = mpsc::channel(8);
        (
            FileWatcherActor {
                _watcher: None,
                workspace_root: PathBuf::from(workspace_root),
                canonical_workspace_root: None,
                event_tx,
                msg_rx,
                bridge_rx,
                forwarded_pending: HashMap::new(),
                discovered_pending: HashMap::new(),
                registrations: HashMap::new(),
                glob_set: GlobSet::empty(),
                watch_kinds: Vec::new(),
            },
            event_rx,
        )
    }

    fn actor_with_globs(root: &str, globs: &[(&str, WatchKind)]) -> FileWatcherActor {
        let (mut actor, _) = test_actor(root);
        for (i, (pattern, kind)) in globs.iter().enumerate() {
            actor.registrations.insert(format!("reg{i}"), vec![watcher(pattern, Some(*kind))]);
        }
        actor.rebuild_glob_state();
        actor
    }

    #[test]
    fn test_build_glob_set() {
        let (gs, kinds) = build(&[watcher("**/*.rs", Some(ALL_KINDS))]).unwrap();
        assert!(gs.is_match("src/main.rs"));
        assert!(!gs.is_match("src/main.py"));
        assert_eq!(kinds, [ALL_KINDS]);
    }

    #[test]
    fn test_build_glob_set_none_cases() {
        assert!(build(&[watcher("[invalid", None)]).is_none());
        assert!(build(&[]).is_none());
    }

    #[test]
    fn test_build_glob_set_preserves_per_watcher_kinds() {
        let (gs, kinds) =
            build(&[watcher("**/*.rs", Some(WatchKind::Create)), watcher("**/*.json", Some(WatchKind::Delete))])
                .unwrap();

        assert_eq!(matched_kind(&gs, &kinds, "src/main.rs"), WatchKind::Create);
        assert_eq!(matched_kind(&gs, &kinds, "config.json"), WatchKind::Delete);

        // Wrong event kind for each pattern should not pass the filter
        assert!(map_event_kind(REMOVE_FILE, WatchKind::Create).is_none());
        assert!(map_event_kind(CREATE_FILE, WatchKind::Delete).is_none());
    }

    #[test]
    fn test_build_glob_set_skips_invalid_keeps_indices_aligned() {
        let (gs, kinds) = build(&[
            watcher("**/*.rs", Some(WatchKind::Create)),
            watcher("[invalid", Some(WatchKind::Change)),
            watcher("**/*.json", Some(WatchKind::Delete)),
        ])
        .unwrap();

        assert_eq!(kinds, [WatchKind::Create, WatchKind::Delete]);
        assert_eq!(gs.matches("lib.rs"), vec![0]);
        assert_eq!(gs.matches("data.json"), vec![1]);
    }

    #[test]
    fn test_map_event_kind() {
        for (kind, watch, expected) in [
            (CREATE_FILE, ALL_KINDS, Some(FileChangeType::CREATED)),
            (MODIFY_CONTENT, ALL_KINDS, Some(FileChangeType::CHANGED)),
            (REMOVE_FILE, ALL_KINDS, Some(FileChangeType::DELETED)),
            (CREATE_FILE, WatchKind::Change, None),
        ] {
            assert_eq!(map_event_kind(kind, watch), expected, "kind={kind:?} watch={watch:?}");
        }
    }

    #[test]
    fn test_path_to_uri() {
        let uri = path_to_uri(Path::new("/home/user/project/src/main.rs")).unwrap();
        assert_eq!(uri.to_string(), "file:///home/user/project/src/main.rs");
    }

    #[test]
    fn test_should_track_implicit_path() {
        for (label, expected, within, rel, abs) in [
            (
                "skips target tree",
                false,
                true,
                "target/debug/incremental/foo.bin",
                "/tmp/project/target/debug/incremental/foo.bin",
            ),
            ("allows targeting dir name", true, true, "targeting/main.rs", "/tmp/project/targeting/main.rs"),
            ("rejects outside workspace", false, false, "/tmp/other/main.rs", "/tmp/other/main.rs"),
        ] {
            assert_eq!(
                should_track_implicit_path(MODIFY_CONTENT, within, Path::new(rel), Path::new(abs)),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn test_rebuild_glob_state_combines_registrations() {
        let actor = actor_with_globs("/tmp", &[("**/*.rs", WatchKind::Create), ("**/*.json", WatchKind::Delete)]);
        assert!(actor.glob_set.is_match("src/main.rs"));
        assert!(actor.glob_set.is_match("config.json"));
        assert!(!actor.glob_set.is_match("readme.md"));
        assert_eq!(actor.watch_kinds.len(), 2);
    }

    #[test]
    fn test_accumulate_event_implicit_mode_without_globs() {
        let (mut actor, _) = test_actor("/tmp/project");
        actor.accumulate_event(&notify_event(MODIFY_CONTENT, vec!["/tmp/project/src/main.rs"]));
        assert!(actor.forwarded_pending.is_empty());
        assert_eq!(actor.discovered_pending.len(), 1);
    }

    #[test]
    fn test_accumulate_event_matching_glob_forwards_changes() {
        let mut actor = actor_with_globs("/tmp/project", &[("**/*.rs", WatchKind::Change)]);
        actor.accumulate_event(&notify_event(MODIFY_CONTENT, vec!["/tmp/project/src/main.rs"]));
        assert_eq!(actor.forwarded_pending.len(), 1);
        assert!(actor.discovered_pending.is_empty());
        let (_, change_type) = actor.forwarded_pending.values().next().unwrap();
        assert_eq!(*change_type, FileChangeType::CHANGED);
    }

    #[test]
    fn test_accumulate_event_tracks_non_matching_paths() {
        let mut actor = actor_with_globs("/tmp/project", &[("**/*.rs", WatchKind::Change)]);
        actor.accumulate_event(&notify_event(MODIFY_CONTENT, vec!["/tmp/project/src/main.py"]));
        assert!(actor.forwarded_pending.is_empty());
        assert_eq!(actor.discovered_pending.len(), 1);
    }

    #[test]
    fn test_accumulate_event_ignores_noise_in_implicit_mode() {
        for path in ["/tmp/project/target/debug/incremental/foo/dep-graph.bin", "/tmp/other/outside.rs"] {
            let (mut actor, _) = test_actor("/tmp/project");
            actor.accumulate_event(&notify_event(MODIFY_CONTENT, vec![path]));
            assert!(actor.forwarded_pending.is_empty(), "forwarded should be empty for {path}");
            assert!(actor.discovered_pending.is_empty(), "discovered should be empty for {path}");
        }
    }

    #[tokio::test]
    async fn test_flush_pending_only_discovered_emits_track_only_batch() {
        let (mut actor, mut event_rx) = test_actor("/tmp/project");
        actor.accumulate_event(&notify_event(MODIFY_CONTENT, vec!["/tmp/project/src/main.rs"]));
        actor.flush_pending().await;
        let batch = event_rx.recv().await.expect("expected file watcher batch");
        assert!(batch.forwarded_changes.is_empty());
        assert_eq!(batch.discovered_uris.len(), 1);
    }

    #[tokio::test]
    async fn test_register_and_unregister_via_handle() {
        let (event_tx, _) = mpsc::channel(64);
        let handle = FileWatcherHandle::spawn(PathBuf::from("/tmp/test"), event_tx);
        handle.register_watchers("reg1".into(), vec![watcher("**/*.rs", Some(WatchKind::Create))]);
        handle.register_watchers("reg2".into(), vec![watcher("**/*.json", Some(WatchKind::Delete))]);
        handle.unregister("reg1".into());
        handle.unregister("reg2".into());
        drop(handle);
        tokio::task::yield_now().await;
    }
}
