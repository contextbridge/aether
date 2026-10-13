use serde::{Deserialize, Serialize};
use std::path::Path;

macro_rules! languages {
    ($($variant:ident => $lsp_id:literal, [$($extension:literal),*], $server:expr;)*) => {
        #[doc = include_str!("docs/language_catalog.md")]
        #[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
        pub enum LanguageId {
            $($variant,)*
        }

        pub const LANGUAGES: &[LanguageMetadata] = &[
            $(LanguageMetadata {
                id: LanguageId::$variant,
                lsp_id: $lsp_id,
                extensions: &[$($extension),*],
                server: $server,
            },)*
        ];
    };
}

languages! {
    Rust => "rust", ["rs"], Some(&RUST_ANALYZER);
    Python => "python", ["py", "pyi", "pyw"], Some(&PYRIGHT);
    JavaScript => "javascript", ["js", "mjs"], Some(&TSC);
    JavaScriptReact => "javascriptreact", ["jsx"], Some(&TSC);
    TypeScript => "typescript", ["ts"], Some(&TSC);
    TypeScriptReact => "typescriptreact", ["tsx"], Some(&TSC);
    Go => "go", ["go"], Some(&GOPLS);
    Java => "java", ["java"], None;
    C => "c", ["c", "h"], Some(&CLANGD);
    Cpp => "cpp", ["cpp", "cxx", "cc", "hpp", "hxx", "hh"], Some(&CLANGD);
    CSharp => "csharp", ["cs"], None;
    Ruby => "ruby", ["rb"], None;
    Php => "php", ["php"], None;
    Swift => "swift", ["swift"], None;
    Kotlin => "kotlin", ["kt", "kts"], None;
    Scala => "scala", ["scala"], None;
    Html => "html", ["html", "htm"], None;
    Css => "css", ["css"], None;
    Json => "json", ["json"], None;
    Yaml => "yaml", ["yaml", "yml"], None;
    Toml => "toml", ["toml"], None;
    Markdown => "markdown", ["md", "markdown"], None;
    Xml => "xml", ["xml"], None;
    Sql => "sql", ["sql"], None;
    ShellScript => "shellscript", ["sh", "bash", "zsh"], None;
    PlainText => "plaintext", ["txt"], None;
}

impl LanguageId {
    /// Get the LSP language ID string
    pub fn as_str(self) -> &'static str {
        self.metadata().lsp_id
    }

    pub fn metadata(self) -> &'static LanguageMetadata {
        &LANGUAGES[self as usize]
    }

    pub fn server(self) -> Option<&'static ServerSpec> {
        self.metadata().server
    }

    /// Detect language from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        LANGUAGES.iter().find(|language| language.extensions.contains(&ext)).map(|language| language.id)
    }

    /// Detect language from file path.
    ///
    /// Returns `PlainText` for files with no extension or unknown extensions.
    pub fn from_path(path: &Path) -> Self {
        path.extension().and_then(|e| e.to_str()).and_then(Self::from_extension).unwrap_or(Self::PlainText)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LanguageMetadata {
    pub id: LanguageId,
    pub lsp_id: &'static str,
    pub extensions: &'static [&'static str],
    pub server: Option<&'static ServerSpec>,
}

#[derive(Debug)]
pub struct ServerSpec {
    pub name: &'static str,
    pub display_name: &'static str,
    pub installation_instructions: Option<&'static str>,
    pub(crate) env_key: &'static str,
    args: &'static [&'static str],
    diagnostics_mode: DiagnosticsMode,
}

pub fn detect_project_languages(root: &Path) -> Vec<LanguageId> {
    PROJECT_LANGUAGE_SPECS
        .iter()
        .filter(|(_, manifests)| manifests.iter().any(|manifest| root.join(manifest).is_file()))
        .map(|(language, _)| *language)
        .collect()
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum DiagnosticsMode {
    Push,
    Pull,
}

#[derive(Debug, Clone)]
pub(crate) struct LspConfig {
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<LanguageId>,
    pub diagnostics_mode: DiagnosticsMode,
}

pub(crate) fn socket_identity_for_language(id: LanguageId) -> &'static str {
    id.server().map_or_else(|| id.as_str(), |server| server.name)
}

pub(crate) fn resolved_config_for_language(language: LanguageId) -> Option<LspConfig> {
    let server = language.server()?;
    let command = std::env::var_os(format!("AETHER_LSPD_SERVER_COMMAND_{}", server.env_key))
        .map_or_else(|| server.name.to_string(), |command| command.to_string_lossy().into_owned());
    let args = std::env::var(format!("AETHER_LSPD_SERVER_ARGS_{}", server.env_key))
        .ok()
        .and_then(|args| serde_json::from_str(&args).ok())
        .unwrap_or_else(|| server.args.iter().map(ToString::to_string).collect());
    let languages = LANGUAGES
        .iter()
        .filter(|candidate| candidate.server.is_some_and(|candidate| candidate.name == server.name))
        .map(|candidate| candidate.id)
        .collect();

    Some(LspConfig { command, args, languages, diagnostics_mode: server.diagnostics_mode })
}

const RUST_ANALYZER: ServerSpec = ServerSpec {
    name: "rust-analyzer",
    display_name: "rust-analyzer",
    installation_instructions: None,
    env_key: "RUST_ANALYZER",
    args: &[],
    diagnostics_mode: DiagnosticsMode::Push,
};

const TSC: ServerSpec = ServerSpec {
    name: "tsc",
    display_name: "TypeScript 7 language server",
    installation_instructions: Some(
        "It requires TypeScript 7 or newer. Install it in this workspace with \
         `npm install --save-dev typescript@^7`, or install it globally with \
         `npm install --global typescript@^7`, then retry.",
    ),
    env_key: "TSC",
    args: &["--lsp", "--stdio"],
    diagnostics_mode: DiagnosticsMode::Pull,
};

const PYRIGHT: ServerSpec = ServerSpec {
    name: "pyright-langserver",
    display_name: "Pyright language server",
    installation_instructions: None,
    env_key: "PYRIGHT",
    args: &["--stdio"],
    diagnostics_mode: DiagnosticsMode::Push,
};

const GOPLS: ServerSpec = ServerSpec {
    name: "gopls",
    display_name: "gopls language server",
    installation_instructions: None,
    env_key: "GOPLS",
    args: &[],
    diagnostics_mode: DiagnosticsMode::Push,
};

const CLANGD: ServerSpec = ServerSpec {
    name: "clangd",
    display_name: "clangd language server",
    installation_instructions: None,
    env_key: "CLANGD",
    args: &[],
    diagnostics_mode: DiagnosticsMode::Push,
};

const PROJECT_LANGUAGE_SPECS: &[(LanguageId, &[&str])] = &[
    (LanguageId::Rust, &["Cargo.toml"]),
    (LanguageId::TypeScript, &["package.json"]),
    (LanguageId::Python, &["pyproject.toml", "setup.py", "requirements.txt"]),
    (LanguageId::Go, &["go.mod"]),
    (LanguageId::Cpp, &["CMakeLists.txt"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_project_languages_from_manifests() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Cargo.toml"), "").unwrap();
        std::fs::write(root.path().join("package.json"), "{}").unwrap();

        assert_eq!(detect_project_languages(root.path()), vec![LanguageId::Rust, LanguageId::TypeScript]);
    }

    #[test]
    fn typescript_server_has_installation_instructions() {
        let server = LanguageId::TypeScript.server().unwrap();

        assert_eq!(server.display_name, "TypeScript 7 language server");
        assert!(server.installation_instructions.unwrap().contains("npm install --save-dev typescript@^7"));
    }

    #[test]
    fn languages_sharing_a_server_share_a_socket_identity() {
        assert_eq!(socket_identity_for_language(LanguageId::TypeScript), "tsc");
        assert_eq!(socket_identity_for_language(LanguageId::TypeScriptReact), "tsc");
        assert_eq!(socket_identity_for_language(LanguageId::C), "clangd");
        assert_eq!(socket_identity_for_language(LanguageId::Cpp), "clangd");
        assert_eq!(socket_identity_for_language(LanguageId::Java), "java");
    }

    #[test]
    fn metadata_returns_the_language_row() {
        for language in LANGUAGES {
            assert_eq!(language.id.metadata().id, language.id);
        }
        assert_eq!(LanguageId::Rust.metadata().extensions, &["rs"]);
    }

    #[test]
    fn language_id_as_str() {
        assert_eq!(LanguageId::Rust.as_str(), "rust");
        assert_eq!(LanguageId::TypeScriptReact.as_str(), "typescriptreact");
    }

    #[test]
    fn language_id_from_extension() {
        assert_eq!(LanguageId::from_extension("rs"), Some(LanguageId::Rust));
        assert_eq!(LanguageId::from_extension("tsx"), Some(LanguageId::TypeScriptReact));
        assert_eq!(LanguageId::from_extension("xyz"), None);
    }

    #[test]
    fn language_id_from_path() {
        assert_eq!(LanguageId::from_path(Path::new("foo.rs")), LanguageId::Rust);
        assert_eq!(LanguageId::from_path(Path::new("bar.py")), LanguageId::Python);
        assert_eq!(LanguageId::from_path(Path::new("baz.tsx")), LanguageId::TypeScriptReact);
        assert_eq!(LanguageId::from_path(Path::new("unknown.xyz")), LanguageId::PlainText);
        assert_eq!(LanguageId::from_path(Path::new("no_extension")), LanguageId::PlainText);
    }

    #[test]
    fn resolved_config_groups_languages_by_server() {
        assert_eq!(resolved_config_for_language(LanguageId::Rust).unwrap().command, "rust-analyzer");

        let ts_config = resolved_config_for_language(LanguageId::TypeScript).unwrap();
        assert_eq!(ts_config.command, "tsc");
        assert_eq!(ts_config.args, ["--lsp", "--stdio"]);
        assert_eq!(ts_config.diagnostics_mode, DiagnosticsMode::Pull);
        assert_eq!(
            ts_config.languages,
            [LanguageId::JavaScript, LanguageId::JavaScriptReact, LanguageId::TypeScript, LanguageId::TypeScriptReact]
        );

        assert!(resolved_config_for_language(LanguageId::PlainText).is_none());
    }
}
