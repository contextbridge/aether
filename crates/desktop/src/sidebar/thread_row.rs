use super::ClickHandler;
use super::status_dot::StatusDot;
use crate::ui::tokens::DOT_SLOT;
use crate::ui::{IconButton, IconName, ListItem, Typography, TypographyExt};
use chrono::{DateTime, Utc};
use clankerdiff_gpui::ui::prelude::UiTheme;
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
/// While the pointer is over the row, its actions show in place of the time: archive (or
/// unarchive), and stop for a live thread. A thread that can reconnect always offers to. A row
/// whose context menu is open is highlighted like the selected one.
#[derive(IntoElement)]
pub(crate) struct ThreadRow {
    selector: SharedString,
    title: SharedString,
    /// Shown in place of the title while the title is edited.
    editor: Option<AnyElement>,
    dot: Option<StatusDot>,
    started_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    selected: bool,
    menu_open: bool,
    theme: UiTheme,
    on_click: Option<ClickHandler>,
    on_stop: Option<ClickHandler>,
    on_reconnect: Option<ClickHandler>,
    /// Whether the thread is archived, and what its archive button does.
    on_archive: Option<(bool, ClickHandler)>,
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
            now: Utc::now(),
            selected: false,
            menu_open: false,
            theme: *theme,
            on_click: None,
            on_stop: None,
            on_reconnect: None,
            on_archive: None,
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

    /// The time the row's age is told as of.
    pub(crate) fn now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
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

    /// Offers a Stop action, tagged `<selector>-stop`.
    pub(crate) fn on_stop(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_stop = Some(Box::new(handler));
        self
    }

    /// Offers a Reconnect action, tagged `<selector>-reconnect`.
    pub(crate) fn on_reconnect(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_reconnect = Some(Box::new(handler));
        self
    }

    /// Offers an Archive action, tagged `<selector>-archive`, or an Unarchive one, tagged
    /// `<selector>-unarchive`, for an `archived` thread.
    pub(crate) fn on_archive(
        mut self,
        archived: bool,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_archive = Some((archived, Box::new(handler)));
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
        let colors = theme.colors;
        let hover_group = ListItem::hover_group(&selector);
        let editing = self.editor.is_some();
        let dot = self.dot.map(|dot| dot.render(&selector, &theme));
        let button = |action: &str, icon, label: &'static str, handler: ClickHandler| {
            IconButton::new(format!("{selector}-{action}"), icon, label, theme).on_click(handler)
        };
        let reconnect = self.on_reconnect.map(|handler| button("reconnect", IconName::RotateCw, "Reconnect", handler));
        let archive = self.on_archive.map(|(archived, handler)| {
            if archived {
                button("unarchive", IconName::ArchiveRestore, "Unarchive", handler)
            } else {
                button("archive", IconName::Archive, "Archive", handler)
            }
        });
        let stop = self.on_stop.map(|handler| button("stop", IconName::Square, "Stop thread", handler));
        let time = self.started_at.map(|started_at| relative_time(started_at, self.now));
        let highlighted = self.selected || self.menu_open;
        let actions_background = if highlighted { colors.surface_selected } else { colors.surface_hover };
        let trailing = div()
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .justify_end()
            .children(time.map(|time| {
                div()
                    .typography(Typography::Caption)
                    .text_color(colors.text_muted)
                    .group_hover(hover_group.clone(), |time| time.opacity(0.0))
                    .child(time)
            }))
            .child(
                div()
                    .absolute()
                    .right_0()
                    .pl_1()
                    .flex()
                    .items_center()
                    .bg(actions_background)
                    .opacity(0.0)
                    .group_hover(hover_group, |actions| actions.opacity(1.0))
                    .children(archive)
                    .children(stop),
            )
            .children(reconnect);
        let title = match self.editor {
            Some(editor) => editor,
            None => div().min_w_0().truncate().child(self.title).into_any_element(),
        };
        let on_click = self.on_click.filter(|_| !editing);
        ListItem::new(self.selector, theme)
            .selected(highlighted)
            .when_some(on_click, ListItem::on_click)
            .child(div().flex_none().w(px(DOT_SLOT * 2.0)).flex().justify_center().children(dot))
            .child(div().flex_1().min_w_0().child(title))
            .child(trailing)
    }
}
