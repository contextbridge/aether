mod transport;

pub use transport::{UnixSocketMcpTransport, UnixSocketPath, UnixSocketServer, UnixSocketTransportError, connect};

/// Environment variable used to pass the active session endpoint to child tools.
pub const AETHER_MCP_IPC_SOCKET: &str = "AETHER_MCP_IPC_SOCKET";

/// Private adapter tool used by the progressive CLI to discover deferred servers.
pub const LIST_SERVERS_TOOL: &str = "_aether_list_servers";
