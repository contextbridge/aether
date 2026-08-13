/// Matches an exact name or a prefix pattern ending in `*`.
pub fn matches_name_pattern(pattern: &str, name: &str) -> bool {
    pattern.strip_suffix('*').map_or_else(|| pattern == name, |prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::matches_name_pattern;

    #[test]
    fn matches_exact_names_and_trailing_prefix_wildcards() {
        assert!(matches_name_pattern("bash", "bash"));
        assert!(!matches_name_pattern("bash", "bash_extra"));
        assert!(matches_name_pattern("lsp_*", "lsp_hover"));
        assert!(matches_name_pattern("*", "anything"));
        assert!(!matches_name_pattern("lsp_*_tool", "lsp_hover_tool"));
    }
}
