use std::fmt::Display;
use std::{collections::BTreeMap, iter::from_fn};

use crate::{LlmResponse, ToolCallRequest};

/// Collects streaming tool call deltas into complete tool calls.
///
/// Generic over the index type `I` since different providers use
/// different integer types (e.g. `u32` for `OpenAI`, `i32` for compatible APIs).
pub(crate) struct ToolCallCollector<T> {
    active_tool_calls: BTreeMap<T, (String, String, String)>,
}

impl<T: Eq + Ord + Copy + Display> ToolCallCollector<T> {
    pub fn new() -> Self {
        Self { active_tool_calls: BTreeMap::new() }
    }

    /// Process a tool call delta with extracted fields.
    ///
    /// Callers should destructure their provider-specific delta type
    /// and pass the relevant fields here.
    pub fn handle_delta(
        &mut self,
        index: T,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    ) -> Vec<LlmResponse> {
        let mut responses = Vec::new();

        if let Some(name) = name {
            let id = id.unwrap_or_else(|| format!("tool_call_{index}"));
            self.start_tool_call(index, id.clone(), name.clone());
            responses.push(LlmResponse::ToolRequestStart { id, name });
        }

        if let Some(arguments) = arguments
            && !arguments.is_empty()
            && let Some(id) = self.add_arguments(index, &arguments)
        {
            responses.push(LlmResponse::ToolRequestArg { id, chunk: arguments });
        }

        responses
    }

    /// Complete the tool call at a specific index and return it, if present.
    #[allow(dead_code)]
    pub fn complete_one(&mut self, index: T) -> Option<ToolCallRequest> {
        self.active_tool_calls.remove(&index).map(to_request)
    }

    /// Complete all pending tool calls and return them.
    pub fn complete_all(&mut self) -> Vec<ToolCallRequest> {
        self.active_tool_calls
            .pop_first()
            .into_iter()
            .chain(from_fn(|| self.active_tool_calls.pop_first()))
            .map(|(_, fields)| to_request(fields))
            .collect()
    }

    fn start_tool_call(&mut self, index: T, id: String, name: String) {
        self.active_tool_calls.insert(index, (id, name, String::new()));
    }

    fn add_arguments(&mut self, index: T, arguments: &str) -> Option<String> {
        if let Some((id, _, accumulated_args)) = self.active_tool_calls.get_mut(&index) {
            accumulated_args.push_str(arguments);
            return Some(id.clone());
        }
        None
    }
}

fn to_request((id, name, arguments): (String, String, String)) -> ToolCallRequest {
    ToolCallRequest { id, name, arguments }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_tool_call() {
        let mut collector = ToolCallCollector::<u32>::new();

        let responses = collector.handle_delta(0, Some("call_1".to_string()), Some("my_tool".to_string()), None);

        assert_eq!(responses.len(), 1);
        assert!(
            matches!(&responses[0], LlmResponse::ToolRequestStart { id, name } if id == "call_1" && name == "my_tool")
        );

        let responses = collector.handle_delta(0, None, None, Some("{\"key\":".to_string()));
        assert_eq!(responses.len(), 1);
        assert!(matches!(&responses[0], LlmResponse::ToolRequestArg { chunk, .. } if chunk == "{\"key\":"));

        let responses = collector.handle_delta(0, None, None, Some("\"val\"}".to_string()));
        assert_eq!(responses.len(), 1);

        let completed = collector.complete_all();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].id, "call_1");
        assert_eq!(completed[0].name, "my_tool");
        assert_eq!(completed[0].arguments, "{\"key\":\"val\"}");
    }

    #[test]
    fn test_multiple_tool_calls_deterministic_order() {
        let mut collector = ToolCallCollector::<i32>::new();

        collector.handle_delta(0, Some("a".into()), Some("tool_a".into()), Some("{}".into()));
        collector.handle_delta(1, Some("b".into()), Some("tool_b".into()), Some("{}".into()));

        let completed = collector.complete_all();
        assert_eq!(completed.len(), 2);
        // BTreeMap guarantees ordered by index
        assert_eq!(completed[0].id, "a");
        assert_eq!(completed[1].id, "b");
    }

    #[test]
    fn test_empty_arguments_ignored() {
        let mut collector = ToolCallCollector::<u32>::new();

        collector.handle_delta(0, Some("id".into()), Some("tool".into()), None);
        let responses = collector.handle_delta(0, None, None, Some(String::new()));
        assert!(responses.is_empty());
    }

    #[test]
    fn test_complete_one_returns_specific_tool_call() {
        let mut collector = ToolCallCollector::<u32>::new();

        collector.handle_delta(0, Some("a".into()), Some("tool_a".into()), Some("{}".into()));
        collector.handle_delta(1, Some("b".into()), Some("tool_b".into()), Some("{}".into()));

        let completed = collector.complete_one(0);
        assert!(completed.is_some());
        assert_eq!(completed.unwrap().id, "a");

        // Index 1 should still be present
        let remaining = collector.complete_all();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "b");
    }

    #[test]
    fn test_complete_one_nonexistent_returns_none() {
        let mut collector = ToolCallCollector::<u32>::new();
        assert!(collector.complete_one(99).is_none());
    }

    #[test]
    fn test_complete_all_drains() {
        let mut collector = ToolCallCollector::<u32>::new();

        collector.handle_delta(0, Some("id".into()), Some("tool".into()), Some("{}".into()));
        let first = collector.complete_all();
        assert_eq!(first.len(), 1);

        let second = collector.complete_all();
        assert!(second.is_empty());
    }
}
