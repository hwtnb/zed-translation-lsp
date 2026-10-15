use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use reqwest::header::HeaderValue;

use crate::providers::{
    credential_directory::{self, Revision, check_path, inspect},
    error::{Error, credentials::CredentialError},
};

const FILE: &str = "anthropic-api-key";
/// A key is about a hundred bytes.
const MAX_BYTES: usize = 4096;
/// How old a file must be before its attributes stand for its contents: one
/// written twice within a tick of its modification time changes without a
/// change in them. FAT has the coarsest tick, 2 seconds; the rest allows for a
/// clock that is set back a little.
const SETTLED: Duration = Duration::from_secs(3);

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

macro_rules! menu {
    () => {
        "Anthropic API key…"
    };
}
pub(super) const MENU: &str = menu!();

pub(crate) mod command;
pub(super) mod terminal;

const MISSING: &str = concat!(
    "Anthropic API key is not saved. Run \"",
    menu!(),
    "\" from the code actions to save it"
);
const NOT_PRIVATE: &str = concat!(
    "Anthropic API key file cannot be used. ",
    directory!(),
    private!(),
    ". Correct them, then translate again"
);
const EMPTY: &str = concat!(
    "Anthropic API key file is empty. Run \"",
    menu!(),
    "\" from the code actions to save the key"
);
const INVALID: &str = concat!(
    "Anthropic API key file does not hold a key: a key has no whitespace or non-ASCII ",
    "characters. Run \"",
    menu!(),
    "\" from the code actions to save the key"
);
const UNREADABLE: &str = concat!(
    "Anthropic API key file could not be read. Check anthropic-api-key in ",
    directory!()
);
const NO_DIRECTORY: &str = concat!(
    "Anthropic API key file could not be located: the OS reported no local configuration ",
    "directory for this user"
);
pub(super) const SEVERAL_WORKSPACES: &str = concat!(
    "The API key works in more than one workspace, and the server does not send a workspace ",
    "ID. Create a key for a single workspace in the Claude Console, then run \"",
    menu!(),
    "\" from the code actions to replace the key"
);
pub(super) const REJECTED: &str = concat!(
    "The API key was rejected. Run \"",
    menu!(),
    "\" from the code actions to replace it"
);

#[derive(Clone)]
pub(super) struct ApiKey {
    pub header: HeaderValue,
    pub text: Arc<str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum NotKey {
    Empty,
    Malformed,
    TooLong,
}

impl ApiKey {
    /// No environment variable supplies the key. Zed writes the environment of a
    /// language server that fails to start to its log, and `ANTHROPIC_API_KEY`
    /// would also move the user's Claude Code usage to API billing.
    fn read(path: &Path) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|source| unusable(CredentialError::io("read key", source)))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::configuration(INVALID))?;
        Self::parse(text).map_err(|error| {
            Error::configuration(match error {
                NotKey::Empty => EMPTY,
                NotKey::Malformed | NotKey::TooLong => INVALID,
            })
        })
    }

    pub fn parse(text: &str) -> Result<Self, NotKey> {
        if text.len() > MAX_BYTES {
            return Err(NotKey::TooLong);
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(NotKey::Empty);
        }
        let header = HeaderValue::from_str(text)
            .ok()
            .filter(|_| text.bytes().all(|byte| byte.is_ascii_graphic()));
        let Some(mut header) = header else {
            return Err(NotKey::Malformed);
        };
        header.set_sensitive(true);
        Ok(Self {
            header,
            text: text.into(),
        })
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

/// Stands for one key in `Context`. It is a count of the keys the provider has
/// read, so it tells nothing of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyId(u64);

impl KeyId {
    #[cfg(test)]
    pub const SYNTHETIC: Self = Self(0);
}

/// The key file, looked at again whenever the key is needed: another process
/// saves and replaces it, and the user may edit or remove it at any time.
#[derive(Clone)]
pub(super) struct KeyFile {
    directory: Result<PathBuf, Error>,
    seen: Arc<Mutex<Seen>>,
}

/// The key of the last read, to tell when another takes its place. A read that
/// finds no usable key forgets it, so the same key saved again counts as new.
#[derive(Default)]
struct Seen {
    key: Option<ApiKey>,
    /// Set only when the file had settled by the time it was read.
    file: Option<Revision>,
    count: u64,
}

impl KeyFile {
    pub fn new(directory: Result<PathBuf, CredentialError>) -> Self {
        Self {
            directory: directory.map_err(unusable),
            seen: Arc::default(),
        }
    }

    pub fn directory(&self) -> Result<&Path, Error> {
        self.directory.as_deref().map_err(Error::clone)
    }

    pub fn read(&self) -> Result<(ApiKey, KeyId), Error> {
        // Readers take turns, so that two of them cannot see a replaced key in
        // different orders and count it twice.
        let mut seen = self.seen.lock().unwrap();
        match self
            .directory()
            .and_then(|directory| seen.current(directory))
        {
            Ok(key) => Ok((key, KeyId(seen.count))),
            Err(error) => {
                (seen.key, seen.file) = (None, None);
                Err(error)
            }
        }
    }
}

impl Seen {
    /// The directory and the file are checked every time. The file is opened
    /// only when its attributes differ from those of the last read.
    fn current(&mut self, directory: &Path) -> Result<ApiKey, Error> {
        let path = directory.join(FILE);
        let Some(metadata) = attributes(directory, &path).map_err(unusable)? else {
            return Err(Error::configuration(MISSING));
        };
        let file = Revision::of(&metadata).ok();
        if let Some(key) = &self.key
            && file.is_some()
            && file == self.file
        {
            return Ok(key.clone());
        }
        // The time is taken before the read, so a file that counts as settled
        // was settled when it was read.
        let settled = file.filter(|file| {
            SystemTime::now()
                .duration_since(file.modified())
                .is_ok_and(|age| age >= SETTLED)
        });
        let key = ApiKey::read(&path)?;
        if self.key.as_ref().map(|seen| &seen.text) != Some(&key.text) {
            self.count += 1;
        }
        (self.key, self.file) = (Some(key.clone()), settled);
        Ok(key)
    }
}

pub(super) fn unusable(error: CredentialError) -> Error {
    Error::configuration(match error {
        CredentialError::Permissions => NOT_PRIVATE,
        CredentialError::MissingDirectory => NO_DIRECTORY,
        _ => UNREADABLE,
    })
}

/// `None` when the directory or the file is missing.
fn attributes(directory: &Path, file: &Path) -> Result<Option<fs::Metadata>, CredentialError> {
    if !check_path(directory, true)? {
        return Ok(None);
    }
    inspect(file, false)
}

/// Whether there is a key file to replace. It may have any permissions, since a
/// new file takes its place, but only a regular file is replaced or removed.
pub(super) fn saved(directory: &Path) -> Result<bool, CredentialError> {
    if !check_path(directory, true)? {
        return Ok(false);
    }
    match fs::symlink_metadata(directory.join(FILE)) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(CredentialError::Permissions),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CredentialError::io("inspect path", source)),
    }
}

pub(super) fn save(directory: &Path, key: &ApiKey) -> Result<(), CredentialError> {
    saved(directory)?;
    credential_directory::create(directory)?;
    credential_directory::replace(directory, FILE, key.text.as_bytes())?;
    Ok(())
}

/// Returns whether there was a key file.
pub(super) fn remove(directory: &Path) -> Result<bool, CredentialError> {
    if !saved(directory)? {
        return Ok(false);
    }
    fs::remove_file(directory.join(FILE))
        .map_err(|source| CredentialError::io("remove key", source))?;
    Ok(true)
}
