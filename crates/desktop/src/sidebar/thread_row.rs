use super::ClickHandler;
use super::status_dot::StatusDot;
use chrono::{DateTime, Utc};
use clankerdiff_gpui::ui::prelude::{ListRow, MutedText, UiTheme, icon_button};
use gpui::{
    AnyElement, App, ClickEvent, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};
use gpui_base::Selectable;

/// Formats how long ago `then` was, as of `now`, in the sidebar's compact style: `now`, `5m`,
/// `3h`, `2d`, `6w`, `1y`.
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(then);
    match (elapsed.num_minutes(), elapsed.num_hours(), elapsed.num_days()) {
        (..1, _, _) => "now".to_owned(),
        (minutes, ..1, _) => format!("{minutes}m"),
        (_, hours, ..1) => format!("{hours}h"),
        (_, _, days @ ..7) => format!("{days}d"),
        (_, _, days @ ..365) => format!("{}w", days / 7),
        (_, _, days) => format!("{}y", days / 365),
    }
}

/// One thread in a project section: its status dot, its title, and how long ago it started.
///
/// The selected row shows the thread's actions in place of the time. A row whose context menu
/// is open is highlighted like the selected one.
#[derive(IntoElement)]
pub(crate) struct ThreadRow {
    selector: SharedString,
    title: SharedString,
    /// Shown in place of the title while the title is edited.
    editor: Option<AnyElement>,
    dot: Option<StatusDot>,
    started_at: Option<DateTime<Utc>>,
    selected: bool,
    menu_open: bool,
    theme: UiTheme,
    on_click: Option<ClickHandler>,
    on_stop: Option<ClickHandler>,
    on_reconnect: Option<ClickHandler>,
}

impl ThreadRow {
    /// `selector` identifies the row, both as its element id and its debug selector.
    pub(crate) fn new(selector: impl Into<SharedString>, title: impl Into<SharedString>, theme: &UiTheme) -> Self {
        Self {
            selector: selector.into(),
            title: title.into(),
            editor: None,
            dot: None,
            started_at: None,
            selected: false,
            menu_open: false,
            theme: *theme,
            on_click: None,
            on_stop: None,
            on_reconnect: None,
        }
    }

    pub(crate) fn dot(mut self, dot: Option<StatusDot>) -> Self {
        self.dot = dot;
        self
    }

    pub(crate) fn started_at(mut self, started_at: Option<DateTime<Utc>>) -> Self {
        self.started_at = started_at;
        self
    }

    /// Shows `editor` in place of the title, and stops a click from selecting the row.
    pub(crate) fn editor(mut self, editor: AnyElement) -> Self {
        self.editor = Some(editor);
        self
    }

    pub(crate) fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// Offers a Stop action, tagged `<selector>-stop`, while the row is selected.
    pub(crate) fn on_stop(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_stop = Some(Box::new(handler));
        self
    }

    /// Offers a Reconnect action, tagged `<selector>-reconnect`, while the row is selected.
    pub(crate) fn on_reconnect(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_reconnect = Some(Box::new(handler));
        self
    }
}

impl Selectable for ThreadRow {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }

    fn open(mut self, open: bool) -> Self {
        self.menu_open = open;
        self
    }

    fn is_open(&self) -> bool {
        self.menu_open
    }
}

impl RenderOnce for ThreadRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let selector = self.selector.to_string();
        let theme = self.theme;
        let editing = self.editor.is_some();
        let dot = self.dot.map(|dot| dot.render(&selector, &theme));
        let actions: Vec<AnyElement> = if self.selected {
            [("reconnect", "↻", "Reconnect", self.on_reconnect), ("stop", "×", "Stop thread", self.on_stop)]
                .into_iter()
                .filter_map(|(action, glyph, label, handler)| {
                    let action_selector = format!("{selector}-{action}");
                    let button = icon_button(action_selector.clone(), glyph, label, theme).on_click(handler?);
                    Some(div().debug_selector(move || action_selector).child(button).into_any_element())
                })
                .collect()
        } else {
            Vec::new()
        };
        let time = self.started_at.filter(|_| actions.is_empty()).map(|started_at| {
            div().flex_none().text_xs().child(MutedText::new(relative_time(started_at, Utc::now()), theme))
        });
        let title = match self.editor {
            Some(editor) => editor,
            None => self.title.into_any_element(),
        };
        let on_click = self.on_click.filter(|_| !editing);
        let row = ListRow::new(self.selector, theme)
            .selected(self.selected || self.menu_open)
            .when_some(on_click, ListRow::on_click)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .pl_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_none().w(px(DOT_SLOT)).flex().justify_center().children(dot))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .children(time),
            );
        div()
            .debug_selector(move || selector)
            .w_full()
            .flex()
            .items_center()
            .child(div().flex_1().min_w_0().child(row))
            .children(actions)
    }
}

/// Wide enough for the status dot, and kept when there is none so titles line up.
const DOT_SLOT: f32 = 8.0;
