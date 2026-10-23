use super::composer::Composer;
use crate::ui::tokens::rem;
use crate::ui::{PickerItem, PickerList, SurfaceExt, UiTheme, fuzzy_filter};
use client_core::command_entry::CommandEntry;
use client_core::composer::attachment::PromptAttachment;
use gpui::{AnyElement, Context, div, prelude::*};
use gpui_base::input::InlineTokenSpan;
use std::ops::Range;

/// The `/` commands or `@` files that complete what the user is typing, with one highlighted.
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
    pub(crate) fn commands(token: ActiveToken, text: &str, commands: &[CommandEntry]) -> Self {
        let query = token.query(text);
        let indices = best_matches(query, commands.iter().map(|command| &command.name));
        let items = indices.into_iter().map(|index| CompletionItem::Command(commands[index].clone())).collect();
        Self { token, items, loading: false, highlighted: 0 }
    }

    /// Offers the indexed `files` whose paths match the token, or shows that indexing is still
    /// running while there are none yet.
    pub(crate) fn files(token: ActiveToken, text: &str, files: Option<&[PromptAttachment]>) -> Self {
        let query = token.query(text);
        let items = files.map_or_else(Vec::new, |files| {
            best_matches(query, files.iter().map(|file| &file.display_name))
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

    pub(crate) fn step(&mut self, by: isize) {
        let last = self.items.len().saturating_sub(1);
        self.highlighted = self.highlighted.saturating_add_signed(by).min(last);
    }

    pub(crate) fn render(&self, theme: &UiTheme, cx: &mut Context<Composer>) -> AnyElement {
        let empty = match (self.token.trigger, self.loading) {
            (Trigger::File, true) => "Indexing files…",
            (Trigger::File, false) => "No matching files",
            (Trigger::Command, _) => "No matching commands",
        };
        let items = self.items.iter().enumerate().map(|(position, item)| match item {
            CompletionItem::Command(command) => {
                PickerItem::new(format!("item-{position}"), format!("/{}", command.name))
                    .detail(command.description.clone())
            }
            CompletionItem::File(file) => PickerItem::new(format!("item-{position}"), file.display_name.clone()),
        });
        div()
            .debug_selector(|| "completion-menu".to_owned())
            .surface(*theme)
            .shadow_lg()
            .child(
                PickerList::new("completion", *theme)
                    .items(items)
                    .highlighted(Some(self.highlighted))
                    .empty(empty)
                    .max_height(rem(MENU_HEIGHT))
                    .on_choose(cx.listener(|composer, position: &usize, window, cx| {
                        composer.accept_completion(*position, window, cx);
                    })),
            )
            .into_any_element()
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

fn best_matches<T: AsRef<str>>(query: &str, keys: impl IntoIterator<Item = T>) -> Vec<usize> {
    let mut indices = fuzzy_filter(query, keys);
    indices.truncate(MAX_ITEMS);
    indices
}
