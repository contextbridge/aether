use agent_client_protocol::schema::v2::{
    ElicitationContentValue, ElicitationPropertySchema, ElicitationSchema, EnumOption, MultiSelectItems,
    MultiSelectPropertySchema, StringPropertySchema,
};
use std::collections::BTreeMap;

/// One question of a form elicitation, read from its schema.
///
/// A field describes the question only. The host keeps whatever editing state
/// it needs and hands the answer back as a [`FieldInput`] to validate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    name: String,
    label: String,
    description: Option<String>,
    required: bool,
    kind: FormFieldKind,
    default_input: FieldInput,
}

/// The control a [`FormField`] is answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldKind {
    Text,
    Integer,
    Number,
    Boolean,
    /// Exactly one of the options.
    Single(Vec<SelectOption>),
    /// Any number of the options.
    Multi(Vec<SelectOption>),
}

/// One choice of a single- or multi-select field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    /// What the answer sends.
    pub value: String,
    /// What the user sees.
    pub title: String,
}

/// The user's raw answer to a [`FormField`], before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldInput {
    /// What was typed into a text, integer, or number field.
    Text(String),
    Boolean(bool),
    /// The index of the chosen option.
    Single(usize),
    /// Whether each option is checked, in option order.
    Multi(Vec<bool>),
}

/// Why an answer cannot be sent. Displays as the message to show beside the
/// field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FieldError {
    #[error("{label} is required")]
    Required { label: String },
    #[error("{label} must be an integer")]
    NotAnInteger { label: String },
    #[error("{label} must be a number")]
    NotANumber { label: String },
}

/// The first field of a form whose answer cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{error}")]
pub struct FormError {
    /// The field's position in the form.
    pub index: usize,
    pub error: FieldError,
}

/// The fields of a form elicitation, in schema order, or `None` when the
/// schema asks for a property no form can answer.
pub fn form_fields(schema: &ElicitationSchema) -> Option<Vec<FormField>> {
    let required = schema.required.as_deref().unwrap_or(&[]);
    schema
        .properties
        .iter()
        .map(|(name, property)| FormField::from_property(name, property, required.contains(name)))
        .collect()
}

/// The content of an accepted form: every answered field's value under its
/// name, with unanswered optional fields left out.
pub fn accept_content<'a>(
    answers: impl IntoIterator<Item = (&'a FormField, FieldInput)>,
) -> Result<BTreeMap<String, ElicitationContentValue>, FormError> {
    let mut content = BTreeMap::new();
    for (index, (field, input)) in answers.into_iter().enumerate() {
        if let Some(value) = field.value(&input).map_err(|error| FormError { index, error })? {
            content.insert(field.name.clone(), value);
        }
    }
    Ok(content)
}

impl FormField {
    /// The property name the answer is sent under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The schema's title, or the property name when it has none.
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn kind(&self) -> &FormFieldKind {
        &self.kind
    }

    /// The answer the field starts on: the schema's default, or an empty
    /// one. A single-select whose default is not offered starts on its first
    /// option.
    pub fn default_input(&self) -> &FieldInput {
        &self.default_input
    }

    /// The value `input` sends, or `None` when an optional field was left
    /// empty. An input of the wrong shape for this field counts as no answer.
    pub fn value(&self, input: &FieldInput) -> Result<Option<ElicitationContentValue>, FieldError> {
        let label = || self.label.clone();
        let value = match (&self.kind, input) {
            (FormFieldKind::Text | FormFieldKind::Integer | FormFieldKind::Number, FieldInput::Text(text))
                if text.is_empty() =>
            {
                None
            }
            (FormFieldKind::Text, FieldInput::Text(text)) => Some(ElicitationContentValue::String(text.clone())),
            (FormFieldKind::Integer, FieldInput::Text(text)) => Some(
                text.parse::<i64>()
                    .map(ElicitationContentValue::Integer)
                    .map_err(|_| FieldError::NotAnInteger { label: label() })?,
            ),
            (FormFieldKind::Number, FieldInput::Text(text)) => Some(
                text.parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite())
                    .map(ElicitationContentValue::Number)
                    .ok_or_else(|| FieldError::NotANumber { label: label() })?,
            ),
            (FormFieldKind::Boolean, FieldInput::Boolean(value)) => Some(ElicitationContentValue::Boolean(*value)),
            (FormFieldKind::Single(options), FieldInput::Single(selected)) => {
                options.get(*selected).map(|option| ElicitationContentValue::String(option.value.clone()))
            }
            (FormFieldKind::Multi(options), FieldInput::Multi(selected)) => Some(ElicitationContentValue::StringArray(
                checked(options, selected).map(|o| o.value.clone()).collect(),
            )),
            _ => None,
        };
        match value {
            None if self.required => Err(FieldError::Required { label: label() }),
            value => Ok(value),
        }
    }

    /// `input` as the user would read it back: the text typed, `Yes` or
    /// `No`, or the titles of the chosen options.
    pub fn display_value(&self, input: &FieldInput) -> String {
        match (&self.kind, input) {
            (FormFieldKind::Text | FormFieldKind::Integer | FormFieldKind::Number, FieldInput::Text(text)) => {
                text.clone()
            }
            (FormFieldKind::Boolean, FieldInput::Boolean(value)) => if *value { "Yes" } else { "No" }.to_string(),
            (FormFieldKind::Single(options), FieldInput::Single(selected)) => {
                options.get(*selected).map(|option| option.title.clone()).unwrap_or_default()
            }
            (FormFieldKind::Multi(options), FieldInput::Multi(selected)) => {
                checked(options, selected).map(|option| option.title.as_str()).collect::<Vec<_>>().join(", ")
            }
            _ => String::new(),
        }
    }
}

impl FormFieldKind {
    /// The choices of a select, or nothing for any other kind.
    pub fn options(&self) -> &[SelectOption] {
        match self {
            Self::Single(options) | Self::Multi(options) => options,
            _ => &[],
        }
    }
}

impl FormField {
    fn from_property(name: &str, property: &ElicitationPropertySchema, required: bool) -> Option<Self> {
        let (title, description, (kind, default_input)) = match property {
            ElicitationPropertySchema::Boolean(value) => (
                &value.title,
                &value.description,
                (FormFieldKind::Boolean, FieldInput::Boolean(value.default.unwrap_or(false))),
            ),
            ElicitationPropertySchema::Integer(value) => (
                &value.title,
                &value.description,
                (FormFieldKind::Integer, FieldInput::Text(stringified_default(value.default))),
            ),
            ElicitationPropertySchema::Number(value) => (
                &value.title,
                &value.description,
                (FormFieldKind::Number, FieldInput::Text(stringified_default(value.default))),
            ),
            ElicitationPropertySchema::String(value) => (&value.title, &value.description, string_kind(value)),
            ElicitationPropertySchema::Array(value) => (&value.title, &value.description, multi_kind(value)?),
            _ => return None,
        };
        Some(Self {
            name: name.to_string(),
            label: title.as_deref().unwrap_or(name).to_string(),
            description: description.clone(),
            required,
            kind,
            default_input,
        })
    }
}

fn checked<'a>(options: &'a [SelectOption], selected: &'a [bool]) -> impl Iterator<Item = &'a SelectOption> {
    options.iter().zip(selected).filter(|(_, selected)| **selected).map(|(option, _)| option)
}

fn string_kind(schema: &StringPropertySchema) -> (FormFieldKind, FieldInput) {
    if let Some(options) = &schema.one_of {
        single(options_from_enum_options(options), schema.default.as_deref())
    } else if let Some(options) = &schema.enum_values {
        single(options_from_strings(options), schema.default.as_deref())
    } else {
        (FormFieldKind::Text, FieldInput::Text(schema.default.clone().unwrap_or_default()))
    }
}

fn multi_kind(schema: &MultiSelectPropertySchema) -> Option<(FormFieldKind, FieldInput)> {
    let options = match &schema.items {
        MultiSelectItems::String(items) => options_from_strings(&items.values),
        MultiSelectItems::Titled(items) => options_from_enum_options(&items.options),
        _ => return None,
    };
    Some(multi(options, schema.default.as_deref()))
}

fn single(options: Vec<SelectOption>, default: Option<&str>) -> (FormFieldKind, FieldInput) {
    let selected = default.and_then(|value| options.iter().position(|o| o.value == value)).unwrap_or(0);
    (FormFieldKind::Single(options), FieldInput::Single(selected))
}

fn multi(options: Vec<SelectOption>, defaults: Option<&[String]>) -> (FormFieldKind, FieldInput) {
    let defaults = defaults.unwrap_or(&[]);
    let selected = options.iter().map(|o| defaults.contains(&o.value)).collect();
    (FormFieldKind::Multi(options), FieldInput::Multi(selected))
}

fn options_from_strings(values: &[String]) -> Vec<SelectOption> {
    values.iter().map(|s| SelectOption { value: s.clone(), title: s.clone() }).collect()
}

fn options_from_enum_options(items: &[EnumOption]) -> Vec<SelectOption> {
    items.iter().map(|item| SelectOption { value: item.value.clone(), title: item.title.clone() }).collect()
}

fn stringified_default<T: std::fmt::Display>(default: Option<T>) -> String {
    default.map(|value| value.to_string()).unwrap_or_default()
}
