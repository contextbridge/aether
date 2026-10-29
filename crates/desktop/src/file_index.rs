use client_core::composer::attachment::PromptAttachment;
use client_core::composer::file_index::index_files;
use gpui::{App, AppContext, Global};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct FileIndex {
    roots: HashMap<PathBuf, IndexedRoot>,
}

#[derive(Default)]
struct IndexedRoot {
    files: Option<Arc<[PromptAttachment]>>,
    walking: bool,
}

impl FileIndex {
    pub(crate) fn files(root: &Path, cx: &App) -> Option<Arc<[PromptAttachment]>> {
        cx.try_global::<Self>()?.roots.get(root)?.files.clone()
    }

    pub(crate) fn refresh(root: &Path, cx: &mut App) {
        if cx.default_global::<Self>().roots.get(root).is_some_and(|indexed| indexed.walking) {
            return;
        }
        let walk = {
            let root = root.to_path_buf();
            cx.background_spawn(async move { index_files(&root) })
        };
        let key = root.to_path_buf();
        cx.global_mut::<Self>().roots.entry(root.to_path_buf()).or_default().walking = true;
        cx.spawn(async move |cx| {
            let files: Arc<[PromptAttachment]> = walk.await.into();
            cx.update_global::<Self, _>(|index, _| {
                let indexed = index.roots.entry(key).or_default();
                indexed.files = Some(files);
                indexed.walking = false;
            });
        })
        .detach();
    }
}

impl Global for FileIndex {}
