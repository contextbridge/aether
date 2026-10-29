use crate::composer::attachment::PromptAttachment;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Submission {
    pub text: String,
    pub mentions: Vec<PromptAttachment>,
    pub media: Vec<PromptAttachment>,
}

impl Submission {
    pub fn attachments(&self) -> Vec<PromptAttachment> {
        self.mentions.iter().chain(&self.media).cloned().collect()
    }

    /// Puts a prompt that never ran back on its own line after whatever the
    /// user typed since, so restoring it never discards the newer draft.
    pub fn restore_onto(&self, draft: &str) -> String {
        match (draft.is_empty(), self.text.is_empty()) {
            (true, _) => self.text.clone(),
            (false, true) => draft.to_owned(),
            (false, false) => format!("{draft}\n{}", self.text),
        }
    }
}

impl From<&str> for Submission {
    fn from(text: &str) -> Self {
        Self { text: text.to_owned(), ..Self::default() }
    }
}
