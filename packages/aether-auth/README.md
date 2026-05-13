# aether-auth

OAuth credential storage and authorization flows for Aether. Provides a pluggable credential storage trait, an OS-keychain-backed implementation, and an end-to-end OAuth authorization-code flow for MCP servers.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Key Types](#key-types)
- [Usage](#usage)
- [Feature Flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Key Types

- **`OAuthCredentialStorage`** -- Trait for persisting OAuth credentials keyed by provider ID, MCP server ID, or another credential key.
- **`OAuthCredential`** -- Access token, refresh token, client ID, and expiry timestamp for a single OAuth identity.
- **`OAuthHandler`** -- Trait implemented by consuming applications to drive the OAuth UI/UX (open a browser, wait for the redirect).
- **`BrowserOAuthHandler`** -- Default `OAuthHandler` that opens the system browser and listens on a dynamic local port.
- **`OsKeyringStore`** -- `OAuthCredentialStorage` backed by the OS keychain (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- **`FakeOAuthCredentialStore`** -- In-memory `OAuthCredentialStorage` for tests.
- **`McpCredentialStore`** -- Per-server adapter that binds an `OAuthCredentialStorage` to one MCP server ID and implements `rmcp::transport::auth::CredentialStore`. Available under the `mcp` feature.
- **`OAuthError`** -- Error enum returned by every fallible API in this crate.

## Usage

Implement `OAuthCredentialStorage` for your own backend, or use the OS keychain store under the `keyring` feature:

```rust,no_run
use aether_auth::{OAuthCredential, OAuthCredentialStorage, OsKeyringStore};

# async fn example() -> Result<(), aether_auth::OAuthError> {
let store = OsKeyringStore::with_platform_store();

store
    .save_credential(
        "anthropic",
        OAuthCredential {
            client_id: "client-id".into(),
            access_token: "token".into(),
            refresh_token: None,
            expires_at: None,
        },
    )
    .await?;

let loaded = store.load_credential("anthropic").await?;
# Ok(())
# }
```

For MCP servers that require OAuth, the `mcp` feature provides `perform_oauth_flow`, which orchestrates the full authorization-code flow (browser launch, callback capture, token exchange, credential storage) and `create_auth_manager_from_store`, which builds an `rmcp::transport::auth::AuthorizationManager` from stored credentials with automatic token refresh.

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `keyring` | `OsKeyringStore` backed by the platform's native keychain | no |
| `mcp` | MCP credential store, authorization-code flow, and `AuthorizationManager` integration via `rmcp` | no |

## License

MIT
