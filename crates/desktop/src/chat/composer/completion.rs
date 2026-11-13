use crate::ui::tokens::COMPOSER_POPUP_HEIGHT;
use crate::ui::{GlassExt, PickerItem, picker_list};
use clankerdiff_rouge_shell::rouge::core::{Div, Length, Styled, div, view};
use clankerdiff_rouge_shell::rouge::reactive::Callback;
use clankerdiff_ui_core::{PickerOption, PickerState};
use client_core::composer::attachment::PromptAttachment;
use client_core::composer::completion_token::{CompletionToken, Trigger};
use client_core::composer::file_listing::RepoFiles;
use client_core::session_core::CommandEntry;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CompletionMenu {
    token: CompletionToken,
    items: Vec<CompletionItem>,
    status: Option<String>,
    highlighted: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompletionItem {
    Command(CommandEntry),
    File(PromptAttachment),
}

impl CompletionMenu {
    pub(crate) fn commands(token: CompletionToken, text: &str, commands: &[CommandEntry]) -> Self {
        let query = token.query(text);
        let indices = best_matches(query, commands.iter().map(|command| &command.name));
        let items = indices.into_iter().map(|index| CompletionItem::Command(commands[index].clone())).collect();
        Self { token, items, status: None, highlighted: 0 }
    }

    pub(crate) fn files(token: CompletionToken, text: &str, listing: Result<RepoFiles, String>) -> Self {
        let (items, status) = match listing {
            Ok(files) => {
                let matches = best_matches(token.query(text), files.paths.iter()).into_iter();
                (matches.map(|index| CompletionItem::File(files.attachment(&files.paths[index]))).collect(), None)
            }
            Err(status) => (Vec::new(), Some(status)),
        };
        Self { token, items, status, highlighted: 0 }
    }

    pub(crate) fn token(&self) -> &CompletionToken {
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

    pub(crate) fn render(&self, on_choose: Callback<(usize,)>) -> Div {
        let empty = self.status.clone().unwrap_or_else(|| {
            match self.token.trigger {
                Trigger::File => "No matching files",
                Trigger::Command => "No matching commands",
            }
            .to_owned()
        });
        let items: Vec<PickerItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(position, item)| match item {
                CompletionItem::Command(command) => {
                    PickerItem::new(format!("completion-item-{position}"), format!("/{}", command.name))
                        .detail(command.description.clone())
                }
                CompletionItem::File(file) => {
                    PickerItem::new(format!("completion-item-{position}"), file.display_name.clone())
                }
            })
            .collect();
        let highlighted = self.highlighted;
        view! {
            <div test_id="completion-menu" glass_sheet p=4.0>
                <picker_list id="completion" items highlighted empty
                             max_height={Length::design(COMPOSER_POPUP_HEIGHT)} on_choose/>
            </div>
        }
    }
}

pub(crate) fn active_token(text: &str, cursor: usize, mentions: &[Range<usize>]) -> Option<CompletionToken> {
    CompletionToken::at(text, cursor).filter(|token| {
        !mentions.iter().any(|mention| mention.start < token.range.end && token.range.start < mention.end)
    })
}

const MAX_ITEMS: usize = 50;

fn best_matches<T: AsRef<str>>(query: &str, keys: impl IntoIterator<Item = T>) -> Vec<usize> {
    let mut picker = PickerState::new(keys.into_iter().map(|key| PickerOption::new(key.as_ref())).collect());
    picker.set_query(query);
    picker.matches().iter().copied().take(MAX_ITEMS).collect()
}
