use acp_utils::notifications::RemoteServerInfo;
use aether_browser::types::{AetherClientErrorDetails, AetherClientEvent, AetherClientOptions, ConversationSnapshot};
use agent_client_protocol::schema::v2::{
    AgentNotification, AgentRequest, AgentResponse, ClientNotification, ClientRequest, ClientResponse,
};
use schemars::SchemaGenerator;
use utils::schema_document::SchemaDocument;

fn main() {
    SchemaDocument::with_external(register_acp)
        .output::<AetherClientEvent<'static>>()
        .output::<ConversationSnapshot<'static>>()
        .output::<AetherClientErrorDetails>()
        .output::<RemoteServerInfo>()
        .input::<AetherClientOptions>()
        .print();
}

fn register_acp(generator: &mut SchemaGenerator) {
    generator.subschema_for::<AgentRequest>();
    generator.subschema_for::<AgentResponse>();
    generator.subschema_for::<AgentNotification>();
    generator.subschema_for::<ClientRequest>();
    generator.subschema_for::<ClientResponse>();
    generator.subschema_for::<ClientNotification>();
}
