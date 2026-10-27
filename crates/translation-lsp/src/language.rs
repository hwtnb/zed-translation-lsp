//! The languages that `extension.toml` registers for the server.

/// A language that `extension.toml` registers, named by its language server ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Language {
    Rust,
    JavaScript,
    TypeScript,
    TypeScriptReact,
    CSharp,
    C,
    Cpp,
    Python,
    Go,
    Java,
    Kotlin,
    Swift,
    Ruby,
    Php,
    Html,
    Css,
    Sql,
    Json,
    Jsonc,
    Yaml,
    Toml,
    ShellScript,
    PowerShell,
    Dockerfile,
    DockerCompose,
    Markdown,
    PlainText,
}

impl Language {
    /// Every language, in the order of `extension.toml`.
    pub const ALL: [Self; 27] = [
        Self::Rust,
        Self::JavaScript,
        Self::TypeScript,
        Self::TypeScriptReact,
        Self::CSharp,
        Self::C,
        Self::Cpp,
        Self::Python,
        Self::Go,
        Self::Java,
        Self::Kotlin,
        Self::Swift,
        Self::Ruby,
        Self::Php,
        Self::Html,
        Self::Css,
        Self::Sql,
        Self::Json,
        Self::Jsonc,
        Self::Yaml,
        Self::Toml,
        Self::ShellScript,
        Self::PowerShell,
        Self::Dockerfile,
        Self::DockerCompose,
        Self::Markdown,
        Self::PlainText,
    ];

    /// The language ID that Zed sends and that settings name.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::CSharp => "csharp",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::Kotlin => "kotlin",
            Self::Swift => "swift",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Html => "html",
            Self::Css => "css",
            Self::Sql => "sql",
            Self::Json => "json",
            Self::Jsonc => "jsonc",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::ShellScript => "shellscript",
            Self::PowerShell => "powershell",
            Self::Dockerfile => "dockerfile",
            Self::DockerCompose => "dockercompose",
            Self::Markdown => "markdown",
            Self::PlainText => "plaintext",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.id() == id)
    }

    /// Markdown and Plain Text hold prose, where the other languages hold code.
    pub fn is_prose(self) -> bool {
        matches!(self, Self::Markdown | Self::PlainText)
    }
}
