use crate::TestResult;
use aether_auth::{EncryptedFileOAuthCredentialStorage, OAuthCredential, OAuthCredentialStorage};
use aether_cli::workspace::testing::init_repo;
use aether_sessions::{SessionMeta, SessionStore};
use agent_client_protocol::schema::v2::{Implementation, InitializeRequest};
use agent_client_protocol::schema::{ProtocolVersion, v2 as acp};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Error as IoError, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::TempDir;

const PASSWORD: &str = "test-password";

#[tokio::test]
async fn encrypted_credentials_default_to_aether_home() -> TestResult {
    let homes = Homes::new()?;
    EncryptedFileOAuthCredentialStorage::new(homes.aether_home().join("credentials.enc"), PASSWORD.to_string())
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_aether"))
            .arg("acp")
            .arg("--log-dir")
            .arg(self.home().join("logs"))
            .arg("--settings-json")
            .arg(settings_json)
            .env("HOME", self.home())
            .env("AETHER_HOME", self.aether_home())
            .env("AETHER_CREDENTIALS_PASSWORD", PASSWORD)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| IoError::other("child stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| IoError::other("child stdout"))?;
        Ok(AcpProcess { child, stdin, stdout: BufReader::new(stdout), next_id: 1 })
    }
}

struct AcpProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl AcpProcess {
    fn initialize(&mut self) -> TestResult<Value> {
        let params =
            serde_json::to_value(InitializeRequest::new(ProtocolVersion::V2, Implementation::new("test", "1")))?;
        self.request("initialize", &params)
    }

    fn request(&mut self, method: &str, params: &Value) -> TestResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()?;

        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(IoError::other(format!("agent exited before answering {method}")).into());
            }
            let mut message: Value = serde_json::from_str(&line)?;
            if message["id"] != json!(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(IoError::other(format!("{method} failed: {error}")).into());
            }
            return Ok(message["result"].take());
        }
    }
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
