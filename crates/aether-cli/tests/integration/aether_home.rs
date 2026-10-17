use crate::TestResult;
use crate::acp_process::AcpProcess;
use aether_auth::{EncryptedFileCredentialStore, OAuthCredential, OAuthCredentialStorage};
use aether_cli::workspace::testing::init_repo;
use aether_sessions::{SessionMeta, SessionStore};
use agent_client_protocol::schema::v2 as acp;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

const PASSWORD: &str = "test-password";

#[tokio::test]
async fn encrypted_credentials_default_to_aether_home() -> TestResult {
    let homes = Homes::new()?;
    EncryptedFileCredentialStore::new(homes.aether_home().join("credentials.enc"), PASSWORD.to_string())
        .with_scrypt_work_factor(10)
        .save_credential(
            "codex",
            OAuthCredential {
                client_id: "client".to_string(),
                access_token: "token".to_string(),
                refresh_token: None,
                expires_at: None,
            },
        )
        .await?;

    let mut agent = homes.spawn_acp(r#"{"credentialsStore":{"type":"encryptedFile"},"agents":[]}"#)?;
    let response: acp::InitializeResponse = serde_json::from_value(agent.initialize()?)?;

    assert!(response.auth_methods.iter().any(|method| matches!(
        method,
        acp::AuthMethod::Agent(method)
            if method.method_id.0.as_ref() == "codex" && method.description.as_deref() == Some("authenticated")
    )));
    Ok(())
}

#[test]
fn workspace_registry_defaults_to_aether_home() -> TestResult {
    let homes = Homes::new()?;
    let repo = init_repo(homes.home(), "repo");
    let meta = SessionMeta {
        session_id: "s1".to_string(),
        cwd: repo,
        model: "anthropic:claude-sonnet-4-5".to_string(),
        selected_mode: None,
        created_at: "2026-05-01T00:00:00Z".to_string(),
    };
    SessionStore::from_path(homes.aether_home().join("sessions")).append_meta("s1", &meta)?;

    let mut agent = homes.spawn_acp(r#"{"credentialsStore":{"type":"memory"},"agents":[]}"#)?;
    agent.initialize()?;
    agent.request("_aether/workspace_list", &json!({ "sessionId": "s1" }))?;

    assert!(homes.aether_home().join("workspaces.json").is_file());
    Ok(())
}

struct Homes {
    home: TempDir,
    aether_home: TempDir,
}

impl Homes {
    fn new() -> TestResult<Self> {
        Ok(Self { home: TempDir::new()?, aether_home: TempDir::new()? })
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn aether_home(&self) -> &Path {
        self.aether_home.path()
    }

    fn spawn_acp(&self, settings_json: &str) -> TestResult<AcpProcess> {
        AcpProcess::builder()
            .log_dir(&self.home().join("logs"))
            .settings_json(settings_json)
            .env("HOME", self.home())
            .env("AETHER_HOME", self.aether_home())
            .env("AETHER_CREDENTIALS_PASSWORD", PASSWORD)
            .spawn()
    }
}
