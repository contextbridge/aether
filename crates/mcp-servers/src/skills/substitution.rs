use regex::{Captures, Regex};
use std::collections::HashMap;
use std::sync::LazyLock;

pub(super) fn substitute_parameters(template: &str, arguments: &HashMap<String, String>) -> String {
    PARAMETER
        .replace_all(template, |caps: &Captures| {
            arguments.get(&caps[1]).cloned().unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

static PARAMETER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(\w+)").expect("valid regex"));
