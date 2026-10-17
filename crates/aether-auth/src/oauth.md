OAuth 2.0 authentication primitives for the Aether agent framework.

# Architecture

- [`authorize_in_browser`] -- Opens the system browser on an authorization URL and returns the absolute callback URL.
- [`OAuthCredentialStorage`] -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- [`KeyringCredentialStore`] -- OS-keychain-backed [`OAuthCredentialStorage`] (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- [`EncryptedFileCredentialStore`] -- File-backed [`OAuthCredentialStorage`] that encrypts the file with [`age`](https://docs.rs/age). The passphrase is read from an environment variable.
- [`MemoryCredentialStore`] -- In-memory storage, for tests and sessions that should not persist credentials.
- [`bind_loopback`] / [`accept_oauth_callback`] -- Listen for an OAuth redirect on a local port.

MCP server OAuth lives in `aether-mcp-utils`, which drives `rmcp`'s authorization state machine with these primitives.

# Errors

All OAuth-specific errors are represented by [`OAuthError`].
