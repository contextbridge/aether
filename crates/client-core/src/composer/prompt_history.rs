#[derive(Debug, Default)]
pub struct PromptHistory {
    entries: Vec<String>,
    index: Option<usize>,
    draft: Option<String>,
}

impl PromptHistory {
    pub fn push(&mut self, prompt: &str) {
        self.reset();
        if prompt.trim().is_empty() {
            return;
        }
        self.entries.push(prompt.to_string());
        if self.entries.len() > MAX_HISTORY_ENTRIES {
            self.entries.remove(0);
        }
    }

    pub fn older(&mut self, draft: &str) -> Option<&str> {
        let index = match self.index {
            Some(0) => return None,
            Some(index) => index - 1,
            None => {
                self.draft = Some(draft.to_string());
                self.entries.len().checked_sub(1)?
            }
        };
        self.index = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    pub fn newer(&mut self) -> Option<String> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            self.index = Some(index + 1);
            return self.entries.get(index + 1).cloned();
        }
        self.index = None;
        Some(self.draft.take().unwrap_or_default())
    }

    pub fn is_recalling(&self) -> bool {
        self.index.is_some()
    }

    pub fn reset(&mut self) {
        self.index = None;
        self.draft = None;
    }
}

const MAX_HISTORY_ENTRIES: usize = 500;
