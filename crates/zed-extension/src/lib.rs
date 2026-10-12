use zed_extension_api::{self as zed, settings::LspSettings};

mod install;

const SERVER_ID: &str = "translation-lsp";
const BINARY_NAME: &str = "translation-lsp";

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
        let command = match binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which(BINARY_NAME))
        {
            Some(command) => command,
            None => install::server(language_server_id)?,
        };

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
