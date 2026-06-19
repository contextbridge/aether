//! Test helpers for the fenced examples embedded in `src/docs/*.md` files.
//!
//! Those markdown files are the single source for both rustdoc (via
//! `#[doc = include_str!(...)]`) and the published settings reference, so the
//! `json` examples inside them must deserialize into the types they document.
//! `cargo test --doc` already compiles `rust` examples; this crate gives the
//! same guarantee to `json` config examples, which rustdoc cannot compile.

use serde::de::DeserializeOwned;

/// Asserts every ` ```json ` block in `markdown` deserializes into `T`.
///
/// `file` is used only for failure messages. Panics if the document contains
/// no `json` blocks, so a doc page that loses its examples fails loudly.
pub fn assert_json_examples<T: DeserializeOwned>(file: &str, markdown: &str) {
    let blocks = code_blocks(markdown, "json");
    assert!(!blocks.is_empty(), "{file}: no ```json examples found");
    for (index, block) in blocks.iter().enumerate() {
        if let Err(error) = serde_json::from_str::<T>(block) {
            panic!("{file} example {index} failed to parse: {error}\n{block}");
        }
    }
}

/// Returns the body of every fenced code block tagged with `lang`.
///
/// Matches the opening fence by its info string's first token, so
/// ` ```json title="settings.json" ` is collected for `lang == "json"`.
pub fn code_blocks(markdown: &str, lang: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        if fence_lang(line) == Some(lang) {
            let mut block = String::new();
            for line in lines.by_ref() {
                if line.trim_start().starts_with("```") {
                    break;
                }
                block.push_str(line);
                block.push('\n');
            }
            blocks.push(block);
        }
    }
    blocks
}

fn fence_lang(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix("```").map(|info| info.split_whitespace().next().unwrap_or(""))
}
