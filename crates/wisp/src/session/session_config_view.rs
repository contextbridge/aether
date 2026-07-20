use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::v1::{self as acp, SessionConfigOptionCategory};
use utils::ReasoningEffort;

/// Client-owned configuration state projected from an ACP session schema.
///
/// ACP values are copied at the protocol boundary so optimistic edits and
/// reconciliation never mutate a protocol object borrowed by the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfigOption {
    pub id: String,
    pub name: String,
    pub(crate) category: Option<SessionConfigOptionCategory>,
    pub(crate) meta: Option<acp::Meta>,
    pub(crate) kind: LocalConfigKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalConfigKind {
    Select { current_value: String, values: Vec<LocalConfigValue>, multi_select: bool },
    Boolean { current_value: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfigValue {
    pub value: String,
    pub name: String,
    pub group: Option<String>,
    pub description: Option<String>,
    pub is_disabled: bool,
    pub meta: SelectOptionMeta,
    pub raw_meta: Option<acp::Meta>,
}

impl LocalConfigOption {
    pub fn from_acp(option: acp::SessionConfigOption) -> Self {
        let kind = match option.kind {
            acp::SessionConfigKind::Select(select) => LocalConfigKind::Select {
                current_value: select.current_value.0.to_string(),
                values: match select.options {
                    acp::SessionConfigSelectOptions::Ungrouped(options) => {
                        options.into_iter().map(|option| (None, option)).collect()
                    }
                    acp::SessionConfigSelectOptions::Grouped(groups) => groups
                        .into_iter()
                        .flat_map(|group| {
                            let name = group.name;
                            group.options.into_iter().map(move |option| (Some(name.clone()), option))
                        })
                        .collect(),
                    _ => Vec::new(),
                }
                .into_iter()
                .map(|(group, value)| LocalConfigValue {
                    value: value.value.0.to_string(),
                    name: value.name,
                    group,
                    is_disabled: value.description.as_deref().is_some_and(|text| text.starts_with("Unavailable:")),
                    description: value.description,
                    meta: SelectOptionMeta::from_meta(value.meta.as_ref()),
                    raw_meta: value.meta,
                })
                .collect(),
                multi_select: ConfigOptionMeta::from_meta(option.meta.as_ref()).multi_select,
            },
            acp::SessionConfigKind::Boolean(boolean) => {
                LocalConfigKind::Boolean { current_value: boolean.current_value }
            }
            _ => LocalConfigKind::Boolean { current_value: false },
        };
        Self { id: option.id.0.to_string(), name: option.name, category: option.category, meta: option.meta, kind }
    }

    pub(crate) fn select(&self) -> Option<LocalConfigSelect<'_>> {
        match &self.kind {
            LocalConfigKind::Select { current_value, values, multi_select: _ } => {
                Some(LocalConfigSelect { current_value, values })
            }
            LocalConfigKind::Boolean { .. } => None,
        }
    }

    /// The selected value of a select option, `None` for other kinds.
    pub fn current_value(&self) -> Option<&str> {
        match &self.kind {
            LocalConfigKind::Select { current_value, .. } => Some(current_value),
            LocalConfigKind::Boolean { .. } => None,
        }
    }
}

pub struct LocalConfigSelect<'a> {
    pub current_value: &'a str,
    pub values: &'a [LocalConfigValue],
}

/// A pure view over client-owned configuration state.
pub struct LocalConfigView<'a> {
    options: &'a [LocalConfigOption],
}

impl<'a> LocalConfigView<'a> {
    pub fn new(options: &'a [LocalConfigOption]) -> Self {
        Self { options }
    }

    pub fn select(&self, id: ConfigOptionId) -> Option<LocalConfigSelect<'a>> {
        self.options.iter().find(|option| option.id == id.as_str()).and_then(LocalConfigOption::select)
    }

    pub fn flattened_options(&self, id: ConfigOptionId) -> Vec<&'a LocalConfigValue> {
        self.select(id).map_or_else(Vec::new, |select| select.values.iter().collect())
    }

    pub fn current_values(&self, id: ConfigOptionId) -> Vec<&'a str> {
        self.select(id)
            .map(|select| select.current_value.split(',').map(str::trim).filter(|value| !value.is_empty()).collect())
            .unwrap_or_default()
    }

    pub fn current_display_name(&self, id: ConfigOptionId) -> Option<String> {
        let values = self.current_values(id);
        let options = self.flattened_options(id);
        let names: Vec<_> = values
            .iter()
            .filter_map(|value| options.iter().find(|option| option.value == *value).map(|option| option.name.as_str()))
            .collect();
        (!names.is_empty()).then(|| names.join(" + "))
    }

    pub fn next_mode(&self) -> Option<(&'a str, &'a str)> {
        let option = self
            .options
            .iter()
            .find(|option| option.category == Some(SessionConfigOptionCategory::Mode) && option.select().is_some())?;
        let select = option.select()?;
        let current = select.values.iter().position(|value| value.value == select.current_value).unwrap_or(0);
        let next = select.values.get((current + 1) % select.values.len().max(1))?;
        Some((option.id.as_str(), next.value.as_str()))
    }

    pub fn reasoning_levels(&self) -> Vec<ReasoningEffort> {
        self.flattened_options(ConfigOptionId::ReasoningEffort)
            .into_iter()
            .filter_map(|option| option.value.parse().ok())
            .collect()
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        ReasoningEffort::parse(self.select(ConfigOptionId::ReasoningEffort)?.current_value).unwrap_or(None)
    }

    pub fn selected_model_metadata(&self) -> Vec<SelectOptionMeta> {
        let values = self.current_values(ConfigOptionId::Model);
        let options = self.flattened_options(ConfigOptionId::Model);
        values
            .iter()
            .filter_map(|value| options.iter().find(|option| option.value == *value).map(|option| option.meta.clone()))
            .collect()
    }
}
