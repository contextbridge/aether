use super::ClickHandler;
use crate::registry::Disclosure;
use crate::ui::UiTheme;
use crate::ui::tokens::{GROUP_HEIGHT, ICON_SIZE_SMALL, RADIUS_CONTROL, STATUS_SLOT, rem};
use crate::ui::{Icon, IconButton, IconName, ListRow, MenuItem, Typography, TypographyExt, with_menu};
use gpui::{
    AnyElement, App, ClickEvent, Div, IntoElement, MouseButton, ParentElement, RenderOnce, SharedString, Window, div,
    prelude::*,
};

/// A project's header, with a disclosure toggle, a `⋯` menu of `actions` and a `+` that starts a
/// thread in it, or offers a menu of agents to start one with, above the project's threads, a
/// "Show N more" row when some of its saved sessions are hidden, and its archived threads in a
/// group of their own.
///
/// The menu and `+` show while the pointer is over the header.
#[derive(IntoElement)]
pub(crate) struct ProjectSection {
    name: SharedString,
    disclosure: Disclosure,
    theme: UiTheme,
    on_toggle: Option<ClickHandler>,
    on_new_thread: Option<ClickHandler>,
    new_thread_menu: Vec<MenuItem>,
    actions: Vec<MenuItem>,
    show_more: Option<(usize, ClickHandler)>,
    archived: Option<ArchivedGroup>,
    children: Vec<AnyElement>,
}

/// A project's archived threads, under a header that expands and collapses them.
pub(crate) struct ArchivedGroup {
    pub(crate) disclosure: Disclosure,
    pub(crate) on_toggle: ClickHandler,
    pub(crate) rows: Vec<AnyElement>,
}

impl ProjectSection {
    pub(crate) fn new(name: impl Into<SharedString>, theme: &UiTheme) -> Self {
        Self {
            name: name.into(),
            disclosure: Disclosure::Expanded,
            theme: *theme,
            on_toggle: None,
            on_new_thread: None,
            new_thread_menu: Vec::new(),
            actions: Vec::new(),
            show_more: None,
            archived: None,
            children: Vec::new(),
        }
    }

    /// Whether the section's threads show below its header.
    pub(crate) fn disclosure(mut self, disclosure: Disclosure) -> Self {
        self.disclosure = disclosure;
        self
    }

    pub(crate) fn on_toggle(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }

    pub(crate) fn on_new_thread(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_new_thread = Some(Box::new(handler));
        self
    }

    /// Makes the `+` open `items` in a menu tagged `project-<name>-new-thread-menu`, in place of
    /// running its [`on_new_thread`](Self::on_new_thread) handler.
    pub(crate) fn new_thread_menu(mut self, items: Vec<MenuItem>) -> Self {
        self.new_thread_menu = items;
        self
    }

    /// Offers `actions` in a menu tagged `project-<name>-actions-menu`.
    pub(crate) fn actions(mut self, actions: Vec<MenuItem>) -> Self {
        self.actions = actions;
        self
    }

    /// Adds a "Show `hidden` more" row below the section's threads that runs `handler` when
    /// clicked.
    pub(crate) fn on_show_more(
        mut self,
        hidden: usize,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.show_more = Some((hidden, Box::new(handler)));
        self
    }

    /// Lists `group` at the bottom of the section, below a header tagged
    /// `project-<name>-archived`.
    pub(crate) fn archived(mut self, group: ArchivedGroup) -> Self {
        self.archived = Some(group);
        self
    }
}

impl ParentElement for ProjectSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ProjectSection {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let selector = format!("project-{}", self.name);
        let hover_group = ListRow::hover_group(&selector);
        let expanded = self.disclosure == Disclosure::Expanded;
        let folder = if expanded { IconName::FolderOpen } else { IconName::Folder };
        let folder_selector = format!("{selector}-folder-{}", if expanded { "open" } else { "closed" });
        let title_selector = format!("{selector}-title");
        let toggle_selector = format!("{selector}-toggle");
        let toggle = div()
            .id(SharedString::from(toggle_selector.clone()))
            .debug_selector(move || toggle_selector)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when_some(self.on_toggle, |toggle, handler| toggle.on_click(handler))
            .child(slot().child(chevron(self.disclosure, &theme)))
            .child(slot().debug_selector(move || folder_selector).child(Icon::new(folder, colors.text_muted)))
            .child(div().min_w_0().truncate().debug_selector(move || title_selector).child(self.name.clone()));
        let new_thread_selector = format!("{selector}-new-thread");
        let new_thread =
            IconButton::new(new_thread_selector.clone(), IconName::Plus, format!("New thread in {}", self.name), theme);
        let new_thread = if self.new_thread_menu.is_empty() {
            new_thread.when_some(self.on_new_thread, |button, handler| button.on_click(handler)).into_any_element()
        } else {
            with_menu(&new_thread_selector, new_thread, MouseButton::Left, self.new_thread_menu, &theme)
                .into_any_element()
        };
        let actions_selector = format!("{selector}-actions");
        let actions = with_menu(
            &actions_selector,
            IconButton::new(actions_selector.clone(), IconName::Ellipsis, "Project actions", theme),
            MouseButton::Left,
            self.actions,
            &theme,
        );
        let header = div()
            .id(SharedString::from(selector.clone()))
            .group(hover_group.clone())
            .debug_selector({
                let selector = selector.clone();
                move || selector
            })
            .w_full()
            .h(rem(GROUP_HEIGHT))
            .pl_2()
            .pr_1()
            .flex()
            .items_center()
            .gap_1()
            .rounded(rem(RADIUS_CONTROL))
            .typography(Typography::Label)
            .text_color(colors.text_muted)
            .hover(move |header| header.bg(colors.surface_hover).text_color(colors.text))
            .child(toggle)
            .child(
                div()
                    .flex()
                    .items_center()
                    .opacity(0.0)
                    .group_hover(hover_group, |actions| actions.opacity(1.0))
                    .child(actions)
                    .child(new_thread),
            );
        let show_more = self.show_more.map(|(hidden, handler)| {
            let title_selector = format!("{selector}-show-more-title");
            ListRow::new(format!("{selector}-show-more"), theme)
                .depth(1)
                .on_click(handler)
                .child(slot())
                .child(slot())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .typography(Typography::Small)
                        .text_color(colors.text_muted)
                        .debug_selector(move || title_selector)
                        .child(format!("Show {hidden} more")),
                )
        });
        let archived = self.archived.map(|group| render_archived(&selector, group, &theme));
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .when(expanded, |section| section.children(self.children).children(show_more).children(archived))
    }
}

fn render_archived(selector: &str, group: ArchivedGroup, theme: &UiTheme) -> Div {
    let colors = theme.colors;
    let count = group.rows.len();
    let rows = if group.disclosure == Disclosure::Expanded { group.rows } else { Vec::new() };
    let icon_selector = format!("{selector}-archived-icon");
    let title_selector = format!("{selector}-archived-title");
    let header = ListRow::new(format!("{selector}-archived"), *theme)
        .depth(1)
        .on_click(group.on_toggle)
        .child(slot().child(chevron(group.disclosure, theme)))
        .child(slot().debug_selector(move || icon_selector).child(Icon::new(IconName::Archive, colors.text_muted)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .typography(Typography::Small)
                .text_color(colors.text_muted)
                .debug_selector(move || title_selector)
                .child("Archived"),
        )
        .child(div().typography(Typography::Caption).text_color(colors.text_muted).child(count.to_string()));
    div().w_full().flex().flex_col().child(header).children(rows)
}

fn slot() -> Div {
    div().flex_none().size(rem(STATUS_SLOT)).flex().items_center().justify_center()
}

fn chevron(disclosure: Disclosure, theme: &UiTheme) -> Icon {
    let name = match disclosure {
        Disclosure::Expanded => IconName::ChevronDown,
        Disclosure::Collapsed => IconName::ChevronRight,
    };
    Icon::new(name, theme.colors.text_muted).size(ICON_SIZE_SMALL)
}
