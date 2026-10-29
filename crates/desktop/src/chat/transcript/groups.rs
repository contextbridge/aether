use acp_utils::conversation::{ConversationContent, ConversationItem, ToolCall, ToolStatus};
use std::collections::HashSet;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) rows: Range<usize>,
    pub(crate) kind: GroupKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GroupKind {
    Turn,
    Run,
}

pub(crate) fn groups(items: &[ConversationItem]) -> Vec<Group> {
    let turns = turns(items);
    let latest = turns.len().saturating_sub(1);
    turns
        .into_iter()
        .enumerate()
        .flat_map(|(index, turn)| {
            let work = (index < latest).then(|| turn_work(items, turn.clone())).flatten();
            match work {
                Some(rows) => vec![Group { rows, kind: GroupKind::Turn }],
                None => runs(items, turn).into_iter().map(|rows| Group { rows, kind: GroupKind::Run }).collect(),
            }
        })
        .collect()
}

pub(super) struct Work {
    pub(super) tools: usize,
    pub(super) files: usize,
    pub(super) failed: usize,
}

impl Work {
    pub(super) fn of(items: &[ConversationItem]) -> Self {
        let tools: Vec<&ToolCall> = items
            .iter()
            .filter_map(|item| match item.content() {
                ConversationContent::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        Self {
            tools: tools.len(),
            files: tools.iter().flat_map(|tool| tool.changed_paths()).collect::<HashSet<_>>().len(),
            failed: tools.iter().filter(|tool| tool.status == ToolStatus::Failed).count(),
        }
    }

    pub(super) fn details(&self) -> impl Iterator<Item = String> + use<> {
        let files = match self.files {
            0 => None,
            1 => Some("1 file changed".to_owned()),
            files => Some(format!("{files} files changed")),
        };
        let failed = (self.failed > 0).then(|| format!("{} failed", self.failed));
        files.into_iter().chain(failed)
    }
}

fn turns(items: &[ConversationItem]) -> Vec<Range<usize>> {
    let prompts = items.iter().enumerate().filter(|(_, item)| matches!(item.content(), ConversationContent::User(_)));
    let mut turns = Vec::new();
    let mut start = 0;
    for (index, _) in prompts {
        turns.push(start..index);
        start = index + 1;
    }
    turns.push(start..items.len());
    turns
}

fn turn_work(items: &[ConversationItem], turn: Range<usize>) -> Option<Range<usize>> {
    let replies = items[turn.clone()]
        .iter()
        .rev()
        .take_while(|item| matches!(item.content(), ConversationContent::Assistant(_)))
        .count();
    let work = turn.start..turn.end - replies;
    let worked = items[work.clone()]
        .iter()
        .any(|item| matches!(item.content(), ConversationContent::Tool(_) | ConversationContent::Thought(_)));
    let finished = items[work.clone()].iter().all(is_settled);
    (replies > 0 && worked && finished).then_some(work)
}

fn runs(items: &[ConversationItem], turn: Range<usize>) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut start = turn.start;
    while start < turn.end {
        let end = start + items[start..turn.end].iter().take_while(|item| is_finished_work(item)).count();
        let tools = items[start..end].iter().filter(|item| matches!(item.content(), ConversationContent::Tool(_)));
        if tools.count() >= GROUP_MIN_TOOLS {
            runs.push(start..end);
        }
        start = end + 1;
    }
    runs
}

fn is_finished_work(item: &ConversationItem) -> bool {
    matches!(item.content(), ConversationContent::Tool(_) | ConversationContent::Thought(_)) && is_settled(item)
}

fn is_settled(item: &ConversationItem) -> bool {
    match item.content() {
        ConversationContent::Tool(tool) => !tool.is_running(),
        _ => !item.is_open(),
    }
}

const GROUP_MIN_TOOLS: usize = 3;
