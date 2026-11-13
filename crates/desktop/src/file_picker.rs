use crate::threads::Thread;
use crate::ui::tokens::{MODAL_WIDTH, PICKER_LIST_HEIGHT};
use crate::ui::{Material, PickerItem, SearchPickerEvent, SearchPickerHandle, backdrop, search_picker};
use clankerdiff_core::RepoPath;
use rouge::core::{Length, View, view};
use rouge::reactive::{Callback, effect, memo, signal, untrack};

pub(crate) fn file_picker(thread: Thread, on_picked: impl Fn(Option<RepoPath>) + 'static) -> impl View {
    let on_picked = Callback::from(on_picked);
    let handle = SearchPickerHandle::new(Vec::new(), move |event| match event {
        SearchPickerEvent::Chose(index) => {
            let path = thread.listed_files().ok().and_then(|files| files.paths.get(index).cloned());
            if path.is_some() {
                on_picked.call(path);
            }
        }
        SearchPickerEvent::Dismissed => on_picked.call(None),
        SearchPickerEvent::Query(_) | SearchPickerEvent::Highlighted(_) => {}
    })
    .max_matches(MAX_MATCHES);
    let status = signal(None);
    let listing = memo(move || thread.listed_files());
    effect(move || {
        let listing = listing.get();
        untrack(|| match listing {
            Ok(files) => {
                status.set(None);
                handle.set_items(files.paths.iter().map(item).collect());
            }
            Err(problem) => {
                status.set(Some(problem));
                handle.set_items(Vec::new());
            }
        });
    });
    view! {
        <backdrop id="file-picker-backdrop" align_top on_dismiss={move || on_picked.call(None)}>
            <search_picker id="file-picker" handle material={Material::Glass} searchable="Search files…"
                           empty="No matching files" footer="↑↓ Move    ↵ Open    Esc Close"
                           width={Length::design(MODAL_WIDTH)} height={Length::design(PICKER_LIST_HEIGHT)} status/>
        </backdrop>
    }
}

const MAX_MATCHES: usize = 50;

fn item(path: &RepoPath) -> PickerItem {
    let item = PickerItem::new(format!("file-picker-{path}"), path.file_name().to_owned());
    match path.directory() {
        Some(directory) => item.detail(directory.to_owned()),
        None => item,
    }
}
