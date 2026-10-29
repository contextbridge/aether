use crate::sidebar::{RowKey, SwitcherEntry};
use crate::theme::DesktopTheme;
use crate::threads::ThreadRef;
use crate::ui::tokens::{MODAL_WIDTH, PICKER_LIST_HEIGHT, rem};
use crate::ui::{Backdrop, PickerItem, SearchPicker, UiTheme};
use gpui::{App, AppContext, Entity, ParentElement, Window};

pub(crate) fn picker(entries: &[SwitcherEntry], window: &mut Window, cx: &mut App) -> Entity<SearchPicker> {
    let items = entries
        .iter()
        .map(|entry| {
            let key = match &entry.thread {
                ThreadRef::Live(thread) => RowKey::Live(thread.read(cx).number()),
                ThreadRef::Saved { session_id, .. } => RowKey::Saved(session_id.clone()),
            };
            PickerItem::new(key.selector(), entry.title.clone()).detail(entry.project.clone())
        })
        .collect();
    let theme = DesktopTheme::global(cx).ui();
    cx.new(|cx| {
        SearchPicker::new("switcher", items, theme, cx)
            .searchable("Search threads…", window, cx)
            .empty("No matching threads")
            .footer("↑↓ Move    ↵ Open    Esc Close")
            .width(rem(MODAL_WIDTH))
            .height(rem(PICKER_LIST_HEIGHT))
    })
}

pub(crate) fn overlay(
    picker: Entity<SearchPicker>,
    theme: &UiTheme,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> Backdrop {
    Backdrop::new("thread-switcher", *theme).align_top().on_dismiss(on_dismiss).child(picker)
}
