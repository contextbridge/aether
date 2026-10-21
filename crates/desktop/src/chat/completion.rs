use super::composer::Composer;
use crate::fuzzy;
use clankerdiff_gpui::ui::prelude::{ListRow, MutedText, SelectionState, UiTheme};
use client_core::command_entry::CommandEntry;
use client_core::composer::attachment::PromptAttachment;
use gpui::{AnyElement, ClickEvent, Context, ElementId, div, prelude::*, px};
use gpui_base::input::InlineTokenSpan;
use std::ops::Range;

/// The `/` commands or `@` files that complete what the user is typing, with one highlighted.
///
/// It is tagged `completion-menu`, and each entry `completion-item-<n>`.
pub(crate) struct CompletionMenu {
    token: ActiveToken,
    items: Vec<CompletionItem>,
    /// Whether the files are still being indexed, so there is nothing to offer yet.
    loading: bool,
    highlighted: usize,
}

/// What the user is completing, and where it sits in the composer's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveToken {
    pub(crate) trigger: Trigger,
    /// From the trigger character to the cursor.
    pub(crate) range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// A `/` command, which only the start of the prompt can name.
    Command,
    /// An `@` file mention, which starts a word anywhere.
    File,
}

#[derive(Debug, Clone)]
pub(crate) enum CompletionItem {
    Command(CommandEntry),
    File(PromptAttachment),
}

impl CompletionMenu {
    /// Offers the `commands` whose names match the token.
    pub(crate) fn commands(token: ActiveToken, text: &str, commands: &[CommandEntry]) -> Self {
        let query = token.query(text);
        let indices = fuzzy_or_all(query, commands.iter().map(|command| command.name.clone()));
        let items = indices.into_iter().map(|index| CompletionItem::Command(commands[index].clone())).collect();
        Self { token, items, loading: false, highlighted: 0 }
    }

    /// Offers the indexed `files` whose paths match the token, or shows that indexing is still
    /// running while there are none yet.
    pub(crate) fn files(token: ActiveToken, text: &str, files: Option<&[PromptAttachment]>) -> Self {
        let query = token.query(text);
        let items = files.map_or_else(Vec::new, |files| {
            fuzzy_or_all(query, files.iter().map(|file| file.display_name.clone()))
                .into_iter()
                .map(|index| CompletionItem::File(files[index].clone()))
                .collect()
        });
        Self { token, items, loading: files.is_none(), highlighted: 0 }
    }

    pub(crate) fn token(&self) -> &ActiveToken {
        &self.token
    }

    pub(crate) fn item(&self, position: usize) -> Option<&CompletionItem> {
        self.items.get(position)
    }

    pub(crate) fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// Moves the highlight `by` entries, stopping at either end of the list.
    pub(crate) fn step(&mut self, by: isize) {
        let last = self.items.len().saturating_sub(1);
        self.highlighted = self.highlighted.saturating_add_signed(by).min(last);
    }

    pub(crate) fn render(&self, theme: &UiTheme, cx: &mut Context<Composer>) -> AnyElement {
        let mut menu = div()
            .id("completion-menu")
            .debug_selector(|| "completion-menu".to_owned())
            .max_h(px(MENU_HEIGHT))
            .overflow_y_scroll()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.surface);
        if self.items.is_empty() {
            let message = match (self.token.trigger, self.loading) {
                (Trigger::File, true) => "Indexing files…",
                (Trigger::File, false) => "No matching files",
                (Trigger::Command, _) => "No matching commands",
            };
            return menu.child(div().px_3().py_1().child(MutedText::new(message, *theme))).into_any_element();
        }
        for (position, item) in self.items.iter().enumerate() {
            let state = if position == self.highlighted { SelectionState::Selected } else { SelectionState::None };
            let (label, detail) = match item {
                CompletionItem::Command(command) => (format!("/{}", command.name), command.description.clone()),
                CompletionItem::File(file) => (file.display_name.clone(), String::new()),
            };
            let row = ListRow::new(ElementId::NamedInteger("completion-row".into(), position as u64), *theme)
                .state(state)
                .on_click(cx.listener(move |composer, _: &ClickEvent, window, cx| {
                    composer.accept_completion(position, window, cx);
                }))
                .child(div().flex_none().child(label))
                .child(div().flex_1().min_w_0().truncate().text_sm().child(MutedText::new(detail, *theme)));
            menu = menu.child(div().debug_selector(move || format!("completion-item-{position}")).child(row));
        }
        menu.into_any_element()
    }
}

impl ActiveToken {
    /// The token the cursor ends, if it is one the composer completes: a `/` that starts the
    /// prompt, or an `@` that starts a word, followed by no whitespace. Mentions already chosen
    /// are not completed again.
    pub(crate) fn at(text: &str, cursor: usize, mentions: &[InlineTokenSpan]) -> Option<Self> {
        let before = text.get(..cursor)?;
        let start =
            before.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(space, c)| space + c.len_utf8());
        let trigger = match before[start..].chars().next()? {
            '/' if start == 0 => Trigger::Command,
            '@' => Trigger::File,
            _ => return None,
        };
        let range = start..cursor;
        let chosen =
            mentions.iter().any(|mention| mention.range().start < range.end && range.start < mention.range().end);
        (!chosen).then_some(Self { trigger, range })
    }

    /// What follows the trigger character.
    fn query<'a>(&self, text: &'a str) -> &'a str {
        &text[self.range.start + 1..self.range.end]
    }
}

/// How many matches a menu lists, best first.
const MAX_ITEMS: usize = 50;
const MENU_HEIGHT: f32 = 240.0;

fn fuzzy_or_all(query: &str, keys: impl ExactSizeIterator<Item = String>) -> Vec<usize> {
    let mut indices = if query.is_empty() { (0..keys.len()).collect() } else { fuzzy::filter(query, keys) };
    indices.truncate(MAX_ITEMS);
    indices
}
