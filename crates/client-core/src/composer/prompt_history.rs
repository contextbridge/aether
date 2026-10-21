/// The prompts submitted this session, and where recall has walked back to.
///
/// Navigation stashes the draft it replaced, so stepping past the newest entry
/// puts the user back where they started.
#[derive(Debug, Default)]
pub struct PromptHistory {
    entries: Vec<String>,
    index: Option<usize>,
    draft: Option<String>,
}

impl PromptHistory {
    pub fn push(&mut self, prompt: &str) {
        if prompt.trim().is_empty() {
            return;
        }
        self.entries.push(prompt.to_string());
        if self.entries.len() > MAX_HISTORY_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// The previous prompt, stashing `draft` the first time recall starts.
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

    /// The next prompt, or the stashed draft once recall walks past the newest.
    pub fn newer(&mut self) -> Option<String> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            self.index = Some(index + 1);
            return self.entries.get(index + 1).cloned();
        }
        self.index = None;
        Some(self.draft.take().unwrap_or_default())
    }

    /// Ends navigation, so the recalled prompt becomes the user's own draft.
    pub fn reset(&mut self) {
        self.index = None;
        self.draft = None;
    }
}

/// Prompts kept for recall. Older ones are dropped rather than growing forever.
const MAX_HISTORY_ENTRIES: usize = 500;
