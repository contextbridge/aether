use crate::settings_args::SettingsSourceArgs;
use aether_auth::{
    EncryptedFileOAuthCredentialStorage, FakeOAuthCredentialStore, OAuthCredentialStorage, OAuthError, OsKeyringStore,
};
use aether_project::CredentialsStoreConfig;
use std::path::Path;
use std::sync::Arc;

pub fn build_oauth_credential_store(
    settings_source: &SettingsSourceArgs,
    cwd: &Path,
) -> Result<Arc<dyn OAuthCredentialStorage>, OAuthError> {
    let settings_source =
        settings_source.load_settings(cwd).map(|settings| settings.credentials_store).map_err(|e| {
            OAuthError::CredentialStore(format!("Failed to load settings for credential store config: {e}"))
        })?;

    match settings_source {
        Some(CredentialsStoreConfig::Memory) => Ok(Arc::new(FakeOAuthCredentialStore::new())),
        Some(CredentialsStoreConfig::EncryptedFile { path, password_env }) => {
            Ok(Arc::new(EncryptedFileOAuthCredentialStorage::from_settings(path, password_env.as_deref())?))
        }
        Some(CredentialsStoreConfig::Keyring) | None => Ok(Arc::new(OsKeyringStore::with_platform_store())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_source(json: &str) -> SettingsSourceArgs {
        SettingsSourceArgs { settings_json: Some(json.to_string()), settings_file: None }
    }

    #[test]
    fn memory_store_selected_from_settings() {
        let source = json_source(
            r#"{
                "credentialsStore": { "type": "memory" },
                "agents": [{"name":"a","description":"a","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]
            }"#,
        );

        let store = build_oauth_credential_store(&source, Path::new(".")).unwrap();
        assert!(!store.has_credential("anything"));
    }

    #[test]
    fn encrypted_file_from_settings_propagates_missing_passphrase_error() {
        let source = json_source(
            r#"{
                "credentialsStore": { "type": "encryptedFile", "passwordEnv": "AETHER_DEFINITELY_UNSET_VAR_98765" },
                "agents": [{"name":"a","description":"a","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]
            }"#,
        );

        let result = build_oauth_credential_store(&source, Path::new("."));
        assert!(result.is_err(), "missing passphrase should surface as a clean error");
    }
}
