/// Content-capture settings. Each field maps 1:1 to a
/// `gen_ai.*` OTEL content attribute; all default to off.
#[allow(clippy::struct_excessive_bools)] // independent opt-ins, not a state machine
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentCaptureSettings {
    pub system_instructions: bool,
    pub input_messages: bool,
    pub output_messages: bool,
    pub tool_definitions: bool,
    pub tool_calls: bool,
}

/// Accumulates span content under a capture policy.
pub(crate) enum ContentBuffer {
    Redacted,
    Recording(String),
}

impl ContentBuffer {
    pub(crate) fn new(capture: bool) -> Self {
        if capture { Self::Recording(String::new()) } else { Self::Redacted }
    }

    pub(crate) fn push(&mut self, chunk: &str) {
        if let Self::Recording(text) = self {
            text.push_str(chunk);
        }
    }

    pub(crate) fn set(&mut self, content: &str) {
        if let Self::Recording(text) = self {
            content.clone_into(text);
        }
    }

    /// The recorded content; `None` when redacted or nothing was recorded.
    pub(crate) fn get(&self) -> Option<&str> {
        match self {
            Self::Recording(text) if !text.is_empty() => Some(text),
            _ => None,
        }
    }
}
