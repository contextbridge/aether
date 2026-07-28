/// Whether exported telemetry may include prompt, response, and tool content.
#[derive(Clone, Copy)]
pub(crate) enum ContentCapture {
    Redact,
    Capture,
}

/// Accumulates span content under a capture policy
pub(crate) enum ContentBuffer {
    Redacted,
    Recording(String),
}

impl ContentCapture {
    pub(crate) fn from_enabled(enabled: bool) -> Self {
        if enabled { Self::Capture } else { Self::Redact }
    }

    pub(crate) fn buffer(self) -> ContentBuffer {
        match self {
            Self::Redact => ContentBuffer::Redacted,
            Self::Capture => ContentBuffer::Recording(String::new()),
        }
    }

    /// `content` when capture is enabled, `None` otherwise.
    pub(crate) fn content(self, content: &str) -> Option<&str> {
        match self {
            Self::Redact => None,
            Self::Capture => Some(content),
        }
    }

    pub(crate) fn collect<T: Default>(self) -> Option<T> {
        match self {
            Self::Redact => None,
            Self::Capture => Some(T::default()),
        }
    }
}

impl ContentBuffer {
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
