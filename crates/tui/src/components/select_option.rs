/// A single option in a selection list.
#[derive(Debug, Clone)]
pub struct SelectOption {
    pub value: String,
    pub title: String,
    pub description: Option<String>,
}
