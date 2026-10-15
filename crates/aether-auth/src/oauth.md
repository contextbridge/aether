OAuth 2.0 authentication primitives for the Aether agent framework.

# Architecture

- [`OAuthHandler`] -- Trait implemented by consuming applications to handle OAuth UI/UX. The handler opens a browser and returns the absolute callback URL.
- [`BrowserOAuthHandler`] -- Default implementation that opens the system browser and listens on a local port.
- [`OAuthCredentialStorage`] -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- [`OsKeyringStore`] -- OS-keychain-backed [`OAuthCredentialStorage`] (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- [`EncryptedFileOAuthCredentialStorage`] -- File-backed [`OAuthCredentialStorage`] that encrypts the file with [`age`](https://docs.rs/age). The passphrase is read from an environment variable.
- [`FakeOAuthCredentialStore`] -- In-memory storage for tests.
- [`bind_loopback`] / [`accept_oauth_callback`] -- Listen for an OAuth redirect on a local port.

MCP server OAuth lives in `aether-mcp-utils`, which drives `rmcp`'s authorization state machine with these primitives.

# Errors

All OAuth-specific errors are represented by [`OAuthError`].
