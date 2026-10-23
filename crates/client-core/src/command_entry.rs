use agent_client_protocol::schema::v2 as acp;

/// One slash command the composer can offer: a client builtin or one the
/// agent advertised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub has_input: bool,
    pub hint: Option<String>,
    pub builtin: bool,
}

impl CommandEntry {
    /// The entry for a command the agent advertised.
    pub fn from_acp(command: &acp::AvailableCommand) -> Self {
        Self {
            name: command.name.clone(),
            description: command.description.clone(),
            has_input: command.input.is_some(),
            hint: match &command.input {
                Some(acp::AvailableCommandInput::Text(input)) => Some(input.hint.clone()),
                _ => None,
            },
            builtin: false,
        }
    }
}
