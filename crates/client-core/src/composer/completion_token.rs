use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionToken {
    pub trigger: Trigger,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Command,
    File,
}

impl CompletionToken {
    pub fn at(text: &str, cursor: usize) -> Option<Self> {
        let before = text.get(..cursor)?;
        let start =
            before.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(space, c)| space + c.len_utf8());
        let trigger = match before[start..].chars().next()? {
            '/' if start == 0 => Trigger::Command,
            '@' => Trigger::File,
            _ => return None,
        };
        Some(Self { trigger, range: start..cursor })
    }

    pub fn query<'a>(&self, text: &'a str) -> &'a str {
        &text[self.range.start + 1..self.range.end]
    }
}
