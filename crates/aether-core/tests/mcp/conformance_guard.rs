//! Source/fixture conformance guard for the MCP refactor (Phases 2.2-2.4).
//!
//! Prevents reintroduction of protocol behavior that was removed during the
//! migration to MRTR:
//!
//! - the `-32042` `URL_ELICITATION_REQUIRED` error path
//! - the `notifications/elicitation/complete` completion notification
//! - protocol `elicitationId` semantics (wire key, event names)
//! - direct `create_elicitation` calls from the built-in servers
//!
//! The guard scans Aether source and test fixtures only, so rmcp's own
//! internals (which still carry a non-conforming `elicitation_id` field on
//! `UrlElicitationParams`) cannot cause false positives. Aether itself may
//! name the removed id in exactly one narrowly named boundary file
//! (`crates/acp-utils/src/elicitation.rs`), where rmcp 3.0's legacy URL
//! constructor still requires it; everywhere else both the camelCase and
//! snake_case spellings are forbidden. `ClientHandler` `create_elicitation`
//! is asserted to remain intact because rmcp's modern MRTR driver resolves
//! input requests through it.

use std::fs;
use std::path::{Path, PathBuf};

/// The single Aether source file where rmcp 3.0's removed elicitation id may
/// still be named: it converts rmcp's legacy URL elicitation into Aether's
/// id-free wire representation and hosts the fixture constructor that rmcp's
/// non-conforming struct literal forces us to provide.
const ELICITATION_ID_BOUNDARY_FILE: &str = "crates/acp-utils/src/elicitation.rs";

/// The removed protocol id spellings (JSON key, event names, type/field names,
/// and fixture literals). Both the camelCase and snake_case forms are banned
/// in Aether sources; only `ELICITATION_ID_BOUNDARY_FILE` is exempt.
const FORBIDDEN_ID_PATTERNS: &[&str] = &["elicitationId", "elicitation_id"];

const FORBIDDEN_PATTERNS: &[&str] = &[
    // Deleted legacy URL-elicitation error code and its identifier.
    "URL_ELICITATION_REQUIRED",
    "32042",
    // Deleted URL completion notification.
    "notifications/elicitation/complete",
    // Old protocol-shaped completion event/type names.
    "UrlElicitationComplete",
];

/// Direct server-initiated elicitation is forbidden in the built-in servers.
const FORBIDDEN_SERVER_PATTERN: &str = ".create_elicitation(";

/// The client handler must keep resolving elicitation input requests for the
/// MRTR driver; only direct *server* calls were removed.
const PRESERVED_CLIENT_HANDLER_FILE: &str = "crates/mcp-utils/src/client/mcp_client.rs";
const PRESERVED_CLIENT_HANDLER_MARKER: &str = "async fn create_elicitation(";

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.ancestors().nth(2).expect("CARGO_MANIFEST_DIR sits two levels below the workspace root").to_path_buf()
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                // This guard itself names the forbidden patterns so the
                // failure messages stay readable; excluding it keeps the scan
                // from flagging its own definitions.
                if path.file_name().is_some_and(|name| name == "conformance_guard.rs") {
                    continue;
                }
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn aether_source_and_fixtures_do_not_reintroduce_removed_elicitation_protocol() {
    let root = workspace_root();
    let mut violations = Vec::new();

    for file in source_files(&root) {
        let relative = file.strip_prefix(&root).expect("source file under workspace root");
        let content = fs::read_to_string(&file).expect("read source file");
        let is_builtin_server = relative.starts_with("crates/mcp-servers/");

        for pattern in FORBIDDEN_PATTERNS.iter().chain(FORBIDDEN_ID_PATTERNS) {
            if content.contains(pattern) {
                // The boundary file must be able to name the removed id to
                // convert rmcp 3.0's legacy URL elicitation type.
                if relative == Path::new(ELICITATION_ID_BOUNDARY_FILE) && FORBIDDEN_ID_PATTERNS.contains(pattern) {
                    continue;
                }
                violations.push(format!("{} contains forbidden pattern `{pattern}`", relative.display()));
            }
        }
        if is_builtin_server && content.contains(FORBIDDEN_SERVER_PATTERN) {
            violations.push(format!(
                "{} issues a direct server-side create_elicitation call; use MRTR InputRequiredResult instead",
                relative.display()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "removed MCP elicitation protocol behavior was reintroduced:\n{}",
        violations.join("\n")
    );
}

#[test]
fn elicitation_id_boundary_file_exists_so_the_exemption_stays_narrow() {
    let root = workspace_root();
    let boundary = root.join(ELICITATION_ID_BOUNDARY_FILE);
    assert!(
        boundary.is_file(),
        "the single allowed elicitation-id boundary file must exist at {ELICITATION_ID_BOUNDARY_FILE}"
    );
}

#[test]
fn client_handler_create_elicitation_is_preserved_for_the_mrtr_driver() {
    let root = workspace_root();
    let file = root.join(PRESERVED_CLIENT_HANDLER_FILE);
    let content = fs::read_to_string(&file).expect("client handler source exists");
    assert!(
        content.contains(PRESERVED_CLIENT_HANDLER_MARKER),
        "ClientHandler::create_elicitation must remain for rmcp's MRTR input resolution"
    );
}
