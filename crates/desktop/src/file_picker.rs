use crate::theme::DesktopTheme;
use crate::threads::{Thread, observe_thread};
use crate::ui::tokens::{MODAL_WIDTH, PICKER_LIST_HEIGHT, rem};
use crate::ui::{PickerItem, SearchPicker, SearchPickerEvent};
use clankerdiff_core::RepoPath;
use gpui::{Context, Entity, EventEmitter, Subscription, Window, prelude::*};

pub(crate) struct FilePicker {
    thread: Entity<Thread>,
    picker: Entity<SearchPicker>,
    _subscriptions: [Subscription; 2],
}

impl FilePicker {
    pub(crate) fn new(thread: Entity<Thread>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = DesktopTheme::global(cx).ui();
        let picker = cx.new(|cx| {
            SearchPicker::new("file-picker", Vec::new(), theme, cx)
                .max_matches(MAX_MATCHES)
                .searchable("Search files…", window, cx)
                .empty("No matching files")
                .footer("↑↓ Move    ↵ Open    Esc Close")
                .width(rem(MODAL_WIDTH))
                .height(rem(PICKER_LIST_HEIGHT))
        });
        let subscriptions = [
            observe_thread(&thread, cx, Thread::listed_files, Self::refresh),
            cx.subscribe(&picker, |this, _, event: &SearchPickerEvent, cx| match event {
                SearchPickerEvent::Chose(index) => {
                    let files = this.thread.read(cx).listed_files();
                    if let Some(path) = files.ok().and_then(|files| files.paths.get(*index).cloned()) {
                        cx.emit(Some(path));
                    }
                }
                SearchPickerEvent::Dismissed => cx.emit(None),
                SearchPickerEvent::Query(_) | SearchPickerEvent::Highlighted(_) => {}
            }),
        ];
        let mut files = Self { thread, picker, _subscriptions: subscriptions };
        files.refresh(cx);
        files
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (items, status) = match self.thread.read(cx).listed_files() {
            Ok(files) => (files.paths.iter().map(item).collect(), None),
            Err(status) => (Vec::new(), Some(status.into())),
        };
        self.picker.update(cx, |picker, cx| {
            picker.set_status(status, cx);
            picker.set_items(items, cx);
        });
    }
}

impl EventEmitter<Option<RepoPath>> for FilePicker {}

impl Render for FilePicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.picker.clone()
    }
}

const MAX_MATCHES: usize = 50;

fn item(path: &RepoPath) -> PickerItem {
    let item = PickerItem::new(path.to_string(), path.file_name().to_owned());
    match path.directory() {
        Some(directory) => item.detail(directory.to_owned()),
        None => item,
    }
}
