use crate::theme::DesktopTheme;
use crate::threads::{Thread, observe_thread};
use crate::ui::UiTheme;
use crate::ui::tokens::{COMPOSER_POPUP_HEIGHT, ICON_SIZE_SMALL, RADIUS_INNER, ROW_HEIGHT, rem};
use crate::ui::{
    Icon, IconName, PickerItem, SearchPicker, SearchPickerEvent, Typography, TypographyExt, picker_closed,
    text_selector, with_middots,
};
use agent_client_protocol::schema::v2 as acp;
use client_core::session_config_view::LocalConfigOption;
use gpui::{
    Anchor, AnyElement, App, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*,
};
use gpui_base::Popover;

pub(crate) struct SessionOptions {
    thread: Entity<Thread>,
    open: Option<OpenPicker>,
    _subscription: Subscription,
}

pub(crate) struct PickerClosed;

impl SessionOptions {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscription = observe_thread(
            &thread,
            cx,
            |thread| thread.connected_core().map(|core| core.session().config_options().to_vec()),
            |_, cx| cx.notify(),
        );
        Self { thread, open: None, _subscription: subscription }
    }

    fn open_picker(&mut self, option_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(option) = self
            .thread
            .read(cx)
            .connected_core()
            .and_then(|core| core.session().config_options().iter().find(|option| option.id == option_id).cloned())
        else {
            return;
        };
        let Some(select) = option.select() else {
            return;
        };
        let values = select.values.clone();
        let current = values.iter().position(|value| value.value == select.current_value).unwrap_or(0);
        let items = values
            .iter()
            .map(|value| {
                let item = PickerItem::new(value.value.clone(), value.name.clone())
                    .keywords(value.value.clone())
                    .disabled(value.is_disabled)
                    .checked(value.value == select.current_value);
                if value.is_disabled { item.detail("unavailable") } else { item }
            })
            .collect();
        let theme = DesktopTheme::global(cx).ui();
        let picker = cx.new(|cx| {
            let picker = SearchPicker::new(format!("config-{}-picker", option.id), items, theme, cx)
                .title(option.name.clone())
                .width(rem(PICKER_WIDTH))
                .max_list_height(rem(COMPOSER_POPUP_HEIGHT))
                .highlight(current);
            if values.len() > SEARCH_THRESHOLD {
                picker.searchable(format!("Search {}…", option.name.to_lowercase()), window, cx)
            } else {
                picker
            }
        });
        let subscription = cx.subscribe_in(&picker, window, move |this, _, event: &SearchPickerEvent, _, cx| {
            let closed = picker_closed(event, |index| {
                let value = &values[index].value;
                Thread::with_core(&this.thread, cx, |core| core.set_config_option(&option_id, value));
            });
            if closed {
                this.close_picker(cx);
            }
        });
        self.open = Some(OpenPicker { option_id: option.id.clone(), picker, _subscription: subscription });
        cx.notify();
    }

    fn close_picker(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.emit(PickerClosed);
            cx.notify();
        }
    }
}

impl EventEmitter<PickerClosed> for SessionOptions {}

impl Render for SessionOptions {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let options = self.thread.read(cx).connected_core().map_or(&[][..], |core| core.session().config_options());
        let selectors = options.iter().filter_map(|option| {
            let picker = self.open.as_ref().filter(|open| open.option_id == option.id).map(|open| open.picker.clone());
            let option_id = option.id.clone();
            let on_open_change = cx.listener(move |this, open: &bool, window, cx| {
                if *open {
                    this.open_picker(option_id.clone(), window, cx);
                } else {
                    this.close_picker(cx);
                }
            });
            config_selector(option, picker, &theme, on_open_change)
        });
        let selectors = with_middots(selectors.collect::<Vec<_>>(), &theme);
        div()
            .debug_selector(|| "session-options".to_owned())
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .typography(Typography::Body)
            .children(selectors)
    }
}

struct OpenPicker {
    option_id: String,
    picker: Entity<SearchPicker>,
    _subscription: Subscription,
}

const SEARCH_THRESHOLD: usize = 8;
const PICKER_WIDTH: f32 = 288.0;

fn config_selector(
    option: &LocalConfigOption,
    picker: Option<Entity<SearchPicker>>,
    theme: &UiTheme,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    let label = option.display_value()?.to_owned();
    let selector = format!("config-{}", option.id);
    let (text, hover, selected) = (theme.colors.text, theme.colors.surface_hover, theme.colors.surface_selected);
    let trigger = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector({
            let selector = selector.clone();
            move || selector
        })
        .h(rem(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .rounded(rem(RADIUS_INNER))
        .text_color(theme.colors.text_secondary)
        .cursor_pointer()
        .hover(|trigger| trigger.text_color(text).bg(hover))
        .children(category_icon(option).map(|icon| {
            let selector = format!("{selector}-icon-{icon:?}").to_lowercase();
            div()
                .flex()
                .debug_selector(move || selector)
                .child(Icon::new(icon, theme.colors.text_muted).size(ICON_SIZE_SMALL))
        }))
        .child({
            let selector = text_selector(&format!("{selector}-value"), &label);
            div().debug_selector(move || selector).child(label)
        });
    let open = picker.is_some();
    let popover = Popover::new(SharedString::from(format!("{selector}-popover")))
        .anchor(Anchor::BottomLeft)
        .open(open)
        .on_open_change(on_open_change)
        .trigger_with(move |open, _, _| {
            trigger.when(open, |trigger| trigger.text_color(text).bg(selected)).into_any_element()
        })
        .content(move |_, _, _| div().children(picker));
    Some(popover.into_any_element())
}

fn category_icon(option: &LocalConfigOption) -> Option<IconName> {
    match option.category.as_ref()? {
        acp::SessionConfigOptionCategory::Mode => Some(IconName::Bot),
        acp::SessionConfigOptionCategory::Model => Some(IconName::Cpu),
        acp::SessionConfigOptionCategory::ThoughtLevel => Some(IconName::Brain),
        _ => None,
    }
}
