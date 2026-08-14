use aether_auth::{
    EncryptedFileOAuthCredentialStorage, FakeOAuthCredentialStore, OAuthCredentialStorage, OAuthError, OsKeyringStore,
};
use aether_project::CredentialsStoreConfig;
use std::sync::Arc;

pub fn oauth_credential_store_from_config(
    config: Option<CredentialsStoreConfig>,
) -> Result<Arc<dyn OAuthCredentialStorage>, OAuthError> {
    match config {
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
    use crate::settings_args::SettingsSourceArgs;
    use std::path::Path;

    fn credentials_config_from_json(json: &str) -> Option<CredentialsStoreConfig> {
        let source = SettingsSourceArgs { settings_json: Some(json.to_string()), settings_file: None };
        source.load_settings(Path::new(".")).unwrap().credentials_store
    }

    #[test]
    fn memory_store_selected_from_settings() {
        let config = credentials_config_from_json(
            r#"{
                "credentialsStore": { "type": "memory" },
                "agents": [{"name":"a","description":"a","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]
            }"#,
        );

        let store = oauth_credential_store_from_config(config).unwrap();
        assert!(!store.contains("anything"));
    }

    #[test]
    fn encrypted_file_from_settings_propagates_missing_passphrase_error() {
        let config = credentials_config_from_json(
            r#"{
                "credentialsStore": { "type": "encryptedFile", "passwordEnv": "AETHER_DEFINITELY_UNSET_VAR_98765" },
                "agents": [{"name":"a","description":"a","model":"anthropic:claude-sonnet-4-5","userInvocable":true}]
            }"#,
        );

        let result = oauth_credential_store_from_config(config);
        assert!(result.is_err(), "missing passphrase should surface as a clean error");
    }
}
