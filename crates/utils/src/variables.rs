use regex::{Captures, Regex};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::sync::{Arc, LazyLock};
use thiserror::Error;

type GetEnvVarFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A small key→value lookup used when expanding `$VAR` / `${VAR}` references.
///
/// `Vars` is checked before falling back to an env lookup (the process
/// environment by default), so callers can inject locally-scoped values
/// (e.g. `WORKSPACE`) without leaking them into global state.
#[derive(Clone)]
pub struct Vars {
    map: BTreeMap<String, String>,
    get_env_var: GetEnvVarFn,
}

impl fmt::Debug for Vars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vars").field("map", &self.map).finish_non_exhaustive()
    }
}

impl Default for Vars {
    fn default() -> Self {
        Self::new()
    }
}

impl Vars {
    pub fn new() -> Self {
        Self { map: BTreeMap::new(), get_env_var: Arc::new(|k| env::var(k).ok()) }
    }

    /// Override the env-var fallback. Useful in tests to avoid mutating the
    /// process environment.
    pub fn with_env_lookup(mut self, f: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.get_env_var = Arc::new(f);
        self
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.insert(key, value);
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.map.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<Cow<'_, str>> {
        if let Some(value) = self.map.get(key) {
            Some(Cow::Borrowed(value.as_str()))
        } else {
            (self.get_env_var)(key).map(Cow::Owned)
        }
    }

    /// Expand `$VAR` / `${VAR}` references in `template`, consulting `self` before
    /// the env lookup. `$$` escapes to a literal `$`.
    pub fn expand(&self, template: &str) -> Result<String, VarError> {
        let mut missing = None;
        let expanded = REFERENCE.replace_all(template, |caps: &Captures| {
            let Some(name) = var_name(caps) else {
                return "$".to_string();
            };
            self.get(name).map_or_else(
                || {
                    missing.get_or_insert_with(|| name.to_string());
                    caps[0].to_string()
                },
                Cow::into_owned,
            )
        });
        match missing {
            Some(var) => Err(VarError::NotFound(var)),
            None => Ok(expanded.into_owned()),
        }
    }

    /// Returns `true` if `s` contains a `$VAR` or `${VAR}` style reference.
    pub fn has_reference(s: &str) -> bool {
        REFERENCE.captures_iter(s).any(|caps| var_name(&caps).is_some())
    }
}

#[derive(Debug, Error)]
pub enum VarError {
    #[error("Environment variable '{0}' not found")]
    NotFound(String),
}

static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$(?:\$|\{([^}]+)\}|([A-Za-z_][A-Za-z0-9_]*))").expect("valid regex"));

fn var_name<'a>(caps: &Captures<'a>) -> Option<&'a str> {
    caps.get(1).or_else(|| caps.get(2)).map(|name| name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env() -> Vars {
        Vars::new().with_env_lookup(|_| None)
    }

    #[test]
    fn test_simple_var() {
        let vars = no_env().with("TEST_VAR_SIMPLE", "hello");
        let result = vars.expand("$TEST_VAR_SIMPLE world").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_bracketed_var() {
        let vars = no_env().with("TEST_VAR_BRACKET", "hello");
        let result = vars.expand("${TEST_VAR_BRACKET} world").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_escape_sequence() {
        let result = no_env().expand("$$VAR").unwrap();
        assert_eq!(result, "$VAR");
    }

    #[test]
    fn test_multiple_vars() {
        let vars = no_env().with("VAR1", "hello").with("VAR2", "world");
        let result = vars.expand("$VAR1 ${VAR2}!").unwrap();
        assert_eq!(result, "hello world!");
    }

    #[test]
    fn test_missing_var() {
        let result = no_env().expand("$MISSING_VAR");
        assert!(matches!(result, Err(VarError::NotFound(ref name)) if name == "MISSING_VAR"));
    }

    #[test]
    fn test_unclosed_brace_left_as_is() {
        let result = no_env().expand("${VAR").unwrap();
        assert_eq!(result, "${VAR");
    }

    #[test]
    fn test_empty_string() {
        let result = no_env().expand("").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_no_vars() {
        let result = no_env().expand("plain text").unwrap();
        assert_eq!(result, "plain text");
    }

    #[test]
    fn test_dollar_at_end() {
        let result = no_env().expand("text$").unwrap();
        assert_eq!(result, "text$");
    }

    #[test]
    fn test_var_with_underscore() {
        let vars = no_env().with("MY_TEST_VAR", "value");
        let result = vars.expand("$MY_TEST_VAR").unwrap();
        assert_eq!(result, "value");
    }

    #[test]
    fn test_var_with_numbers() {
        let vars = no_env().with("VAR123", "value");
        let result = vars.expand("$VAR123").unwrap();
        assert_eq!(result, "value");
    }

    #[test]
    fn test_special_char_stops_var_name() {
        let vars = no_env().with("VAR", "value");
        let result = vars.expand("$VAR-suffix").unwrap();
        assert_eq!(result, "value-suffix");
    }

    #[test]
    fn vars_lookup_takes_precedence_over_env() {
        let vars = Vars::new()
            .with_env_lookup(|k| (k == "WORKSPACE").then(|| "/from-env".into()))
            .with("WORKSPACE", "/from-vars");
        let result = vars.expand("${WORKSPACE}/foo").unwrap();
        assert_eq!(result, "/from-vars/foo");
    }

    #[test]
    fn vars_falls_through_to_env_when_missing() {
        let vars = Vars::new().with_env_lookup(|k| (k == "ONLY_IN_ENV").then(|| "from-env".into()));
        let result = vars.expand("$ONLY_IN_ENV").unwrap();
        assert_eq!(result, "from-env");
    }

    #[test]
    fn substituted_values_are_not_expanded_again() {
        let vars = no_env().with("A", "$B").with("B", "expanded twice");
        assert_eq!(vars.expand("${A} $A").unwrap(), "$B $B");
    }

    #[test]
    fn has_reference_ignores_escaped_dollars() {
        assert!(!Vars::has_reference("$$HOME"));
    }

    #[test]
    fn has_reference_detects_both_forms() {
        assert!(Vars::has_reference("${WORKSPACE}/foo"));
        assert!(Vars::has_reference("$HOME/bar"));
        assert!(!Vars::has_reference("plain/path"));
        assert!(!Vars::has_reference("$"));
        assert!(!Vars::has_reference("$$"));
    }
}
