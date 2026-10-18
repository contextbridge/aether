# aether-auth

OAuth credential storage and browser authorization primitives for Aether. Provides a pluggable credential storage trait with OS-keychain, encrypted-file and in-memory implementations, helpers for capturing an OAuth redirect on a local port, and AWS SigV4 request signing.

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
- **`KeyringCredentialStore`** -- `OAuthCredentialStorage` backed by the OS keychain (macOS Keychain, Windows Credential Manager, Linux/FreeBSD Secret Service). Available under the `keyring` feature.
- **`EncryptedFileCredentialStore`** -- `OAuthCredentialStorage` in a file encrypted with an [`age`](https://docs.rs/age) passphrase read from an environment variable.
- **`MemoryCredentialStore`** -- In-memory `OAuthCredentialStorage`, for tests and sessions that should not persist credentials.
- **`authorize_in_browser`** -- Opens an authorization URL in the system browser and returns the callback URL it redirects to.
- **`bind_loopback`** / **`accept_oauth_callback`** -- The local-port redirect listener behind `authorize_in_browser`, for flows that present the URL themselves.
- **`oauth_http_client`** -- HTTP client for token requests that refuses redirects.
- **`SigV4Signer`** -- Signs requests with AWS SigV4. Available under the `sigv4` feature.
- **`OAuthError`** -- Error enum returned by every fallible OAuth API in this crate.

## Usage

Implement `OAuthCredentialStorage` for your own backend, or use the OS keychain store under the `keyring` feature:

```rust,no_run
use aether_auth::{OAuthCredential, OAuthCredentialStorage, KeyringCredentialStore};

# async fn example() -> Result<(), aether_auth::OAuthError> {
let store = KeyringCredentialStore::with_platform_store();

store.save_credential("anthropic", OAuthCredential::new("client-id", "token")).await?;

let loaded = store.load_credential("anthropic").await?;
# Ok(())
# }
```

MCP server OAuth (discovery, registration, PKCE, refresh) lives in `aether-mcp-utils`, which builds on these primitives and `rmcp`'s authorization state machine.

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `keyring` | `KeyringCredentialStore` backed by the platform's native keychain | no |
| `sigv4` | `SigV4Signer` for AWS SigV4 request signing | no |

## License

MIT
