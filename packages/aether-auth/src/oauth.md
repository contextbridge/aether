OAuth 2.0 authentication primitives for the Aether agent framework.

# Architecture

- [`OAuthHandler`] -- Trait implemented by consuming applications to handle OAuth UI/UX. The handler opens a browser and waits for the authorization code on a local port.
- [`BrowserOAuthHandler`] -- Default implementation that opens the system browser and listens on a dynamic local port.
- [`OAuthCredentialStorage`] -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- [`OsKeyringStore`] -- OS-keychain-backed [`OAuthCredentialStorage`] (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- [`FakeOAuthCredentialStore`] -- In-memory storage for tests.

Behind the `mcp` feature:

- [`McpCredentialStore`] -- Per-server adapter that binds an [`OAuthCredentialStorage`] to one MCP server ID and implements `rmcp::transport::auth::CredentialStore`.
- [`perform_oauth_flow`] -- Orchestrates the full MCP authorization code flow: browser launch, callback capture, token exchange, and credential storage.
- [`create_auth_manager_from_store`] -- Build an `AuthorizationManager` from stored credentials, handling automatic token refresh.

# Errors

All OAuth-specific errors are represented by [`OAuthError`].
