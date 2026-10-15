use crate::common::{TestWorkspace, fake_daemon, file_uri, wait_for_workspace_diagnostics, workspace_error_count};
use aether_lspd::LanguageId;

#[tokio::test]
async fn diagnostics_are_pulled_from_pull_only_server() {
    let project = TestWorkspace::new().file("index.ts", "const error = 1;\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::TypeScript).await;

    let diagnostics =
        client.get_diagnostics(Some(file_uri(&project.path("index.ts")))).await.expect("Failed to get diagnostics");

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics.len(), 1);
    assert_eq!(diagnostics[0].diagnostics[0].message, "error token");
}

#[tokio::test]
async fn cancelled_pull_is_retried() {
    let project = TestWorkspace::new().file("index.ts", "// cancel-first-pull\nconst error = 1;\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::TypeScript).await;

    let diagnostics = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");

    assert_eq!(workspace_error_count(&diagnostics), 1, "{diagnostics:?}");
}

#[tokio::test]
async fn server_refresh_request_re_pulls_dependent_files() {
    let project = TestWorkspace::new().file("a.ts", "// depends: b.ts\n").file("b.ts", "export const ok = 1;\n");
    let (_daemon, client) = fake_daemon(project.root(), LanguageId::TypeScript).await;

    let initial = client.get_diagnostics(None).await.expect("Failed to get workspace diagnostics");
    assert_eq!(initial.len(), 2);
    assert_eq!(workspace_error_count(&initial), 0);

    project.write("b.ts", "export const error = 1;\n");

    let refreshed =
        wait_for_workspace_diagnostics(&client, |diagnostics| workspace_error_count(diagnostics) == 2).await;
    let a_uri = file_uri(&project.path("a.ts"));
    let a_diagnostics = refreshed.iter().find(|params| params.uri == a_uri).expect("a.ts diagnostics");
    assert_eq!(a_diagnostics.diagnostics[0].message, "dependency error");
}
