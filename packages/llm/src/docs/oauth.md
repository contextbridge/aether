OAuth 2.0 authentication for LLM providers that require it.

This module is feature-gated behind `oauth`. Enable it in `Cargo.toml`:
```toml
aether-llm = { version = "...", features = ["oauth"] }
```

# Architecture

- [`OAuthHandler`] -- Trait for handling the OAuth callback. The handler opens a browser and waits for the authorization code on a local port.
- [`BrowserOAuthHandler`] -- Default implementation that opens the system browser and listens on a dynamic local port.
- [`OAuthCredentialStorage`] -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- [`OAuthCredentialStore`] -- Keyed OS credential-store repository backed by macOS Keychain, Windows Credential Manager, or Linux/FreeBSD Secret Service.
- [`McpCredentialStore`] -- Per-server adapter used internally for `rmcp` integration. It binds an [`OAuthCredentialStore`] to one MCP server ID and implements `rmcp::transport::auth::CredentialStore`.

Tests should use [`FakeOAuthCredentialStore`] or construct [`OAuthCredentialStore`] with `keyring_core::mock::Store` to avoid OS keychain prompts.

# Running the flow

[`perform_oauth_flow`] orchestrates the full MCP authorization code flow: browser launch, callback capture, token exchange, and credential storage. It creates an [`McpCredentialStore`] adapter for the target server while sharing the keyed [`OAuthCredentialStore`] repository.

[`create_auth_manager_from_store`] creates an auth manager from stored credentials, handling automatic token refresh.

# Errors

All OAuth-specific errors are represented by [`OAuthError`].
