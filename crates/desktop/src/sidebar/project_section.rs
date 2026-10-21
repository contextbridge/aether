use super::ClickHandler;
use crate::registry::Disclosure;
use clankerdiff_gpui::ui::prelude::{ListRow, MutedText, UiTheme, icon_button};
use gpui::{
    AnyElement, App, ClickEvent, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Window, div,
    prelude::*,
};

/// A project's header, with a disclosure toggle and a `+` that starts a thread in it, above the
/// project's threads, a "Show more" row when some of its saved sessions are hidden, and its
/// archived threads in a group of their own.
#[derive(IntoElement)]
pub(crate) struct ProjectSection {
    name: SharedString,
    disclosure: Disclosure,
    theme: UiTheme,
    on_toggle: Option<ClickHandler>,
    on_new_thread: Option<ClickHandler>,
    on_show_more: Option<ClickHandler>,
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
            on_show_more: None,
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

    /// Adds a "Show more" row below the section's threads that runs `handler` when clicked.
    pub(crate) fn on_show_more(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_show_more = Some(Box::new(handler));
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
        let selector = format!("project-{}", self.name);
        let expanded = self.disclosure == Disclosure::Expanded;
        let disclosure = disclosure_glyph(self.disclosure);
        let toggle_selector = format!("{selector}-toggle");
        let toggle =
            ListRow::new(toggle_selector.clone(), self.theme).when_some(self.on_toggle, ListRow::on_click).child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().flex_none().text_color(self.theme.colors.text_muted).child(disclosure))
                    .child(div().min_w_0().truncate().child(self.name.clone())),
            );
        let new_thread =
            icon_button(format!("{selector}-new-thread"), "+", format!("New thread in {}", self.name), self.theme)
                .when_some(self.on_new_thread, |button, handler| button.on_click(handler));
        let show_more = self.on_show_more.map(|handler| {
            let show_more_selector = format!("{selector}-show-more");
            let row = ListRow::new(show_more_selector.clone(), self.theme)
                .on_click(handler)
                .child(div().pl_4().child(MutedText::new("Show more", self.theme)));
            div().debug_selector(move || show_more_selector).w_full().child(row)
        });
        let archived = self.archived.map(|group| {
            let archived_selector = format!("{selector}-archived");
            let label = format!("{} Archived ({})", disclosure_glyph(group.disclosure), group.rows.len());
            let row = ListRow::new(archived_selector.clone(), self.theme)
                .on_click(group.on_toggle)
                .child(div().pl_1().child(MutedText::new(label, self.theme)));
            let rows = if group.disclosure == Disclosure::Expanded { group.rows } else { Vec::new() };
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(div().debug_selector(move || archived_selector).w_full().child(row))
                .children(rows)
        });
        let header = div()
            .debug_selector(move || selector)
            .w_full()
            .flex()
            .items_center()
            .pr_1()
            .child(div().debug_selector(move || toggle_selector).flex_1().min_w_0().child(toggle))
            .child(new_thread);
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .when(expanded, |section| section.children(self.children).children(show_more).children(archived))
    }
}

fn disclosure_glyph(disclosure: Disclosure) -> &'static str {
    match disclosure {
        Disclosure::Expanded => "▾",
        Disclosure::Collapsed => "▸",
    }
}
