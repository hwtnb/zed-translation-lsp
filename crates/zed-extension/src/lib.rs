use zed_extension_api::{self as zed, settings::LspSettings};

const SERVER_ID: &str = "zed-code-translation";
const BINARY_NAME: &str = "zed-code-translation-lsp";

struct TranslationExtension;

impl zed::Extension for TranslationExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        if language_server_id.as_ref() != SERVER_ID {
            return Err(format!("Unknown language server: {language_server_id}"));
        }

        let settings = LspSettings::for_worktree(SERVER_ID, worktree)?;
        let binary = settings.binary;
        let command = binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which(BINARY_NAME))
            .ok_or_else(|| {
                format!(
                    "Build {BINARY_NAME} locally and set lsp.{SERVER_ID}.binary.path \
                     to its absolute path. See the extension README."
                )
            })?;

        Ok(zed::Command {
            command,
            args: binary
                .as_ref()
                .and_then(|binary| binary.arguments.clone())
                .unwrap_or_default(),
            env: binary
                .and_then(|binary| binary.env)
                .unwrap_or_default()
                .into_iter()
                .collect(),
        })
    }
}

zed::register_extension!(TranslationExtension);
