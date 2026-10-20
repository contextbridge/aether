pub(crate) struct Turns<T> {
    turns: Vec<(Speaker, Vec<T>)>,
    collecting_tool_results: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    User,
    Assistant,
}

impl<T> Turns<T> {
    pub(crate) fn user(&mut self, blocks: Vec<T>) {
        self.push(Speaker::User, blocks);
    }

    pub(crate) fn assistant(&mut self, blocks: Vec<T>) {
        if !blocks.is_empty() {
            self.push(Speaker::Assistant, blocks);
        }
    }

    pub(crate) fn tool_result(&mut self, block: T) {
        match self.turns.last_mut() {
            Some((Speaker::User, blocks)) if self.collecting_tool_results => blocks.push(block),
            _ => self.turns.push((Speaker::User, vec![block])),
        }
        self.collecting_tool_results = true;
    }

    pub(crate) fn last_turn_mut(&mut self) -> Option<&mut Vec<T>> {
        self.turns.last_mut().map(|(_, blocks)| blocks)
    }

    pub(crate) fn into_turns(self) -> impl Iterator<Item = (Speaker, Vec<T>)> {
        self.turns.into_iter()
    }

    fn push(&mut self, speaker: Speaker, blocks: Vec<T>) {
        self.turns.push((speaker, blocks));
        self.collecting_tool_results = false;
    }
}

impl<T> Default for Turns<T> {
    fn default() -> Self {
        Self { turns: Vec::new(), collecting_tool_results: false }
    }
}
