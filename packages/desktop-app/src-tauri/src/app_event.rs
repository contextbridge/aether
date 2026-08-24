use agent_client_protocol::schema::v1::SessionUpdate;
use serde::Serialize;
use specta::Type;

struct AcpSessionUpdateType;

impl Type for AcpSessionUpdateType {
    fn definition(_: &mut specta::Types) -> specta::datatype::DataType {
        specta::datatype::DataType::Reference(specta_typescript::define("SessionUpdate"))
    }
}

/// Events serialized to the renderer using the ACP v1 wire representation.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub(crate) enum AppEvent {
    SessionUpdate {
        connection_id: String,
        session_id: String,
        #[specta(type = AcpSessionUpdateType)]
        update: Box<SessionUpdate>,
    },
    PromptDone {
        session_id: String,
        connection_id: String,
        stop_reason: String,
    },
    PromptError {
        session_id: String,
        connection_id: String,
        error: String,
    },
    ConnectionClosed {
        session_id: String,
        connection_id: String,
        error: Option<String>,
    },
}
