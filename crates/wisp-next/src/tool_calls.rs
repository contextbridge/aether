use crate::diff::DiffPreview;
use agent_client_protocol::schema as acp;
use std::path::Path;

/// Minimal tool-call tracker: id, display title, and lifecycle status.
#[derive(Debug, Default)]
pub struct ToolCallLog {
    entries: Vec<ToolCallEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallEntry {
    pub id: String,
    pub title: String,
    pub status: ToolStatus,
    pub diff: Option<DiffPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Success,
    Error(String),
}

impl ToolCallLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_tool_call(&mut self, tool_call: &acp::ToolCall) {
        let id = tool_call.tool_call_id.0.to_string();

        if let Some(entry) = self.entry_mut(&id) {
            update_title(&mut entry.title, &tool_call.title);
            entry.status = ToolStatus::Running;
        } else {
            self.entries.push(ToolCallEntry {
                id,
                title: tool_call.title.clone(),
                status: ToolStatus::Running,
                diff: None,
            });
        }
    }

    pub fn on_tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let Some(entry) = self.entry_mut(&update.tool_call_id.0) else {
            return;
        };

        if let Some(title) = &update.fields.title {
            update_title(&mut entry.title, title);
        }
        if let Some(content) = &update.fields.content {
            for item in content {
                if let acp::ToolCallContent::Diff(diff) = item {
                    let language =
                        Path::new(&diff.path).extension().and_then(|extension| extension.to_str()).unwrap_or_default();
                    entry.diff = Some(DiffPreview::compute(
                        diff.old_text.as_deref().unwrap_or_default(),
                        &diff.new_text,
                        language,
                    ));
                }
            }
        }
        if let Some(status) = update.fields.status {
            match status {
                acp::ToolCallStatus::Completed => entry.status = ToolStatus::Success,
                acp::ToolCallStatus::Failed => entry.status = ToolStatus::Error("failed".to_string()),
                acp::ToolCallStatus::InProgress | acp::ToolCallStatus::Pending => entry.status = ToolStatus::Running,
                _ => {}
            }
        }
    }

    /// Mark every still-running tool call with the given terminal status.
    /// Called when the prompt turn ends (done, cancelled, or failed).
    pub fn finalize_running(&mut self, terminal_status: &ToolStatus) {
        for entry in &mut self.entries {
            if entry.status == ToolStatus::Running {
                entry.status = terminal_status.clone();
            }
        }
    }

    pub fn has_tool(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.entry(id).is_some_and(|entry| entry.status == ToolStatus::Running)
    }

    pub fn any_running(&self) -> bool {
        self.entries.iter().any(|entry| entry.status == ToolStatus::Running)
    }

    pub fn entry(&self, id: &str) -> Option<&ToolCallEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<ToolCallEntry> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        Some(self.entries.remove(index))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut ToolCallEntry> {
        self.entries.iter_mut().find(|entry| entry.id == id)
    }
}

fn update_title(current: &mut String, new_title: &str) {
    if !new_title.is_empty() {
        current.clear();
        current.push_str(new_title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_update(id: &str, status: acp::ToolCallStatus) -> acp::ToolCallUpdate {
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new().status(status))
    }

    #[test]
    fn tool_call_registers_as_running() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        assert!(log.is_running("tool-1"));
        assert_eq!(log.entry("tool-1").unwrap().title, "Read file");
    }

    #[test]
    fn completed_update_marks_success() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));
        log.on_tool_call_update(&status_update("tool-1", acp::ToolCallStatus::Completed));

        assert_eq!(log.entry("tool-1").unwrap().status, ToolStatus::Success);
        assert!(!log.any_running());
    }

    #[test]
    fn failed_update_marks_error() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));
        log.on_tool_call_update(&status_update("tool-1", acp::ToolCallStatus::Failed));

        assert_eq!(log.entry("tool-1").unwrap().status, ToolStatus::Error("failed".to_string()));
    }

    #[test]
    fn update_for_unknown_tool_is_ignored() {
        let mut log = ToolCallLog::new();
        log.on_tool_call_update(&status_update("ghost", acp::ToolCallStatus::Completed));

        assert!(!log.has_tool("ghost"));
    }

    #[test]
    fn update_replaces_title_when_provided() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read"));
        log.on_tool_call_update(&acp::ToolCallUpdate::new(
            "tool-1".to_string(),
            acp::ToolCallUpdateFields::new().title("Read main.rs"),
        ));

        assert_eq!(log.entry("tool-1").unwrap().title, "Read main.rs");
    }

    #[test]
    fn finalize_running_only_touches_running_entries() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("done".to_string(), "Done tool"));
        log.on_tool_call_update(&status_update("done", acp::ToolCallStatus::Completed));
        log.on_tool_call(&acp::ToolCall::new("stuck".to_string(), "Stuck tool"));

        log.finalize_running(&ToolStatus::Error("cancelled".to_string()));

        assert_eq!(log.entry("done").unwrap().status, ToolStatus::Success);
        assert_eq!(log.entry("stuck").unwrap().status, ToolStatus::Error("cancelled".to_string()));
    }

    #[test]
    fn remove_returns_entry_and_forgets_it() {
        let mut log = ToolCallLog::new();
        log.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));

        let removed = log.remove("tool-1").unwrap();

        assert_eq!(removed.title, "Read file");
        assert!(!log.has_tool("tool-1"));
        assert!(log.remove("tool-1").is_none());
    }
}
