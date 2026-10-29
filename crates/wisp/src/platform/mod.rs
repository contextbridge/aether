pub mod terminal;

use crossterm::execute;
use crossterm::style::Print;
use std::io;
use std::sync::Arc;

pub(crate) type BrowserOpener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub(crate) type ClipboardWriter = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub(crate) type Bell = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct Platform {
    pub browser_opener: BrowserOpener,
    pub clipboard_writer: ClipboardWriter,
    pub bell: Bell,
}

impl Platform {
    pub fn system() -> Self {
        Self {
            browser_opener: Arc::new(open_url),
            clipboard_writer: Arc::new(write_clipboard),
            bell: Arc::new(|| execute!(io::stdout(), Print("\x07")).map_err(|error| error.to_string())),
        }
    }
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> Result<(), String> {
    run("open", &[url])
}

#[cfg(target_os = "linux")]
fn open_url(url: &str) -> Result<(), String> {
    run("xdg-open", &[url])
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> Result<(), String> {
    run("cmd", &["/C", "start", "", url])
}

// Every other platform falls back to the caller's copy-URL affordance.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn open_url(_url: &str) -> Result<(), String> {
    Err("Unsupported platform for opening URLs".to_string())
}

#[cfg(target_os = "macos")]
fn write_clipboard(text: &str) -> Result<(), String> {
    pipe_to("pbcopy", &[], text)
}

#[cfg(target_os = "linux")]
fn write_clipboard(text: &str) -> Result<(), String> {
    pipe_to("wl-copy", &[], text)
        .or_else(|_| pipe_to("xclip", &["-selection", "clipboard"], text))
        .or_else(|_| pipe_to("xsel", &["--clipboard", "--input"], text))
        .map_err(|_| "No clipboard tool found (wl-copy, xclip, or xsel)".to_string())
}

#[cfg(target_os = "windows")]
fn write_clipboard(text: &str) -> Result<(), String> {
    pipe_to("clip", &[], text)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn write_clipboard(_text: &str) -> Result<(), String> {
    Err("Unsupported platform for copying text".to_string())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn run(command: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(command)
        .args(args)
        .status()
        .map_err(|error| format!("Failed to spawn '{command}': {error}"))?;
    exit_ok(command, status)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn pipe_to(command: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("Failed to spawn '{command}': {error}"))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("'{command}' has no stdin"))?
        .write_all(text.as_bytes())
        .map_err(|error| format!("Failed to write to '{command}': {error}"))?;
    let status = child.wait().map_err(|error| format!("Failed to wait for '{command}': {error}"))?;
    exit_ok(command, status)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn exit_ok(command: &str, status: std::process::ExitStatus) -> Result<(), String> {
    status.success().then_some(()).ok_or_else(|| format!("'{command}' exited with status {status}"))
}
