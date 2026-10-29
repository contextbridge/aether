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
}

impl From<&str> for Submission {
    fn from(text: &str) -> Self {
        Self { text: text.to_owned(), ..Self::default() }
    }
}
