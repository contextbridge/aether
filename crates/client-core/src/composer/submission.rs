use crate::composer::attachment::PromptAttachment;

/// A prompt the user sent from the composer, with the files it carries.
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
