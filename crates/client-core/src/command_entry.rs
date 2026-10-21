use acp_utils::notifications::AetherCapabilities;
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

/// The commands the client itself runs, given what the agent supports.
pub fn builtin_commands(capabilities: &AetherCapabilities) -> Vec<CommandEntry> {
    let mut commands: Vec<CommandEntry> = [
        ("clear", "Start a new session"),
        ("resume", "Resume a previous session"),
        ("settings", "Configure agent options"),
    ]
    .into_iter()
    .map(builtin)
    .collect();
    if capabilities.workspace_move {
        commands.push(builtin(("move", "Move session to another workspace")));
    }
    commands
}

fn builtin((name, description): (&str, &str)) -> CommandEntry {
    CommandEntry {
        name: name.to_string(),
        description: description.to_string(),
        has_input: false,
        hint: None,
        builtin: true,
    }
}
