use crate::common::{DaemonHarness, TestProject, use_crashing_native_ts_server};
use aether_lspd::LanguageId;
use std::path::Path;

struct TsProject(tempfile::TempDir);

impl TestProject for TsProject {
    fn root(&self) -> &Path {
        self.0.path()
    }
}

#[tokio::test]
async fn legacy_server_serves_diagnostics_when_native_tsc_dies_at_initialize() {
    use_crashing_native_ts_server();

    let project = TsProject(tempfile::tempdir().expect("Failed to create temp directory"));
    project.add_file("index.ts", "const value: string = \"error\";\n").expect("Failed to add source file");

    let harness = DaemonHarness::spawn(project.root(), LanguageId::TypeScript).await.expect("Failed to spawn daemon");
    let client = harness.connect().await.expect("Failed to connect client");

    let diagnostics =
        client.get_diagnostics(Some(project.file_uri("index.ts"))).await.expect("Failed to get diagnostics");

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");

    harness.kill().await.expect("Failed to kill daemon");
}
