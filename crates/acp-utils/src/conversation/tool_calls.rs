use super::{Conversation, ConversationContent, ConversationItem};
use crate::client::AcpEvent;
use crate::content::first_line;
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Serialize, Serializer};
use std::borrow::Cow;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct SubAgent {
    announcement: acp::SubagentUpdate,
    conversation: Conversation,
}

/// A tool call as the merge of every update the agent sent for it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub status: ToolStatus,
    pub sub_agents: Vec<SubAgent>,
    #[serde(rename = "toolCall")]
    protocol: Box<acp::ToolCallUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Success,
    Cancelled,
    Failed,
}

impl ToolCall {
    pub fn id(&self) -> &str {
        &self.protocol.tool_call_id.0
    }

    pub fn title(&self) -> &str {
        self.protocol.title.value().map_or("", String::as_str)
    }

    pub fn raw_input(&self) -> String {
        self.protocol.raw_input.value().map_or_else(String::new, raw_input_text)
    }

    pub fn display_value(&self) -> Option<&str> {
        self.meta_str("display_value")
    }

    pub fn content(&self) -> &[acp::ToolCallContent] {
        self.protocol.content.value().map_or(&[], Vec::as_slice)
    }

    pub fn diffs(&self) -> impl Iterator<Item = &acp::Diff> {
        self.content().iter().filter_map(|content| match content {
            acp::ToolCallContent::Diff(diff) => Some(diff),
            _ => None,
        })
    }

    pub fn locations(&self) -> &[acp::ToolCallLocation] {
        self.protocol.locations.value().map_or(&[], Vec::as_slice)
    }

    pub fn headline(&self) -> Option<&str> {
        first_line(self.title())
    }

    pub fn changed_paths(&self) -> Vec<&Path> {
        let mut paths: Vec<&Path> = Vec::new();
        for path in self.diffs().flat_map(|diff| &diff.changes).filter_map(change_path) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    pub fn bash_command(&self) -> Option<&str> {
        if self.kind() != ToolKind::Bash {
            return None;
        }
        self.protocol.raw_input.value()?.get("command")?.as_str()
    }

    pub(super) fn from_update(update: &acp::ToolCallUpdate) -> Self {
        let mut tool = Self { status: ToolStatus::Running, sub_agents: Vec::new(), protocol: Box::new(update.clone()) };
        tool.refresh_status();
        tool
    }

    pub(super) fn apply_update(&mut self, update: &acp::ToolCallUpdate) -> bool {
        let (protocol, status) = (self.protocol.clone(), self.status);
        self.protocol.apply_update(update.clone());
        self.refresh_status();
        self.protocol != protocol || self.status != status
    }

    pub(super) fn append_content(&mut self, content: acp::ToolCallContent) {
        match &mut self.protocol.content {
            MaybeUndefined::Value(items) => items.push(content),
            value => *value = MaybeUndefined::Value(vec![content]),
        }
    }

    pub(super) fn announce_sub_agent(&mut self, update: &acp::SubagentUpdate) -> bool {
        if let Some(agent) = self.sub_agents.iter_mut().find(|agent| *agent.session_id() == update.session_id) {
            return agent.announce(update);
        }
        self.sub_agents.push(SubAgent::new(update));
        true
    }

    pub(super) fn tracks(&self, session_id: &acp::SessionId) -> bool {
        self.sub_agents.iter().any(|agent| agent.tracks(session_id))
    }

    pub(super) fn apply_sub_agent_event(&mut self, session_id: &acp::SessionId, event: &AcpEvent) -> bool {
        self.sub_agents.iter_mut().find(|agent| agent.tracks(session_id)).is_some_and(|agent| agent.apply_event(event))
    }

    pub(super) fn unconfirm_sub_agents(&mut self) -> bool {
        let mut changed = false;
        for agent in &mut self.sub_agents {
            changed |= agent.unconfirm();
        }
        changed
    }

    pub(super) fn finalize(&mut self, status: ToolStatus) {
        if self.status == ToolStatus::Running {
            self.status = status;
        }
        for agent in &mut self.sub_agents {
            agent.finalize(status);
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == ToolStatus::Running || self.sub_agents.iter().any(SubAgent::is_running)
    }

    pub(super) fn is_settled(&self) -> bool {
        self.status != ToolStatus::Running && self.sub_agents.iter().all(SubAgent::is_settled)
    }

    pub(super) fn runs_in_background(&self) -> bool {
        self.status != ToolStatus::Running && !self.is_settled()
    }

    pub(super) fn rendering_final(&self) -> bool {
        self.is_settled() && (self.kind() != ToolKind::SpawnSubagent || !self.sub_agents.is_empty())
    }

    fn kind(&self) -> ToolKind {
        tool_kind(self.protocol.name.value().map_or_else(|| self.title(), String::as_str))
    }

    fn refresh_status(&mut self) {
        self.status = match self.protocol.status.value() {
            Some(acp::ToolCallStatus::Completed) => ToolStatus::Success,
            Some(acp::ToolCallStatus::Failed) => ToolStatus::Failed,
            Some(acp::ToolCallStatus::Cancelled) => ToolStatus::Cancelled,
            _ => ToolStatus::Running,
        };
    }

    fn meta_str(&self, key: &str) -> Option<&str> {
        self.protocol.meta.value().and_then(|meta| meta.get(key)).and_then(serde_json::Value::as_str)
    }
}

pub fn diff_change_label(change: &acp::DiffChange) -> String {
    match &change.operation {
        acp::DiffChangeOperation::Add(change) => format!("A {}", change.path.0.display()),
        acp::DiffChangeOperation::Delete(change) => format!("D {}", change.path.0.display()),
        acp::DiffChangeOperation::Modify(change) => format!("M {}", change.path.0.display()),
        acp::DiffChangeOperation::Move(change) => {
            format!("R {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        acp::DiffChangeOperation::Copy(change) => {
            format!("C {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        _ => "Unknown file change".into(),
    }
}

impl SubAgent {
    pub fn session_id(&self) -> &acp::SessionId {
        &self.announcement.session_id
    }

    pub fn title(&self) -> Option<&str> {
        self.announcement.title.value().map(String::as_str)
    }

    pub fn status(&self) -> Option<ToolStatus> {
        self.conversation.work.status()
    }

    pub fn items(&self) -> &[ConversationItem] {
        self.conversation.items()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.items().iter().filter_map(|item| match item.content() {
            ConversationContent::Tool(tool_call) => Some(tool_call),
            _ => None,
        })
    }

    fn new(update: &acp::SubagentUpdate) -> Self {
        let mut agent = Self {
            announcement: acp::SubagentUpdate::new(update.session_id.clone()),
            conversation: Conversation::new(update.session_id.clone()),
        };
        agent.announce(update);
        agent
    }

    fn announce(&mut self, update: &acp::SubagentUpdate) -> bool {
        let previous = self.announcement.clone();
        patch(&mut self.announcement.title, &update.title);
        patch(&mut self.announcement.description, &update.description);
        patch(&mut self.announcement.capabilities, &update.capabilities);
        acp::apply_meta_update(&mut self.announcement.meta, update.meta.clone());
        let reported = self.revises(|conversation| match &update.state {
            MaybeUndefined::Value(state) => {
                conversation.apply_update(&acp::SessionUpdate::StateUpdate(state.clone()));
            }
            MaybeUndefined::Null => conversation.forget_work(),
            MaybeUndefined::Undefined => {}
        });
        self.announcement != previous || reported
    }

    fn tracks(&self, session_id: &acp::SessionId) -> bool {
        self.conversation.tracks(session_id)
    }

    fn apply_event(&mut self, event: &AcpEvent) -> bool {
        self.revises(|conversation| {
            conversation.apply_event(event);
        })
    }

    fn unconfirm(&mut self) -> bool {
        self.revises(Conversation::unconfirm)
    }

    fn finalize(&mut self, status: ToolStatus) {
        self.conversation.seal(status);
    }

    fn is_running(&self) -> bool {
        self.status() == Some(ToolStatus::Running) || self.conversation.any_running()
    }

    fn is_settled(&self) -> bool {
        self.conversation.is_settled()
    }

    fn revises(&mut self, apply: impl FnOnce(&mut Conversation)) -> bool {
        let revision = self.conversation.revision();
        apply(&mut self.conversation);
        self.conversation.revision() != revision
    }
}

impl Serialize for SubAgent {
    fn serialize<T: Serializer>(&self, serializer: T) -> Result<T::Ok, T::Error> {
        SubAgentJson { subagent: &self.announcement, status: self.status(), items: self.items() }.serialize(serializer)
    }
}

impl JsonSchema for SubAgent {
    fn schema_name() -> Cow<'static, str> {
        "SubAgent".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        SubAgentJson::json_schema(generator)
    }
}

impl From<Option<&acp::StopReason>> for ToolStatus {
    fn from(stop_reason: Option<&acp::StopReason>) -> Self {
        match stop_reason {
            Some(acp::StopReason::Cancelled) => Self::Cancelled,
            Some(acp::StopReason::Error(_) | acp::StopReason::Refusal) => Self::Failed,
            _ => Self::Success,
        }
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SubAgentJson<'a> {
    subagent: &'a acp::SubagentUpdate,
    status: Option<ToolStatus>,
    items: &'a [ConversationItem],
}

fn patch<T: Clone>(stored: &mut MaybeUndefined<T>, update: &MaybeUndefined<T>) {
    if !update.is_undefined() {
        stored.clone_from(update);
    }
}

fn raw_input_text(raw_input: &serde_json::Value) -> String {
    raw_input.as_str().map_or_else(|| raw_input.to_string(), str::to_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Bash,
    SpawnSubagent,
    Other,
}

fn tool_kind(tool_name: &str) -> ToolKind {
    let name = tool_name.rsplit("__").next().unwrap_or(tool_name);
    if name.eq_ignore_ascii_case("bash") {
        ToolKind::Bash
    } else if name.eq_ignore_ascii_case("spawn_subagent") {
        ToolKind::SpawnSubagent
    } else {
        ToolKind::Other
    }
}

fn change_path(change: &acp::DiffChange) -> Option<&Path> {
    match &change.operation {
        acp::DiffChangeOperation::Add(change)
        | acp::DiffChangeOperation::Delete(change)
        | acp::DiffChangeOperation::Modify(change) => Some(&change.path.0),
        acp::DiffChangeOperation::Move(change) | acp::DiffChangeOperation::Copy(change) => Some(&change.path.0),
        _ => None,
    }
}
