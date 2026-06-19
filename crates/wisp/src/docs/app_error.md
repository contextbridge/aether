Application-level errors for the Wisp TUI.

# Variants

- **`Io`** — wraps [`std::io::Error`] from terminal I/O (raw mode, rendering).
- **`Acp`** — wraps [`AcpClientError`](acp_utils::client::AcpClientError) from the agent connection. Distinguishes between invalid agent command, pre-handshake connect failure, protocol failure, and agent crash.

Both variants implement `From` conversions so `?` propagation works naturally.
