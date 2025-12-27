//! File picker dropdown component.
//!
//! Displays a filterable list of files when the user types "@".

use dioxus::prelude::*;
use std::path::PathBuf;

/// Get a file icon based on extension
fn get_file_icon(path: &PathBuf) -> &'static str {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "rs" => "🦀",
        "ts" | "tsx" => "📘",
        "js" | "jsx" => "📒",
        "py" => "🐍",
        "go" => "🐹",
        "java" => "☕",
        "c" | "cpp" | "h" | "hpp" => "⚙️",
        "json" => "📋",
        "yaml" | "yml" => "📄",
        "toml" => "⚙️",
        "md" => "📝",
        "html" => "🌐",
        "css" | "scss" | "sass" => "🎨",
        "sql" => "🗃️",
        "sh" | "bash" | "zsh" => "💻",
        "dockerfile" => "🐳",
        "proto" => "📡",
        _ => "📄",
    }
}

/// Highlight matching parts of a path based on the filter query
fn highlight_path(path: &str, filter: &str) -> Element {
    if filter.is_empty() {
        return rsx! { span { "{path}" } };
    }

    let path_lower = path.to_lowercase();
    let filter_lower = filter.to_lowercase();

    if let Some(start) = path_lower.find(&filter_lower) {
        let before = &path[..start];
        let matched = &path[start..start + filter.len()];
        let after = &path[start + filter.len()..];

        rsx! {
            span { "{before}" }
            span { class: "text-blue-400 font-semibold", "{matched}" }
            span { "{after}" }
        }
    } else {
        rsx! { span { "{path}" } }
    }
}

#[component]
pub fn FilePickerDropdown(
    /// List of file paths to display
    files: Vec<PathBuf>,
    /// Current filter text (text after "@")
    filter: String,
    /// Currently selected index in filtered list
    selected_index: usize,
    /// Called when a file is selected
    on_select: EventHandler<PathBuf>,
) -> Element {
    if files.is_empty() {
        return rsx! {
            div {
                class: "absolute bottom-full left-0 right-0 mb-2 bg-[#1a1d23] border border-[#373b47] rounded-xl shadow-2xl p-4 text-gray-400 text-sm z-50",
                if filter.is_empty() {
                    "Type to search for files..."
                } else {
                    "No matching files"
                }
            }
        };
    }

    // Clamp selected index to valid range
    let selected_index = selected_index.min(files.len().saturating_sub(1));

    rsx! {
        div {
            class: "absolute bottom-full left-0 right-0 mb-2 bg-[#1a1d23] border border-[#373b47] rounded-xl shadow-2xl overflow-hidden max-h-80 overflow-y-auto z-50",

            // Header
            div {
                class: "px-4 py-3 border-b border-[#2d313a] bg-[#252830] text-xs text-gray-500 font-semibold uppercase tracking-wide flex items-center gap-2",
                span { "📁" }
                span { "File Picker" }
                span { class: "text-gray-600 font-normal ml-auto", "@ to search files" }
            }

            // File list
            for (index, file_path) in files.iter().enumerate() {
                FileItem {
                    key: "{file_path:?}",
                    path: file_path.clone(),
                    filter: filter.clone(),
                    is_selected: index == selected_index,
                    on_click: {
                        let path = file_path.clone();
                        move |_| on_select.call(path.clone())
                    },
                }
            }
        }
    }
}

#[component]
fn FileItem(
    path: PathBuf,
    filter: String,
    is_selected: bool,
    on_click: EventHandler<()>,
) -> Element {
    let class_str = if is_selected {
        "px-4 py-2 cursor-pointer transition-colors bg-blue-600 border-l-2 border-blue-400"
    } else {
        "px-4 py-2 cursor-pointer transition-colors hover:bg-[#252830] border-l-2 border-transparent"
    };

    let icon = get_file_icon(&path);
    let path_str = path.to_string_lossy().to_string();

    // Split into directory and filename for display
    let (dir, filename) = if let Some(parent) = path.parent() {
        let parent_str = parent.to_string_lossy();
        let filename = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if parent_str.is_empty() {
            (None, filename)
        } else {
            (Some(format!("{}/", parent_str)), filename)
        }
    } else {
        (None, path_str.clone())
    };

    rsx! {
        div {
            class: "{class_str}",
            onclick: move |_| on_click.call(()),

            div {
                class: "flex items-center gap-2",
                span { class: "text-base", "{icon}" }
                div {
                    class: "flex items-baseline gap-1 min-w-0 overflow-hidden",
                    if let Some(dir_path) = &dir {
                        span {
                            class: "text-gray-500 text-xs font-mono truncate",
                            "{dir_path}"
                        }
                    }
                    span {
                        class: "text-white text-sm font-mono font-medium truncate",
                        {highlight_path(&filename, &filter)}
                    }
                }
            }
        }
    }
}
