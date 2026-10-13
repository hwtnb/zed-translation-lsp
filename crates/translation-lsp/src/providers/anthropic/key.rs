use std::{fs::File, io::Read, path::PathBuf, sync::Arc};

use reqwest::header::HeaderValue;

use crate::providers::{
    credential_directory::check_path,
    error::{Error, credentials::CredentialError},
};

const FILE: &str = "anthropic-api-key";
// A key is about a hundred bytes.
const MAX_BYTES: u64 = 4096;

// The credential directory as the documentation writes it. The messages name
// it this way, never by the resolved path, which holds the user's name.
#[cfg(windows)]
macro_rules! directory {
    () => {
        r"%LOCALAPPDATA%\translation-lsp"
    };
}
#[cfg(target_os = "macos")]
macro_rules! directory {
    () => {
        "~/Library/Application Support/translation-lsp"
    };
}
#[cfg(not(any(windows, target_os = "macos")))]
macro_rules! directory {
    () => {
        "$XDG_CONFIG_HOME/translation-lsp (~/.config/translation-lsp when XDG_CONFIG_HOME is unset)"
    };
}

// What `check_path` requires of the directory and the file.
#[cfg(unix)]
macro_rules! private {
    () => {
        " must be a directory with mode 0700 and anthropic-api-key in it a regular file with mode 0600, neither a symbolic link"
    };
}
#[cfg(not(unix))]
macro_rules! private {
    () => {
        " must be a directory and anthropic-api-key in it a regular file, neither a symbolic link"
    };
}

const MISSING: &str = concat!(
    "Anthropic API key file was not found. Save the key in a file named anthropic-api-key in ",
    directory!(),
    ", then restart the language server"
);
const NOT_PRIVATE: &str = concat!(
    "Anthropic API key file cannot be used. ",
    directory!(),
    private!(),
    ". Correct them, then restart the language server"
);
const EMPTY: &str = concat!(
    "Anthropic API key file is empty. Save the key in anthropic-api-key in ",
    directory!(),
    ", then restart the language server"
);
const INVALID: &str = concat!(
    "Anthropic API key file must hold only the key, which has no whitespace or non-ASCII ",
    "characters. Correct anthropic-api-key in ",
    directory!(),
    ", then restart the language server"
);
const UNREADABLE: &str = concat!(
    "Anthropic API key file could not be read. Check anthropic-api-key in ",
    directory!(),
    ", then restart the language server"
);
const NO_DIRECTORY: &str = concat!(
    "Anthropic API key file could not be located: the OS reported no local configuration ",
    "directory for this user"
);
pub(super) const SEVERAL_WORKSPACES: &str = concat!(
    "The API key works in more than one workspace, and the server does not send a workspace ",
    "ID. Create a key for a single workspace in the Claude Console, save it in ",
    "anthropic-api-key in ",
    directory!(),
    ", then restart the language server"
);
pub(super) const REJECTED: &str = concat!(
    "The API key was rejected. Check the key in anthropic-api-key in ",
    directory!(),
    " and restart the language server after changing it"
);

#[derive(Clone)]
pub(super) struct ApiKey {
    pub header: HeaderValue,
    pub text: Arc<str>,
}

impl ApiKey {
    // Reads the key from its file in the credential directory. The server only
    // reads there: it creates, changes, and removes nothing.
    //
    // No environment variable supplies the key. Zed writes the environment of a
    // language server that fails to start to its log, and `ANTHROPIC_API_KEY`
    // would also move the user's Claude Code usage to API billing.
    pub fn read(directory: Result<PathBuf, CredentialError>) -> Result<Self, Error> {
        let bytes = contents(directory).map_err(|error| {
            Error::configuration(match error {
                CredentialError::Permissions => NOT_PRIVATE,
                CredentialError::MissingDirectory => NO_DIRECTORY,
                _ => UNREADABLE,
            })
        })?;
        let Some(bytes) = bytes else {
            return Err(Error::configuration(MISSING));
        };
        match std::str::from_utf8(&bytes) {
            Ok(text) if bytes.len() as u64 <= MAX_BYTES => Self::parse(text),
            _ => Err(Error::configuration(INVALID)),
        }
    }

    // Whitespace around the key, such as the line break that ends the file, is
    // not part of it.
    fn parse(text: &str) -> Result<Self, Error> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::configuration(EMPTY));
        }
        let header = HeaderValue::from_str(text)
            .ok()
            .filter(|_| text.bytes().all(|byte| byte.is_ascii_graphic()));
        let Some(mut header) = header else {
            return Err(Error::configuration(INVALID));
        };
        header.set_sensitive(true);
        Ok(Self {
            header,
            text: text.into(),
        })
    }

    // The key of a file with this text, or of no file.
    #[cfg(test)]
    pub fn synthetic(text: Option<&str>) -> Result<Self, Error> {
        text.map_or(Err(Error::configuration(MISSING)), Self::parse)
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

// The bytes of the key file, or `None` when the directory or the file is missing.
fn contents(
    directory: Result<PathBuf, CredentialError>,
) -> Result<Option<Vec<u8>>, CredentialError> {
    let directory = directory?;
    if !check_path(&directory, true)? {
        return Ok(None);
    }
    let path = directory.join(FILE);
    if !check_path(&path, false)? {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|source| CredentialError::io("read key", source))?;
    Ok(Some(bytes))
}
