pub struct Task {
    prompt: String,
}

impl Task {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self { prompt: prompt.into() }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}
