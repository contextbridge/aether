use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

#[doc = include_str!("docs/language_catalog.md")]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LanguageId {
    Rust,
    Python,
    JavaScript,
    JavaScriptReact,
    TypeScript,
    TypeScriptReact,
    Go,
    Java,
    C,
    Cpp,
    CSharp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    Scala,
    Html,
    Css,
    Json,
    Yaml,
    Toml,
    Markdown,
    Xml,
    Sql,
    ShellScript,
    PlainText,
}

impl LanguageId {
    /// Get the LSP language ID string
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::JavaScriptReact => "javascriptreact",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Swift => "swift",
            Self::Kotlin => "kotlin",
            Self::Scala => "scala",
            Self::Html => "html",
            Self::Css => "css",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Markdown => "markdown",
            Self::Xml => "xml",
            Self::Sql => "sql",
            Self::ShellScript => "shellscript",
            Self::PlainText => "plaintext",
        }
    }

    /// Detect language from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        from_extension(ext)
    }

    /// Detect language from file path.
    ///
    /// Returns `PlainText` for files with no extension or unknown extensions.
    pub fn from_path(path: &Path) -> Self {
        path.extension().and_then(|e| e.to_str()).and_then(Self::from_extension).unwrap_or(Self::PlainText)
    }

    pub fn primary_extension(self) -> Option<&'static str> {
        metadata_for(self).and_then(|metadata| metadata.primary_extension)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum ServerKind {
    RustAnalyzer,
    TypeScriptLanguageServer,
    Pyright,
    Gopls,
    Clangd,
}

impl ServerKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "rust-analyzer",
            Self::TypeScriptLanguageServer => "typescript-language-server",
            Self::Pyright => "pyright-langserver",
            Self::Gopls => "gopls",
            Self::Clangd => "clangd",
        }
    }

    fn env_key(self) -> &'static str {
        match self {
            Self::RustAnalyzer => "RUST_ANALYZER",
            Self::TypeScriptLanguageServer => "TYPESCRIPT_LANGUAGE_SERVER",
            Self::Pyright => "PYRIGHT",
            Self::Gopls => "GOPLS",
            Self::Clangd => "CLANGD",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LanguageMetadata {
    pub id: LanguageId,
    pub primary_extension: Option<&'static str>,
    pub aliases: &'static [&'static str],
    pub extensions: &'static [&'static str],
}

#[derive(Debug, Clone)]
pub struct LspConfig {
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<LanguageId>,
}

impl LspConfig {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into(), args: Vec::new(), languages: Vec::new() }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_languages(mut self, languages: Vec<LanguageId>) -> Self {
        self.languages = languages;
        self
    }
}

#[derive(Clone, Copy)]
struct ServerSpec {
    kind: ServerKind,
    command: &'static str,
    args: &'static [&'static str],
}

#[derive(Clone, Copy)]
struct LanguageSpec {
    metadata: LanguageMetadata,
    server_kind: Option<ServerKind>,
}

const SERVER_SPECS: &[ServerSpec] = &[
    ServerSpec { kind: ServerKind::RustAnalyzer, command: "rust-analyzer", args: &[] },
    ServerSpec {
        kind: ServerKind::TypeScriptLanguageServer,
        command: "typescript-language-server",
        args: &["--stdio"],
    },
    ServerSpec { kind: ServerKind::Pyright, command: "pyright-langserver", args: &["--stdio"] },
    ServerSpec { kind: ServerKind::Gopls, command: "gopls", args: &[] },
    ServerSpec { kind: ServerKind::Clangd, command: "clangd", args: &[] },
];

const LANGUAGE_SPECS: &[LanguageSpec] = &[
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Rust,
            primary_extension: Some("rs"),
            aliases: &["rust", "rs"],
            extensions: &["rs"],
        },
        server_kind: Some(ServerKind::RustAnalyzer),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Python,
            primary_extension: Some("py"),
            aliases: &["python", "py"],
            extensions: &["py", "pyi", "pyw"],
        },
        server_kind: Some(ServerKind::Pyright),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::JavaScript,
            primary_extension: Some("js"),
            aliases: &["javascript", "js"],
            extensions: &["js", "mjs"],
        },
        server_kind: Some(ServerKind::TypeScriptLanguageServer),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::JavaScriptReact,
            primary_extension: Some("jsx"),
            aliases: &["javascript", "js", "javascriptreact", "jsx"],
            extensions: &["jsx"],
        },
        server_kind: Some(ServerKind::TypeScriptLanguageServer),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::TypeScript,
            primary_extension: Some("ts"),
            aliases: &["typescript", "ts"],
            extensions: &["ts"],
        },
        server_kind: Some(ServerKind::TypeScriptLanguageServer),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::TypeScriptReact,
            primary_extension: Some("tsx"),
            aliases: &["typescript", "ts", "typescriptreact", "tsx"],
            extensions: &["tsx"],
        },
        server_kind: Some(ServerKind::TypeScriptLanguageServer),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Go,
            primary_extension: Some("go"),
            aliases: &["go"],
            extensions: &["go"],
        },
        server_kind: Some(ServerKind::Gopls),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Java,
            primary_extension: Some("java"),
            aliases: &["java"],
            extensions: &["java"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::C,
            primary_extension: Some("c"),
            aliases: &["c"],
            extensions: &["c", "h"],
        },
        server_kind: Some(ServerKind::Clangd),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Cpp,
            primary_extension: Some("cpp"),
            aliases: &["cpp", "c++"],
            extensions: &["cpp", "cxx", "cc", "hpp", "hxx", "hh"],
        },
        server_kind: Some(ServerKind::Clangd),
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::CSharp,
            primary_extension: Some("cs"),
            aliases: &["csharp", "cs"],
            extensions: &["cs"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Ruby,
            primary_extension: Some("rb"),
            aliases: &["ruby", "rb"],
            extensions: &["rb"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Php,
            primary_extension: Some("php"),
            aliases: &["php"],
            extensions: &["php"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Swift,
            primary_extension: Some("swift"),
            aliases: &["swift"],
            extensions: &["swift"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Kotlin,
            primary_extension: Some("kt"),
            aliases: &["kotlin"],
            extensions: &["kt", "kts"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Scala,
            primary_extension: Some("scala"),
            aliases: &["scala"],
            extensions: &["scala"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Html,
            primary_extension: Some("html"),
            aliases: &["html"],
            extensions: &["html", "htm"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Css,
            primary_extension: Some("css"),
            aliases: &["css"],
            extensions: &["css"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Json,
            primary_extension: Some("json"),
            aliases: &["json"],
            extensions: &["json"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Yaml,
            primary_extension: Some("yaml"),
            aliases: &["yaml", "yml"],
            extensions: &["yaml", "yml"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Toml,
            primary_extension: Some("toml"),
            aliases: &["toml"],
            extensions: &["toml"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Markdown,
            primary_extension: Some("md"),
            aliases: &["markdown", "md"],
            extensions: &["md", "markdown"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Xml,
            primary_extension: Some("xml"),
            aliases: &["xml"],
            extensions: &["xml"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::Sql,
            primary_extension: Some("sql"),
            aliases: &["sql"],
            extensions: &["sql"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::ShellScript,
            primary_extension: Some("sh"),
            aliases: &["sh", "shell", "bash"],
            extensions: &["sh", "bash", "zsh"],
        },
        server_kind: None,
    },
    LanguageSpec {
        metadata: LanguageMetadata {
            id: LanguageId::PlainText,
            primary_extension: None,
            aliases: &["plaintext", "text", "txt"],
            extensions: &["txt"],
        },
        server_kind: None,
    },
];

pub static LANGUAGE_METADATA: LazyLock<Vec<LanguageMetadata>> =
    LazyLock::new(|| LANGUAGE_SPECS.iter().map(|spec| spec.metadata).collect());

static CONFIG_MAP: LazyLock<HashMap<LanguageId, LspConfig>> = LazyLock::new(|| {
    let languages_by_server: HashMap<ServerKind, Vec<LanguageId>> = LANGUAGE_SPECS
        .iter()
        .filter_map(|spec| spec.server_kind.map(|kind| (kind, spec.metadata.id)))
        .fold(HashMap::new(), |mut acc, (kind, id)| {
            acc.entry(kind).or_default().push(id);
            acc
        });

    LANGUAGE_SPECS
        .iter()
        .filter_map(|spec| {
            let server_kind = spec.server_kind?;
            let server = SERVER_SPECS.iter().find(|server| server.kind == server_kind)?;
            Some((
                spec.metadata.id,
                LspConfig::new(server.command)
                    .with_args(server.args.iter().map(|arg| (*arg).to_string()).collect())
                    .with_languages(languages_by_server.get(&server_kind).cloned().unwrap_or_default()),
            ))
        })
        .collect()
});

pub(crate) fn server_kind_for_language(id: LanguageId) -> Option<ServerKind> {
    LANGUAGE_SPECS.iter().find(|spec| spec.metadata.id == id).and_then(|spec| spec.server_kind)
}

pub(crate) fn socket_identity_for_language(id: LanguageId) -> &'static str {
    server_kind_for_language(id).map_or_else(|| id.as_str(), ServerKind::as_str)
}

pub(crate) fn resolved_config_for_language(language: LanguageId) -> Option<LspConfig> {
    let mut config = get_config_for_language(language)?.clone();
    let server_kind = server_kind_for_language(language)?;

    let command_key = format!("AETHER_LSPD_SERVER_COMMAND_{}", server_kind.env_key());
    if let Some(command) = std::env::var_os(command_key) {
        config.command = command.to_string_lossy().into_owned();
    }

    let args_key = format!("AETHER_LSPD_SERVER_ARGS_{}", server_kind.env_key());
    if let Ok(args) = std::env::var(args_key)
        && let Ok(parsed) = serde_json::from_str::<Vec<String>>(&args)
    {
        config.args = parsed;
    }

    Some(config)
}

pub(crate) fn from_extension(ext: &str) -> Option<LanguageId> {
    LANGUAGE_SPECS.iter().find(|spec| spec.metadata.extensions.contains(&ext)).map(|spec| spec.metadata.id)
}

pub fn metadata_for(id: LanguageId) -> Option<&'static LanguageMetadata> {
    LANGUAGE_METADATA.iter().find(|metadata| metadata.id == id)
}

pub fn from_lsp_id(lsp_id: &str) -> Option<LanguageId> {
    LANGUAGE_SPECS.iter().find(|spec| spec.metadata.id.as_str() == lsp_id).map(|spec| spec.metadata.id)
}

pub fn extensions_for_alias(alias: &str) -> Vec<&'static str> {
    let lower = alias.to_lowercase();
    LANGUAGE_SPECS
        .iter()
        .filter(|spec| spec.metadata.aliases.iter().any(|candidate| *candidate == lower))
        .flat_map(|spec| spec.metadata.extensions.iter().copied())
        .collect()
}

pub fn get_config_for_language(language: LanguageId) -> Option<&'static LspConfig> {
    CONFIG_MAP.get(&language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_family_shares_server_kind() {
        assert_eq!(
            server_kind_for_language(LanguageId::TypeScript),
            server_kind_for_language(LanguageId::TypeScriptReact)
        );
        assert_eq!(
            socket_identity_for_language(LanguageId::TypeScript),
            socket_identity_for_language(LanguageId::TypeScriptReact)
        );
    }

    #[test]
    fn c_family_shares_server_kind() {
        assert_eq!(server_kind_for_language(LanguageId::C), server_kind_for_language(LanguageId::Cpp));
        assert_eq!(socket_identity_for_language(LanguageId::C), socket_identity_for_language(LanguageId::Cpp));
    }

    #[test]
    fn metadata_for_returns_correct_data() {
        let meta = metadata_for(LanguageId::Rust).unwrap();
        assert_eq!(meta.id.as_str(), "rust");
        assert_eq!(meta.primary_extension, Some("rs"));
        assert!(meta.aliases.contains(&"rust"));
        assert!(meta.aliases.contains(&"rs"));
    }

    #[test]
    fn from_lsp_id_resolves_known_languages() {
        assert_eq!(from_lsp_id("rust"), Some(LanguageId::Rust));
        assert_eq!(from_lsp_id("typescriptreact"), Some(LanguageId::TypeScriptReact));
        assert_eq!(from_lsp_id("unknown"), None);
    }

    #[test]
    fn primary_extension_delegates_to_catalog() {
        assert_eq!(LanguageId::Rust.primary_extension(), Some("rs"));
        assert_eq!(LanguageId::Python.primary_extension(), Some("py"));
        assert_eq!(LanguageId::PlainText.primary_extension(), None);
    }

    #[test]
    fn extensions_for_alias_includes_related_variants() {
        let js_exts = extensions_for_alias("javascript");
        assert!(js_exts.contains(&"js"));
        assert!(js_exts.contains(&"mjs"));
        assert!(js_exts.contains(&"jsx"));

        let ts_exts = extensions_for_alias("typescript");
        assert!(ts_exts.contains(&"ts"));
        assert!(ts_exts.contains(&"tsx"));

        let sh_exts = extensions_for_alias("bash");
        assert!(sh_exts.contains(&"sh"));
        assert!(sh_exts.contains(&"bash"));
        assert!(sh_exts.contains(&"zsh"));
    }

    #[test]
    fn all_languages_have_metadata() {
        let variants = [
            LanguageId::Rust,
            LanguageId::Python,
            LanguageId::JavaScript,
            LanguageId::JavaScriptReact,
            LanguageId::TypeScript,
            LanguageId::TypeScriptReact,
            LanguageId::Go,
            LanguageId::Java,
            LanguageId::C,
            LanguageId::Cpp,
            LanguageId::CSharp,
            LanguageId::Ruby,
            LanguageId::Php,
            LanguageId::Swift,
            LanguageId::Kotlin,
            LanguageId::Scala,
            LanguageId::Html,
            LanguageId::Css,
            LanguageId::Json,
            LanguageId::Yaml,
            LanguageId::Toml,
            LanguageId::Markdown,
            LanguageId::Xml,
            LanguageId::Sql,
            LanguageId::ShellScript,
            LanguageId::PlainText,
        ];

        for variant in variants {
            assert!(metadata_for(variant).is_some(), "Missing metadata for {variant:?}");
        }
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
    fn lsp_config_builder() {
        let config = LspConfig::new("test-lsp")
            .with_args(vec!["--stdio".to_string(), "--debug".to_string()])
            .with_languages(vec![LanguageId::Rust]);

        assert_eq!(config.command, "test-lsp");
        assert_eq!(config.args, vec!["--stdio", "--debug"]);
        assert_eq!(config.languages, vec![LanguageId::Rust]);
    }

    #[test]
    fn get_config_for_known_languages() {
        let rust_config = get_config_for_language(LanguageId::Rust);
        assert!(rust_config.is_some());
        assert_eq!(rust_config.unwrap().command, "rust-analyzer");

        let ts_config = get_config_for_language(LanguageId::TypeScript);
        assert!(ts_config.is_some());
        assert_eq!(ts_config.unwrap().command, "typescript-language-server");

        let plaintext_config = get_config_for_language(LanguageId::PlainText);
        assert!(plaintext_config.is_none());
    }
}
