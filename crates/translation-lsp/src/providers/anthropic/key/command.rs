// `translation-lsp anthropic-key`: asks for the key in the terminal and saves
// it. The key reaches the file without passing through the editor.
use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use super::{ApiKey, NotKey, save, saved};
use crate::providers::{credential_directory, error::credentials::CredentialError};

pub(crate) const ARGUMENT: &str = "anthropic-key";

// The terminal of the process, whatever its standard streams are. The server
// starts this command in a new window without giving it streams of its own.
#[cfg(windows)]
const TERMINAL: [&str; 2] = ["CONIN$", "CONOUT$"];
#[cfg(not(windows))]
const TERMINAL: [&str; 2] = ["/dev/tty", "/dev/tty"];

#[cfg(unix)]
macro_rules! replaceable {
    () => {
        " must be a directory with mode 0700, and anthropic-api-key in it a regular file if it exists, neither a symbolic link."
    };
}
#[cfg(not(unix))]
macro_rules! replaceable {
    () => {
        " must be a directory, and anthropic-api-key in it a regular file if it exists, neither a symbolic link."
    };
}

// The texts are ASCII: a Windows console reads them in its own code page.
const INTRODUCTION: &str = concat!(
    "Translation LSP: Anthropic API key\n",
    "\n",
    "The key is saved in the file anthropic-api-key in ",
    directory!(),
    ".\n",
    "The language server sends it to the Anthropic API and nowhere else.\n",
    "Create the key for a single workspace in the Claude Console:\n",
    "https://platform.claude.com/settings/keys\n",
);
const REPLACES: &str = "\nA key is saved there now. The key you enter replaces it.\n";
const PROMPT: &str = "\nPaste the Anthropic API key and press Enter (input is hidden). Press Enter alone to cancel: ";
const MALFORMED: &str =
    "\nThat is not a key: a key has no whitespace or non-ASCII characters. Nothing was saved.\n";
const TOO_LONG: &str = "\nThat is not a key: it is longer than 4096 bytes. Nothing was saved.\n";
const CANCELLED: &str = "\nNo key was saved.\n";
const SAVED: &str = "\nThe key was saved. Return to Zed: the next translation uses it.\n";
const NOT_SAVED: &str = "\nThe key was not saved. ";
const NOT_PRIVATE: &str = concat!(directory!(), replaceable!());
const NO_DIRECTORY: &str = "The OS reported no local configuration directory for this user.";
// A window that the server opened may close when the process ends.
const FINISH: &str = "Press Enter to finish.";
const NO_TERMINAL: &str = "translation-lsp anthropic-key asks for the key in a terminal, and this process has none. Run it in a terminal window";
const ARGUMENTS: &str = "translation-lsp anthropic-key takes no arguments";

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

// `None` when the arguments do not name the command: the process then serves
// an editor.
pub fn from_arguments() -> Option<ExitCode> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next()? != ARGUMENT {
        return None;
    }
    let problem = if arguments.next().is_some() {
        Some(ARGUMENTS)
    } else if TERMINAL.iter().any(|path| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_err()
    }) {
        Some(NO_TERMINAL)
    } else {
        None
    };
    if let Some(problem) = problem {
        eprintln!("{problem}");
        return Some(ExitCode::FAILURE);
    }
    Some(if run(credential_directory::locate(), &mut Terminal) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

// Returns whether a key was saved.
pub(crate) fn run(directory: Result<PathBuf, CredentialError>, console: &mut impl Console) -> bool {
    // A terminal that went away ends the command like an input that ended.
    dialogue(directory, console).unwrap_or(false)
}

fn dialogue(
    directory: Result<PathBuf, CredentialError>,
    console: &mut impl Console,
) -> io::Result<bool> {
    console.say(INTRODUCTION)?;
    // A place that cannot take the key is reported before the key is typed.
    let place = directory.and_then(|directory| Ok((saved(&directory)?, directory)));
    let (replaces, directory) = match place {
        Ok(place) => place,
        Err(error) => return finish(console, Err(error)),
    };
    if replaces {
        console.say(REPLACES)?;
    }
    loop {
        console.say(PROMPT)?;
        let Ok(line) = console.read_hidden() else {
            // The input ended or was interrupted.
            console.say(CANCELLED)?;
            return Ok(false);
        };
        match ApiKey::parse(&line) {
            Ok(key) => return finish(console, save(&directory, &key)),
            Err(NotKey::Empty) => {
                console.say(CANCELLED)?;
                return Ok(false);
            }
            Err(NotKey::Malformed) => console.say(MALFORMED)?,
            Err(NotKey::TooLong) => console.say(TOO_LONG)?,
        }
    }
}

fn finish(console: &mut impl Console, result: Result<(), CredentialError>) -> io::Result<bool> {
    match &result {
        Ok(()) => console.say(SAVED)?,
        Err(error) => {
            let reason = match error {
                CredentialError::Permissions => NOT_PRIVATE.into(),
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
