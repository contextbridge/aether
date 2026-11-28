use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::v2::schema::{
    McpRequestId, McpServer, McpServerAcpId, MessageMcpNotification, MessageMcpRequest, MessageMcpResponse,
};
use agent_client_protocol::{
    Client, ConnectionTo, Dispatch, DynamicHandlerGuard, Error, HandleDispatchFrom, Handled, RequestCancellationHandle,
};
use mcp_utils::client::Transport;
use rmcp::RoleClient;
use rmcp::model::{
    ClientJsonRpcMessage, ClientNotification, ClientRequest, ErrorCode, ErrorData, JsonObject, JsonRpcMessage,
    JsonRpcRequest, RequestId, ServerJsonRpcMessage,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

pub(crate) type AttachedClient = watch::Receiver<Option<ClientLink>>;

#[derive(Clone)]
pub(crate) struct ClientLink {
    cx: ConnectionTo<Client>,
    servers: HashMap<String, McpServerAcpId>,
}

impl ClientLink {
    pub(crate) fn new(cx: ConnectionTo<Client>, servers: &[McpServer]) -> Self {
        let servers = servers
            .iter()
            .filter_map(|server| match server {
                McpServer::Acp(server) => Some((server.name.clone(), server.server_id.clone())),
                _ => None,
            })
            .collect();
        Self { cx, servers }
    }

    pub(crate) fn cx(&self) -> &ConnectionTo<Client> {
        &self.cx
    }
}

pub(crate) fn client_mcp_transport(client: AttachedClient, name: String) -> Transport {
    Transport::custom(move || AcpTransport::open(client.borrow().as_ref()?, &name))
}

struct AcpTransport {
    cx: ConnectionTo<Client>,
    server: McpServerAcpId,
    ids: WireIds,
    in_flight: HashMap<RequestId, RequestCancellationHandle>,
    inbox: mpsc::UnboundedSender<Inbound>,
    incoming: mpsc::UnboundedReceiver<Inbound>,
    _notifications: DynamicHandlerGuard<Client>,
}

enum Inbound {
    Notification(ServerJsonRpcMessage),
    Settled(RequestId, Option<ServerJsonRpcMessage>),
}

impl AcpTransport {
    fn open(client: &ClientLink, name: &str) -> Option<Self> {
        let server = client.servers.get(name)?.clone();
        let ids = WireIds(Uuid::new_v4());
        let (inbox, incoming) = mpsc::unbounded_channel();
        let notifications = Notifications { server: server.clone(), ids, inbox: inbox.clone() };
        let guard = client
            .cx
            .add_dynamic_handler(notifications)
            .inspect_err(|error| tracing::warn!(?error, server = name, "Client MCP server is unreachable"))
            .ok()?;
        Some(Self {
            cx: client.cx.clone(),
            server,
            ids,
            in_flight: HashMap::new(),
            inbox,
            incoming,
            _notifications: guard,
        })
    }

    fn request(
        &mut self,
        request: JsonRpcRequest<ClientRequest>,
    ) -> Result<impl Future<Output = Result<(), Error>> + Send + use<>, Error> {
        let id = request.id;
        let wire = self.ids.wire(&id);
        let outgoing = self.cx.prepare_request(request_to_wire(self.server.clone(), wire.clone(), &request.request)?);
        self.in_flight.insert(id.clone(), outgoing.cancellation_handle());
        let inbox = self.inbox.clone();
        let response = outgoing.block_task();
        Ok(async move {
            let (message, outcome) = match response.await.and_then(|response| response_from_wire(response, &wire, &id))
            {
                Ok(message) => (Some(message), Ok(())),
                Err(error) => (None, Err(error)),
            };
            let _ = inbox.send(Inbound::Settled(id, message));
            outcome
        })
    }

    fn cancel(&self, id: Option<RequestId>) -> Result<(), Error> {
        id.and_then(|id| self.in_flight.get(&id)).map_or(Ok(()), RequestCancellationHandle::cancel)
    }
}

impl rmcp::transport::Transport<RoleClient> for AcpTransport {
    type Error = Error;

    fn send(&mut self, item: ClientJsonRpcMessage) -> impl Future<Output = Result<(), Error>> + Send + 'static {
        let response = match item {
            JsonRpcMessage::Request(request) => self.request(request).map(Some),
            JsonRpcMessage::Notification(note) => match note.notification {
                ClientNotification::CancelledNotification(cancel) => {
                    self.cancel(cancel.params.request_id).map(|()| None)
                }
                _ => Ok(None),
            },
            _ => Ok(None),
        };
        async move {
            match response? {
                Some(response) => response.await,
                None => Ok(()),
            }
        }
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        loop {
            let inbound = tokio::select! {
                () = self.cx.incoming_closed() => None,
                inbound = self.incoming.recv() => inbound,
            };
            match inbound? {
                Inbound::Notification(message) => return Some(message),
                Inbound::Settled(id, message) => {
                    self.in_flight.remove(&id);
                    if message.is_some() {
                        return message;
                    }
                }
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Error>> + Send {
        self.incoming.close();
        std::future::ready(Ok(()))
    }
}

struct Notifications {
    server: McpServerAcpId,
    ids: WireIds,
    inbox: mpsc::UnboundedSender<Inbound>,
}

impl HandleDispatchFrom<Client> for Notifications {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Client>,
    ) -> Result<Handled<Dispatch>, Error> {
        MatchDispatchFrom::new(message, &cx)
            .if_notification(async |note: MessageMcpNotification| {
                let id = self.ids.local(&note.request_id).filter(|_| note.server_id == self.server);
                let Some(id) = id else { return Ok(Handled::No { message: note, retry: false }) };
                match notification_from_wire(&note, &id) {
                    Ok(message) => {
                        let _ = self.inbox.send(Inbound::Notification(message));
                    }
                    Err(error) => tracing::warn!(%error, "Invalid MCP notification"),
                }
                Ok(Handled::Yes)
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "ClientMcpNotifications"
    }
}

#[derive(Clone, Copy)]
struct WireIds(Uuid);

impl WireIds {
    fn wire(&self, id: &RequestId) -> McpRequestId {
        McpRequestId::new(format!("{}:{}", self.0, json!(id)))
    }

    fn local(&self, wire: &McpRequestId) -> Option<RequestId> {
        let id = wire.0.strip_prefix(&format!("{}:", self.0))?;
        serde_json::from_str(id).ok()
    }
}

fn request_to_wire(
    server: McpServerAcpId,
    wire: McpRequestId,
    request: &ClientRequest,
) -> Result<MessageMcpRequest, Error> {
    let mut body = serde_json::to_value(request)?;
    let params: Option<JsonObject> = serde_json::from_value(body["params"].take())?;
    Ok(MessageMcpRequest::new(server, wire, request.method()).params(params))
}

fn response_from_wire(
    response: MessageMcpResponse,
    wire: &McpRequestId,
    id: &RequestId,
) -> Result<ServerJsonRpcMessage, Error> {
    match response {
        MessageMcpResponse::Result { mut result, .. } => {
            restore_subscription_id(&mut result, wire, id);
            Ok(JsonRpcMessage::response(serde_json::from_value(result)?, id.clone()))
        }
        MessageMcpResponse::Error { error, .. } => Ok(JsonRpcMessage::error(
            ErrorData::new(ErrorCode(error.code), error.message, error.data.value().cloned()),
            Some(id.clone()),
        )),
        _ => Err(Error::internal_error().data("unknown MCP response")),
    }
}

fn notification_from_wire(note: &MessageMcpNotification, id: &RequestId) -> serde_json::Result<ServerJsonRpcMessage> {
    let mut payload = json!({"method": note.method, "params": note.params});
    restore_subscription_id(&mut payload["params"], &note.request_id, id);
    serde_json::from_value(payload).map(JsonRpcMessage::notification)
}

fn restore_subscription_id(payload: &mut Value, wire: &McpRequestId, id: &RequestId) {
    if let Some(value) = payload.pointer_mut("/_meta/io.modelcontextprotocol~1subscriptionId")
        && value.as_str() == Some(&wire.0)
    {
        *value = json!(id);
    }
}
