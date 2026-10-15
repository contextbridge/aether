use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::LazyLock;

use regex::{Captures, Regex};

/// Substitute parameters in a prompt template
/// Supports named parameters using the format `$parameter_name`
pub fn substitute_parameters<S: BuildHasher>(template: &str, arguments: &Option<HashMap<String, String, S>>) -> String {
    let Some(args) = arguments else {
        return template.to_string();
    };

    PARAMETER
        .replace_all(template, |caps: &Captures| args.get(&caps[1]).cloned().unwrap_or_else(|| caps[0].to_string()))
        .into_owned()
}

static PARAMETER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(\w+)").expect("valid regex"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_named_parameters() {
        let template = "Review this $language code in $framework";
        let args = HashMap::from([
            ("language".to_string(), "Rust".to_string()),
            ("framework".to_string(), "Actix".to_string()),
        ]);

        let result = substitute_parameters(template, &Some(args));
        assert_eq!(result, "Review this Rust code in Actix");
    }

    #[test]
    fn test_no_arguments() {
        let template = "Hello $name, welcome to $project. Today is $day";
        let none: Option<HashMap<String, String>> = None;
        let result = substitute_parameters(template, &none);
        assert_eq!(result, "Hello $name, welcome to $project. Today is $day");
    }

    #[test]
    fn test_missing_parameter() {
        let template = "Language: $language, Framework: $framework, Database: $database";
        let args = HashMap::from([
            ("language".to_string(), "Rust".to_string()),
            ("framework".to_string(), "Actix".to_string()),
        ]);

        let result = substitute_parameters(template, &Some(args));
        assert_eq!(result, "Language: Rust, Framework: Actix, Database: $database");
    }

    #[test]
    fn test_empty_arguments() {
        let template = "Process $input with $config";
        let args = HashMap::new();
        let result = substitute_parameters(template, &Some(args));
        assert_eq!(result, "Process $input with $config");
    }

    #[test]
    fn test_no_parameters_in_template() {
        let template = "Just a plain string without parameters";
        let args = HashMap::from([("unused".to_string(), "value".to_string())]);
        let result = substitute_parameters(template, &Some(args));
        assert_eq!(result, "Just a plain string without parameters");
    }
}
