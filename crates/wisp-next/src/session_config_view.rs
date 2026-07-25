use acp_utils::config_meta::SelectOptionMeta;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{
    SessionConfigKind, SessionConfigOption, SessionConfigSelect, SessionConfigSelectOption, SessionConfigSelectOptions,
};
use utils::ReasoningEffort;

/// A pure, borrowed interpretation of an ACP session configuration schema.
pub struct SessionConfigView<'a> {
    options: &'a [SessionConfigOption],
}

/// The select payload behind `option`, or nothing when it is another kind.
pub fn as_select(option: &SessionConfigOption) -> Option<&SessionConfigSelect> {
    match &option.kind {
        SessionConfigKind::Select(select) => Some(select),
        _ => None,
    }
}

/// Every value `select` offers, in display order, with groups flattened away.
///
/// Callers that only present or cycle values never need the grouping, and
/// flattening in one place keeps grouped options from being silently skipped.
pub fn select_values(select: &SessionConfigSelect) -> Vec<&SessionConfigSelectOption> {
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups.iter().flat_map(|group| group.options.iter()).collect(),
        _ => Vec::new(),
    }
}

impl<'a> SessionConfigView<'a> {
    pub fn new(options: &'a [SessionConfigOption]) -> Self {
        Self { options }
    }

    pub fn select(&self, id: ConfigOptionId) -> Option<&'a SessionConfigSelect> {
        self.options.iter().find(|option| option.id.0.as_ref() == id.as_str()).and_then(as_select)
    }

    pub fn flattened_options(&self, id: ConfigOptionId) -> Vec<&'a SessionConfigSelectOption> {
        self.select(id).map(select_values).unwrap_or_default()
    }

    pub fn current_values(&self, id: ConfigOptionId) -> Vec<&'a str> {
        self.select(id)
            .map(|select| select.current_value.0.split(',').map(str::trim).filter(|value| !value.is_empty()).collect())
            .unwrap_or_default()
    }

    pub fn current_display_name(&self, id: ConfigOptionId) -> Option<String> {
        let values = self.current_values(id);
        let options = self.flattened_options(id);
        let names: Vec<_> = values
            .iter()
            .filter_map(|value| {
                options.iter().find(|option| option.value.0.as_ref() == *value).map(|option| option.name.as_str())
            })
            .collect();
        (!names.is_empty()).then(|| names.join(" + "))
    }

    pub fn reasoning_levels(&self) -> Vec<ReasoningEffort> {
        self.flattened_options(ConfigOptionId::ReasoningEffort)
            .into_iter()
            .filter_map(|option| option.value.0.as_ref().parse().ok())
            .collect()
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        ReasoningEffort::parse(self.select(ConfigOptionId::ReasoningEffort)?.current_value.0.as_ref()).unwrap_or(None)
    }

    pub fn selected_model_metadata(&self) -> Vec<SelectOptionMeta> {
        let values = self.current_values(ConfigOptionId::Model);
        let options = self.flattened_options(ConfigOptionId::Model);
        values
            .iter()
            .filter_map(|value| {
                options
                    .iter()
                    .find(|option| option.value.0.as_ref() == *value)
                    .map(|option| SelectOptionMeta::from_meta(option.meta.as_ref()))
            })
            .collect()
    }
}
