OAuth 2.0 authentication primitives for the Aether agent framework.

# Architecture

- [`OAuthHandler`] -- Trait implemented by consuming applications to handle OAuth UI/UX. The handler opens a browser and returns the absolute callback URL.
- [`BrowserOAuthHandler`] -- Default implementation that opens the system browser and listens on a dynamic local port.
- [`OAuthCredentialStorage`] -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- [`OsKeyringStore`] -- OS-keychain-backed [`OAuthCredentialStorage`] (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- [`EncryptedFileOAuthCredentialStorage`] -- File-backed [`OAuthCredentialStorage`] that encrypts the file with [`age`](https://docs.rs/age). The passphrase is read from an environment variable.
- [`FakeOAuthCredentialStore`] -- In-memory storage for tests.

Behind the `mcp` feature:

- [`McpCredentialStore`] -- Per-server adapter that binds an [`OAuthCredentialStorage`] to one MCP server ID and implements `rmcp::transport::auth::CredentialStore`.
- [`perform_oauth_flow`] -- Integrates Aether's browser/callback UI with `rmcp`'s MCP OAuth state machine. `rmcp` provides discovery, registration selection (pre-registered client, CIMD, then DCR), PKCE, resource indicators, scope selection and unioning, refresh, and issuer validation.
- [`create_auth_manager_from_store`] -- Build an issuer-bound `AuthorizationManager` from stored credentials, handling automatic token refresh and rejecting credentials minted by a different authorization server.

# Errors

All OAuth-specific errors are represented by [`OAuthError`].
