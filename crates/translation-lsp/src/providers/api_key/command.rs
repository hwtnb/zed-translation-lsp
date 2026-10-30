//! `translation-lsp anthropic-key`, `translation-lsp deepl-key`, and
//! `translation-lsp api-key <origin>`: ask for a key in the terminal and save it. The key reaches the file without
//! passing through the editor.
use std::{
    env,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use super::{ApiKey, DIRECTORY, KeyKind, NotKey, Origin, save, saved};
use crate::providers::{credential_directory, error::credentials::CredentialError};

pub(crate) const ANTHROPIC: &str = "anthropic-key";
pub(crate) const DEEPL: &str = "deepl-key";
pub(crate) const ORIGIN: &str = "api-key";

// The terminal of the process, whatever its standard streams are. The server
// starts this command in a new window without giving it streams of its own.
#[cfg(windows)]
const TERMINAL: [&str; 2] = ["CONIN$", "CONOUT$"];
#[cfg(not(windows))]
const TERMINAL: [&str; 2] = ["/dev/tty", "/dev/tty"];

// The texts are ASCII: a Windows console reads them in its own code page.
const ANTHROPIC_INTRODUCTION: &str = concat!(
    "Translation LSP: Anthropic API key\n",
    "\n",
    "The key is saved in the file anthropic-api-key in ",
    directory!(),
    ".\n",
    "The language server sends it to the Anthropic API and nowhere else.\n",
    "Create the key for a single workspace in the Claude Console:\n",
    "https://platform.claude.com/settings/keys\n",
);
const DEEPL_INTRODUCTION: &str = concat!(
    "Translation LSP: DeepL API key\n",
    "\n",
    "The key is saved in the file deepl-api-key in ",
    directory!(),
    ".\n",
    "The language server sends it to the DeepL API and nowhere else: a DeepL API\n",
    "Free key to api-free.deepl.com, any other key to api.deepl.com.\n",
    "Find the key in your DeepL account:\n",
    "https://www.deepl.com/your-account/keys\n",
);
const OPENAI_KEYS: &str =
    "Create the key in the OpenAI Platform:\nhttps://platform.openai.com/api-keys\n";
const REPLACES: &str = "\nA key is saved there now. The key you enter replaces it.\n";
const PROMPT: &str = " and press Enter (input is hidden). Press Enter alone to cancel: ";
const MALFORMED: &str =
    "\nThat is not a key: a key has no whitespace or non-ASCII characters. Nothing was saved.\n";
const TOO_LONG: &str = "\nThat is not a key: it is longer than 4096 bytes. Nothing was saved.\n";
const CANCELLED: &str = "\nNo key was saved.\n";
const SAVED: &str = "\nThe key was saved. Return to Zed: the next translation uses it.\n";
const NOT_SAVED: &str = "\nThe key was not saved. ";
const NO_DIRECTORY: &str = "The OS reported no local configuration directory for this user.";
/// A window that the server opened may close when the process ends.
const FINISH: &str = "Press Enter to finish.";
const ORIGIN_ARGUMENT: &str = "translation-lsp api-key takes one argument: the URL of the service the key is sent to, such as https://openrouter.ai. Plain http is only for this computer (localhost, 127.0.0.1, or ::1)";

pub(crate) trait Console {
    fn say(&mut self, text: &str) -> io::Result<()>;
    fn read_hidden(&mut self) -> io::Result<String>;
}

struct Terminal;

impl Console for Terminal {
    fn say(&mut self, text: &str) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .open(TERMINAL[1])?
            .write_all(text.as_bytes())
    }

    fn read_hidden(&mut self) -> io::Result<String> {
        rpassword::read_password()
    }
}

/// The arguments that run the command for the key of `kind`, after the
/// executable.
pub(crate) fn arguments(kind: &KeyKind) -> Vec<&str> {
    match kind {
        KeyKind::Anthropic => vec![ANTHROPIC],
        KeyKind::DeepL => vec![DEEPL],
        KeyKind::Origin(origin) => vec![ORIGIN, origin.as_str()],
    }
}

/// `None` when the arguments do not name the command: the process then serves
/// an editor.
pub fn from_arguments() -> Option<ExitCode> {
    let mut arguments = env::args().skip(1);
    let name = arguments.next()?;
    let rest: Vec<String> = arguments.collect();
    let kind = match name.as_str() {
        ANTHROPIC | DEEPL if !rest.is_empty() => {
            Err(format!("translation-lsp {name} takes no arguments"))
        }
        ANTHROPIC => Ok(KeyKind::Anthropic),
        DEEPL => Ok(KeyKind::DeepL),
        ORIGIN => match &rest[..] {
            [origin] => Origin::parse(origin)
                .map(KeyKind::Origin)
                .ok_or_else(|| ORIGIN_ARGUMENT.to_owned()),
            _ => Err(ORIGIN_ARGUMENT.to_owned()),
        },
        _ => return None,
    };
    let problem = match kind {
        Ok(_)
            if TERMINAL.iter().any(|path| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .is_err()
            }) =>
        {
            Err(format!(
                "translation-lsp {name} asks for the key in a terminal, and this process has none. Run it in a terminal window"
            ))
        }
        kind => kind,
    };
    let kind = match problem {
        Ok(kind) => kind,
        Err(problem) => {
            eprintln!("{problem}");
            return Some(ExitCode::FAILURE);
        }
    };
    #[cfg(unix)]
    let Ok(_interruptions) = receive_interruptions() else {
        eprintln!(
            "translation-lsp {name} could not prepare to be interrupted, and asked for no key"
        );
        return Some(ExitCode::FAILURE);
    };
    Some(
        if run(credential_directory::locate(), &kind, &mut Terminal) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    )
}

/// The hidden read takes Ctrl+C as a character and raises the signal itself,
/// before it restores the terminal. A process ended there would leave the shell
/// that started it without Ctrl+C. Received instead, the signal does nothing:
/// the read reports the interruption and restores the terminal. Between reads
/// Ctrl+C is ignored, so saving is never cut short.
///
/// Unix only: a Windows console ends the process on Ctrl+C without the read
/// seeing it, and a process kept alive there would save what was typed so far.
#[cfg(unix)]
fn receive_interruptions() -> io::Result<impl Sized> {
    use tokio::{
        runtime,
        signal::unix::{SignalKind, signal},
    };

    let runtime = runtime::Builder::new_current_thread().enable_io().build()?;
    let interruptions = {
        let _context = runtime.enter();
        signal(SignalKind::interrupt())?
    };
    Ok((runtime, interruptions))
}

/// What the command says first: which key it saves, where, and where the
/// server sends it.
fn introduction(kind: &KeyKind) -> String {
    let origin = match kind {
        KeyKind::Anthropic => return ANTHROPIC_INTRODUCTION.into(),
        KeyKind::DeepL => return DEEPL_INTRODUCTION.into(),
        KeyKind::Origin(origin) => origin,
    };
    let file = kind.file();
    let origin = origin.as_str();
    let mut text = format!(
        "Translation LSP: API key for {origin}\n\nThe key is saved in the file {file} in {DIRECTORY}.\nThe language server sends it to {origin} and nowhere else.\n"
    );
    if origin == Origin::openai().as_str() {
        text.push_str(OPENAI_KEYS);
    }
    text
}

fn prompt(kind: &KeyKind) -> String {
    let key = match kind {
        KeyKind::Anthropic => "the Anthropic API key",
        KeyKind::DeepL => "the DeepL API key",
        KeyKind::Origin(_) => "the API key",
    };
    format!("\nPaste {key}{PROMPT}")
}

/// Why the key cannot be saved in a place that is not private.
fn not_private(file: &str) -> String {
    if cfg!(unix) {
        format!(
            "{DIRECTORY} must be a directory with mode 0700, and {file} in it a regular file if it exists, neither a symbolic link."
        )
    } else {
        format!(
            "{DIRECTORY} must be a directory, and {file} in it a regular file if it exists, neither a symbolic link."
        )
    }
}

/// Saves the key of `kind`. Returns whether a key was saved.
pub(crate) fn run(
    directory: Result<PathBuf, CredentialError>,
    kind: &KeyKind,
    console: &mut impl Console,
) -> bool {
    // A terminal that went away ends the command like an input that ended.
    dialogue(directory, kind, console).unwrap_or(false)
}

fn dialogue(
    directory: Result<PathBuf, CredentialError>,
    kind: &KeyKind,
    console: &mut impl Console,
) -> io::Result<bool> {
    console.say(&introduction(kind))?;
    let file = kind.file();
    // A place that cannot take the key is reported before the key is typed.
    let place = directory.and_then(|directory| Ok((saved(&directory, file)?, directory)));
    let (replaces, directory) = match place {
        Ok(place) => place,
        Err(error) => return finish(console, file, Err(error)),
    };
    if replaces {
        console.say(REPLACES)?;
    }
    let prompt = prompt(kind);
    loop {
        console.say(&prompt)?;
        let Ok(line) = console.read_hidden() else {
            // The input ended or was interrupted.
            console.say(CANCELLED)?;
            return Ok(false);
        };
        match ApiKey::parse(&line) {
            Ok(key) => return finish(console, file, save(&directory, file, &key)),
            Err(NotKey::Empty) => {
                console.say(CANCELLED)?;
                return Ok(false);
            }
            Err(NotKey::Malformed) => console.say(MALFORMED)?,
            Err(NotKey::TooLong) => console.say(TOO_LONG)?,
        }
    }
}

fn finish(
    console: &mut impl Console,
    file: &str,
    result: Result<(), CredentialError>,
) -> io::Result<bool> {
    match &result {
        Ok(()) => console.say(SAVED)?,
        Err(error) => {
            let reason = match error {
                CredentialError::Permissions => not_private(file),
                CredentialError::MissingDirectory => NO_DIRECTORY.into(),
                // Neither part names a path.
                CredentialError::Io { operation, source } => {
                    format!("The file could not be written ({operation}: {source}).")
                }
                _ => "The file could not be written.".to_owned(),
            };
            console.say(&format!("{NOT_SAVED}{reason}\n"))?;
        }
    }
    console.say(FINISH)?;
    let _ = console.read_hidden();
    Ok(result.is_ok())
}

#[cfg(test)]
mod tests;
