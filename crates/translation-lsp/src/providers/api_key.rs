//! API keys that the server reads from files in the credential directory. The
//! key command saves them from a terminal, so that no key passes through the
//! editor.
use std::{
    env, fmt,
    fs::{self, File},
    io::{self, Read},
    net::{IpAddr, Ipv6Addr},
    path::{Path, PathBuf},
    str,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use reqwest::{Url, header::HeaderValue};

use crate::providers::{
    Interaction,
    credential_directory::{self, Revision, check_path, inspect},
    error::{Cause, Error, credentials::CredentialError},
};

/// The file of the Anthropic key, which is older than the keys of origins.
const ANTHROPIC_FILE: &str = "anthropic-api-key";
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
const DIRECTORY: &str = directory!();

pub(crate) mod command;
pub(crate) mod terminal;
#[cfg(test)]
pub(crate) mod test_support;

const CONFIRM: &str = "Remove";

/// Where the key command runs.
#[derive(Clone)]
pub(crate) enum Terminal {
    System,
    /// Records the executable instead, and answers whether a window opened.
    #[cfg(test)]
    Synthetic(test_support::Terminal),
}

/// Where a key is sent: the scheme, host, and port of a service. Plain HTTP is
/// only for this computer, so that neither a key nor a text crosses a network
/// unencrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    /// As a browser writes it, such as `https://openrouter.ai`.
    text: String,
    /// The name of the key file, such as `api-key-https-openrouter.ai-443`.
    file: String,
}

impl Origin {
    /// The origin of `url`, or `None` when keys and texts may not be sent there.
    pub fn of(url: &Url) -> Option<Self> {
        let host = url.host_str()?;
        let port = url.port_or_known_default()?;
        // The URL has written an address in its shortest form, an IPv6 one
        // in brackets.
        let address = match host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        {
            Some(address) => address.parse::<Ipv6Addr>().ok().map(IpAddr::V6),
            None => host.parse::<IpAddr>().ok(),
        };
        let local = address.map_or(host == "localhost", |address| address.is_loopback());
        match url.scheme() {
            "https" => (),
            "http" if local => (),
            _ => return None,
        }
        // Windows takes no colon in a file name. The brackets, which no host
        // name contains, keep `[::1]` and a host named `__1` apart.
        let name = match address {
            Some(IpAddr::V6(address)) => format!("[{}]", address.to_string().replace(':', "_")),
            _ => host.to_owned(),
        };
        Some(Self {
            text: url.origin().ascii_serialization(),
            file: format!("api-key-{}-{name}-{port}", url.scheme()),
        })
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::of(&Url::parse(text).ok()?)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The origin of the OpenAI API.
    pub fn openai() -> Self {
        Self::parse("https://api.openai.com").unwrap()
    }
}

/// One key file, and the names the texts give its key.
#[derive(Clone, Debug)]
pub(crate) struct KeySpec {
    /// What the texts call the key, such as `Anthropic API key`.
    pub name: &'static str,
    /// The code action that saves the key.
    pub menu: &'static str,
    /// `None` for the Anthropic key, which has a command of its own.
    origin: Option<Origin>,
}

impl KeySpec {
    pub fn anthropic(menu: &'static str) -> Self {
        Self {
            name: "Anthropic API key",
            menu,
            origin: None,
        }
    }

    /// The key that is sent to `origin`.
    pub fn origin(name: &'static str, menu: &'static str, origin: Origin) -> Self {
        Self {
            name,
            menu,
            origin: Some(origin),
        }
    }

    pub fn file(&self) -> &str {
        file(self.origin.as_ref())
    }

    /// Where the key is sent, for the texts. `None` for the Anthropic key.
    pub fn destination(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }

    fn missing(&self) -> String {
        format!(
            "{} is not saved. Run \"{}\" from the code actions to save it",
            self.name, self.menu
        )
    }

    fn not_private(&self) -> String {
        format!(
            "{} file cannot be used. {DIRECTORY}{}. Correct them, then translate again",
            self.name,
            private(self.file())
        )
    }

    fn empty(&self) -> String {
        format!(
            "{} file is empty. Run \"{}\" from the code actions to save the key",
            self.name, self.menu
        )
    }

    fn invalid(&self) -> String {
        format!(
            "{} file does not hold a key: a key has no whitespace or non-ASCII characters. Run \"{}\" from the code actions to save the key",
            self.name, self.menu
        )
    }

    fn unreadable(&self) -> String {
        format!(
            "{} file could not be read. Check {} in {DIRECTORY}",
            self.name,
            self.file()
        )
    }

    fn no_directory(&self) -> String {
        format!(
            "{} file could not be located: the OS reported no local configuration directory for this user",
            self.name
        )
    }

    pub fn unusable(&self, error: CredentialError) -> Error {
        Error::configuration(match error {
            CredentialError::Permissions => self.not_private(),
            CredentialError::MissingDirectory => self.no_directory(),
            _ => self.unreadable(),
        })
    }
}

/// The file of the key sent to `destination`, or of the Anthropic key.
fn file(destination: Option<&Origin>) -> &str {
    destination.map_or(ANTHROPIC_FILE, |origin| &origin.file)
}

/// What the key file and its directory must be, after the directory's name.
fn private(file: &str) -> String {
    if cfg!(unix) {
        format!(
            " must be a directory with mode 0700 and {file} in it a regular file with mode 0600, neither a symbolic link"
        )
    } else {
        format!(" must be a directory and {file} in it a regular file, neither a symbolic link")
    }
}

#[derive(Clone)]
pub(crate) struct ApiKey {
    pub header: HeaderValue,
    pub text: Arc<str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotKey {
    Empty,
    Malformed,
    TooLong,
}

impl ApiKey {
    /// No environment variable supplies the key. Zed writes the environment of a
    /// language server that fails to start to its log, and `ANTHROPIC_API_KEY`
    /// would also move the user's Claude Code usage to API billing.
    fn read(path: &Path, spec: &KeySpec) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|source| spec.unusable(CredentialError::io("read key", source)))?;
        let text = str::from_utf8(&bytes).map_err(|_| Error::configuration(spec.invalid()))?;
        Self::parse(text).map_err(|error| {
            Error::configuration(match error {
                NotKey::Empty => spec.empty(),
                NotKey::Malformed | NotKey::TooLong => spec.invalid(),
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

    /// The value of an `Authorization` header that carries the key.
    pub fn bearer(&self) -> HeaderValue {
        let mut header = HeaderValue::from_str(&format!("Bearer {}", self.text))
            .expect("a key is visible ASCII");
        header.set_sensitive(true);
        header
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey([redacted])")
    }
}

/// Stands for one key as the provider's context. It is a count of the keys the
/// provider has read, so it tells nothing of the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyId(u64);

impl KeyId {
    #[cfg(test)]
    pub const SYNTHETIC: Self = Self(0);
}

/// The key file, looked at again whenever the key is needed: another process
/// saves and replaces it, and the user may edit or remove it at any time.
#[derive(Clone)]
pub(crate) struct KeyFile {
    spec: KeySpec,
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
    pub fn new(spec: KeySpec, directory: Result<PathBuf, CredentialError>) -> Self {
        Self {
            directory: directory.map_err(|error| spec.unusable(error)),
            spec,
            seen: Arc::default(),
        }
    }

    pub fn directory(&self) -> Result<&Path, Error> {
        self.directory.as_deref().map_err(Error::clone)
    }

    /// Where the key is sent. `None` for the Anthropic key.
    pub fn destination(&self) -> Option<&Origin> {
        self.spec.destination()
    }

    /// The saved key, unless another has taken the place of `expected`.
    pub fn in_use(&self, expected: KeyId) -> Result<ApiKey, Error> {
        let (key, current) = self.read()?;
        if current != expected {
            return Err(Cause::Modified(self.spec.name).into());
        }
        Ok(key)
    }

    /// The saved key, or none, unless that has changed since `expected`.
    pub fn optional_in_use(&self, expected: Option<KeyId>) -> Result<Option<ApiKey>, Error> {
        let current = self.read_optional()?;
        if current.as_ref().map(|(_, id)| *id) != expected {
            return Err(Cause::Modified(self.spec.name).into());
        }
        Ok(current.map(|(key, _)| key))
    }

    /// Opens a terminal window that asks for the key. The notice names the
    /// command either way: the window may be on a machine the user is not
    /// sitting at.
    pub async fn ask_in_terminal(&self, terminal: &Terminal) -> Result<String, Error> {
        let executable = env::current_exe()
            .map_err(|_| Error::failed("Could not locate the language server executable"))?;
        let destination = self.spec.destination();
        let command = terminal::manual_command(&executable, destination);
        let opened = match terminal {
            Terminal::System => terminal::open(&executable, destination).await,
            #[cfg(test)]
            Terminal::Synthetic(terminal) => terminal.open(&executable),
        };
        if opened {
            Ok(format!(
                "A terminal window was opened. Enter the {} there. If no window appeared, run this in a terminal: {command}",
                self.spec.name
            ))
        } else {
            Err(Cause::Terminal {
                purpose: format!("save the {}", self.spec.name),
                command,
            }
            .into())
        }
    }

    /// Removes the key file once the user agrees to `question`.
    pub async fn remove(&self, ui: &impl Interaction, question: &str) -> Result<String, Error> {
        let directory = self.directory()?;
        let name = self.spec.name;
        if ui.choose(question, &[CONFIRM.into()]).await?.is_none() {
            return Ok(format!("The {name} was not removed."));
        }
        let removed =
            remove(directory, self.spec.file()).map_err(|error| self.spec.unusable(error))?;
        Ok(if removed {
            format!("The {name} was removed.")
        } else {
            format!("No {name} is saved.")
        })
    }

    /// The saved key. A missing file is a settings error.
    pub fn read(&self) -> Result<(ApiKey, KeyId), Error> {
        self.read_optional()?
            .ok_or_else(|| Error::configuration(self.spec.missing()))
    }

    /// The saved key, or `None` when no key file exists.
    pub fn read_optional(&self) -> Result<Option<(ApiKey, KeyId)>, Error> {
        // Readers take turns, so that two of them cannot see a replaced key in
        // different orders and count it twice.
        let mut seen = self.seen.lock().unwrap();
        match self
            .directory()
            .and_then(|directory| seen.current(directory, &self.spec))
        {
            Ok(Some(key)) => Ok(Some((key, KeyId(seen.count)))),
            Ok(None) => {
                (seen.key, seen.file) = (None, None);
                Ok(None)
            }
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
    fn current(&mut self, directory: &Path, spec: &KeySpec) -> Result<Option<ApiKey>, Error> {
        let path = directory.join(spec.file());
        let Some(metadata) = attributes(directory, &path).map_err(|error| spec.unusable(error))?
        else {
            return Ok(None);
        };
        let file = Revision::of(&metadata).ok();
        if let Some(key) = &self.key
            && file.is_some()
            && file == self.file
        {
            return Ok(Some(key.clone()));
        }
        // The time is taken before the read, so a file that counts as settled
        // was settled when it was read.
        let settled = file.filter(|file| {
            SystemTime::now()
                .duration_since(file.modified())
                .is_ok_and(|age| age >= SETTLED)
        });
        let key = ApiKey::read(&path, spec)?;
        if self.key.as_ref().map(|seen| &seen.text) != Some(&key.text) {
            self.count += 1;
        }
        (self.key, self.file) = (Some(key.clone()), settled);
        Ok(Some(key))
    }
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
pub(crate) fn saved(directory: &Path, file: &str) -> Result<bool, CredentialError> {
    if !check_path(directory, true)? {
        return Ok(false);
    }
    match fs::symlink_metadata(directory.join(file)) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(CredentialError::Permissions),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(CredentialError::io("inspect path", source)),
    }
}

pub(crate) fn save(directory: &Path, file: &str, key: &ApiKey) -> Result<(), CredentialError> {
    saved(directory, file)?;
    credential_directory::create(directory)?;
    credential_directory::replace(directory, file, key.text.as_bytes())?;
    Ok(())
}

/// Returns whether there was a key file.
pub(crate) fn remove(directory: &Path, file: &str) -> Result<bool, CredentialError> {
    if !saved(directory, file)? {
        return Ok(false);
    }
    fs::remove_file(directory.join(file))
        .map_err(|source| CredentialError::io("remove key", source))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
