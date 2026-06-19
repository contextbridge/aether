use acp_utils::config_meta::SelectOptionMeta;

pub struct SettingsMenuEntry {
    pub config_id: String,
    pub title: String,
    pub values: Vec<SettingsMenuValue>,
    pub current_value_index: usize,
    pub current_raw_value: String,
    pub entry_kind: SettingsMenuEntryKind,
    pub multi_select: bool,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsMenuEntryKind {
    Select,
    McpServers,
    ProviderLogins,
}

#[derive(Debug, Clone)]
pub struct SettingsMenuValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    pub is_disabled: bool,
    pub meta: SelectOptionMeta,
}

#[derive(Debug)]
pub struct SettingsChange {
    pub config_id: String,
    pub new_value: String,
}
