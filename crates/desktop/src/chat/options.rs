use crate::threads::Thread;
use crate::ui::tokens::{COMPOSER_POPUP_HEIGHT, ICON_SIZE_SMALL, RADIUS_INNER, ROW_HEIGHT};
use crate::ui::{
    Icon, IconName, PickerItem, SearchPickerHandle, UiStyles, UiTheme, bare, picker_closed, search_picker, with_middots,
};
use agent_client_protocol::v2::schema as acp;
use clankerdiff_rouge_shell::rouge::core::{AnyView, Color, Cursor, Length, Styled, View, div, dynamic, text, view};
use clankerdiff_rouge_shell::rouge::reactive::{memo, signal};
use clankerdiff_rouge_shell::rouge::widgets::{Placement, popover};
use client_core::session_config_view::LocalConfigOption;

#[derive(Clone, Copy)]
pub(crate) struct SessionOptions {
    thread: Thread,
}

impl SessionOptions {
    pub(crate) fn new(thread: Thread) -> Self {
        Self { thread }
    }

    pub(crate) fn view(self) -> impl View {
        let thread = self.thread;
        let options = memo(move || {
            thread.with_connected_core(|core| core.session().config_options().to_vec()).unwrap_or_default()
        });
        view! {
            <div test_id="session-options" flex_none flex items_center gap=4.0 body>
                <dynamic>{with_middots(options.get().iter().filter_map(|option| self.selector(option)))}</dynamic>
            </div>
        }
    }

    fn selector(self, option: &LocalConfigOption) -> Option<AnyView> {
        let label = option.display_value()?.to_owned();
        let theme = UiTheme::current();
        let id = format!("config-{}", option.id);
        let open = signal(false);
        let hovered = signal(false);
        let icon = category_icon(option).map(|icon| {
            let icon_id = format!("{id}-icon-{icon:?}").to_lowercase();
            view! { <div test_id=icon_id flex>{Icon::new(icon, theme.text_muted).size(ICON_SIZE_SMALL)}</div> }
        });
        let trigger = view! {
            <div test_id={id.clone()} h=ROW_HEIGHT flex items_center gap=4.0 px=8.0 rounded=RADIUS_INNER
                 cursor=Cursor::Pointer
                 bg={move || {
                     if open.get() {
                         theme.surface_selected.get()
                     } else if hovered.get() {
                         theme.surface_hover.get()
                     } else {
                         Color::TRANSPARENT
                     }
                 }}
                 on_hover={move |inside| {
                     hovered.set_if_changed(inside);
                 }}
                 on_click={move |event| {
                     event.stop_propagation();
                     open.set(true);
                 }}>
                {icon}
                <div test_id={format!("{id}-value")}>
                    <text color={move || if open.get() || hovered.get() { theme.text.get() } else { theme.text_secondary.get() }}>
                        {label}
                    </text>
                </div>
            </div>
        };
        let (thread, option) = (self.thread, option.clone());
        let content = move || {
            let Some(select) = option.select() else {
                return ().into_any();
            };
            let values = select.values.clone();
            let current = values.iter().position(|value| value.value == select.current_value).unwrap_or(0);
            let picker = format!("config-{}-picker", option.id);
            let items = values
                .iter()
                .map(|value| {
                    let item = PickerItem::new(format!("{picker}-{}", value.value), value.name.clone())
                        .keywords(value.value.clone())
                        .disabled(value.is_disabled)
                        .checked(value.value == select.current_value);
                    if value.is_disabled { item.detail("unavailable") } else { item }
                })
                .collect();
            let option_id = option.id.clone();
            let handle = SearchPickerHandle::new(items, move |event| {
                let closed = picker_closed(&event, |index| {
                    if let Some(value) = values.get(index) {
                        thread.update_core(|core| core.set_config_option(&option_id, &value.value));
                    }
                });
                if closed {
                    open.set(false);
                }
            })
            .highlighted(current);
            let searchable =
                (select.values.len() > SEARCH_THRESHOLD).then(|| format!("Search {}…", option.name.to_lowercase()));
            bare(|| {
                view! {
                    <search_picker id=picker handle searchable?=searchable title={option.name.clone()}
                                   width={Length::design(PICKER_WIDTH)}
                                   max_list_height={Length::design(COMPOSER_POPUP_HEIGHT)}/>
                }
            })
            .into_any()
        };
        Some(popover(open, trigger, content).placement(Placement::Above).into_any())
    }
}

const SEARCH_THRESHOLD: usize = 8;
const PICKER_WIDTH: f32 = 288.0;

fn category_icon(option: &LocalConfigOption) -> Option<IconName> {
    match option.category.as_ref()? {
        acp::SessionConfigOptionCategory::Mode => Some(IconName::Bot),
        acp::SessionConfigOptionCategory::Model => Some(IconName::Cpu),
        acp::SessionConfigOptionCategory::ThoughtLevel => Some(IconName::Brain),
        _ => None,
    }
}
