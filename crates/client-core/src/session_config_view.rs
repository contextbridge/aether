use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::v2::{self as acp, SessionConfigOptionCategory};
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
    pub kind: LocalConfigKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalConfigKind {
    Select(LocalConfigSelect),
    Boolean { current_value: bool },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalConfigSelect {
    pub current_value: String,
    pub values: Vec<LocalConfigValue>,
    pub multi_select: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalConfigValue {
    pub value: String,
    pub name: String,
    pub group: Option<String>,
    pub description: Option<String>,
    pub is_disabled: bool,
    pub meta: SelectOptionMeta,
}

impl LocalConfigOption {
    pub fn from_acp(option: acp::SessionConfigOption) -> Self {
        let kind = match option.kind {
            acp::SessionConfigKind::Select(select) => LocalConfigKind::Select(LocalConfigSelect {
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
                })
                .collect(),
                multi_select: ConfigOptionMeta::from_meta(option.meta.as_ref()).multi_select,
            }),
            acp::SessionConfigKind::Boolean(boolean) => {
                LocalConfigKind::Boolean { current_value: boolean.current_value }
            }
            _ => LocalConfigKind::Boolean { current_value: false },
        };
        Self { id: option.config_id.0.to_string(), name: option.name, category: option.category, kind }
    }

    pub fn select(&self) -> Option<&LocalConfigSelect> {
        match &self.kind {
            LocalConfigKind::Select(select) => Some(select),
            LocalConfigKind::Boolean { .. } => None,
        }
    }

    pub fn category(&self) -> Option<&SessionConfigOptionCategory> {
        self.category.as_ref()
    }

    pub fn display_value(&self) -> Option<&str> {
        let select = self.select()?;
        let current = select.values.iter().find(|value| value.value == select.current_value);
        Some(current.map_or(select.current_value.as_str(), |value| value.name.as_str()))
    }

    /// The selected value of a select option, `None` for other kinds.
    pub fn current_value(&self) -> Option<&str> {
        self.select().map(|select| select.current_value.as_str())
    }
}

impl LocalConfigSelect {
    pub fn current_values(&self) -> impl Iterator<Item = &str> {
        self.current_value.split(',').map(str::trim).filter(|value| !value.is_empty())
    }

    pub fn selected(&self) -> impl Iterator<Item = &LocalConfigValue> {
        self.current_values().filter_map(|current| self.values.iter().find(|value| value.value == current))
    }

    pub fn selected_names(&self) -> Option<String> {
        let names: Vec<_> = self.selected().map(|value| value.name.as_str()).collect();
        (!names.is_empty()).then(|| names.join(", "))
    }
}

/// A pure view over client-owned configuration state.
pub struct LocalConfigView<'a> {
    options: &'a [LocalConfigOption],
}

impl<'a> LocalConfigView<'a> {
    pub fn new(options: &'a [LocalConfigOption]) -> Self {
        Self { options }
    }

    pub fn select(&self, id: ConfigOptionId) -> Option<&'a LocalConfigSelect> {
        self.options.iter().find(|option| option.id == id.as_str()).and_then(LocalConfigOption::select)
    }

    pub fn flattened_options(&self, id: ConfigOptionId) -> &'a [LocalConfigValue] {
        self.select(id).map_or(&[], |select| &select.values)
    }

    pub fn current_display_name(&self, id: ConfigOptionId) -> Option<String> {
        self.select(id)?.selected_names()
    }

    pub fn next_mode(&self) -> Option<(&'a str, &'a str)> {
        let (option, select) = self.options.iter().find_map(|option| {
            let select = option.select().filter(|_| option.category == Some(SessionConfigOptionCategory::Mode))?;
            Some((option, select))
        })?;
        let current = select.values.iter().position(|value| value.value == select.current_value).unwrap_or(0);
        let next = select.values.get((current + 1) % select.values.len().max(1))?;
        Some((option.id.as_str(), next.value.as_str()))
    }

    pub fn next_reasoning_effort(&self) -> Option<(&'a str, &'a str)> {
        let levels = self.reasoning_levels();
        if levels.is_empty() {
            return None;
        }
        let next = ReasoningEffort::cycle_within(self.reasoning_effort(), &levels);
        Some((ConfigOptionId::ReasoningEffort.as_str(), ReasoningEffort::config_str(next)))
    }

    pub fn reasoning_levels(&self) -> Vec<ReasoningEffort> {
        self.flattened_options(ConfigOptionId::ReasoningEffort)
            .iter()
            .filter_map(|option| option.value.parse().ok())
            .filter(|effort| *effort != ReasoningEffort::Default)
            .collect()
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        ReasoningEffort::parse(&self.select(ConfigOptionId::ReasoningEffort)?.current_value).unwrap_or(None)
    }

    pub fn selected_model_metadata(&self) -> Vec<SelectOptionMeta> {
        self.select(ConfigOptionId::Model)
            .map(|select| select.selected().map(|value| value.meta.clone()).collect())
            .unwrap_or_default()
    }
}
