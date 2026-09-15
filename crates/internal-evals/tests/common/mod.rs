//! Shared helpers for the eval tests in this directory.

#![allow(dead_code)]

use aether_evals::Workspace;
use internal_evals::EvalHarnessError;
use std::fs::read_to_string;

pub fn lines(lines: &[&str]) -> String {
    lines.join("\n")
}

pub fn file_contents(lines: &[&str]) -> String {
    format!("{}\n", lines.join("\n"))
}

pub fn read_file(workspace: &Workspace, path: &str) -> Result<String, EvalHarnessError> {
    Ok(read_to_string(workspace.join(path))?)
}
