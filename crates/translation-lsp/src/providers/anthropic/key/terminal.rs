// Opens a terminal window of the OS that runs this executable with the key
// command. The window belongs to the terminal: the server gives it none of its
// own streams, and does not wait for what the user does there.
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::command::ARGUMENT;

// How long a started program has to fail. `open` and the terminals that hand
// the window to a running instance end sooner than this; the others are still
// running then, with the window open.
const GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Launch {
    program: OsString,
    arguments: Vec<OsString>,
}

impl Launch {
    fn new(program: impl Into<OsString>, arguments: &[&str], command: &[&Path]) -> Self {
        Self {
            program: program.into(),
            arguments: arguments
                .iter()
                .map(OsString::from)
                .chain(command.iter().map(OsString::from))
                .collect(),
        }
    }
}

// Terminal of macOS takes a file to run and no arguments for it, so it is given
// a script that runs the command.
pub(super) fn macos(script: &Path) -> Vec<Launch> {
    vec![Launch::new("/usr/bin/open", &["-a", "Terminal"], &[script])]
}

// Linux has no single way to open a terminal, so the ones below are tried in
// this order: the two that stand for the user's choice, then those of the
// desktops, then the rest. Each takes the command the way its manual gives.
pub(super) fn linux(executable: &Path) -> Vec<Launch> {
    let command = [executable, Path::new(ARGUMENT)];
    let terminals: [(&str, &[&str]); 11] = [
        ("xdg-terminal-exec", &[]),
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("ptyxis", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-x"]),
        ("kitty", &[]),
        ("alacritty", &["-e"]),
        ("foot", &[]),
        ("wezterm", &["start", "--"]),
        ("xterm", &["-e"]),
    ];
    terminals
        .into_iter()
        .map(|(program, arguments)| Launch::new(program, arguments, &command))
        .collect()
}

// Windows gives the command a console of its own; see `spawn`.
pub(super) fn windows(executable: &Path) -> Vec<Launch> {
    vec![Launch::new(executable, &[ARGUMENT], &[])]
}

// The script Terminal runs. It removes itself first, so that it is gone
// whatever happens to the command, and it holds nothing but the path of the
// executable.
pub(super) fn script(executable: &str) -> String {
    format!(
        "#!/bin/sh\nrm -f \"$0\"\nexec {} {ARGUMENT}\n",
        quoted(executable)
    )
}

fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

// What the user types to run the command by hand, when no window opens or the
// server runs on another machine.
pub(in crate::providers::anthropic) fn manual_command(executable: &Path) -> String {
    let path = executable.to_string_lossy();
    let plain = |extra: &str| {
        path.chars()
            .all(|character| character.is_ascii_alphanumeric() || extra.contains(character))
    };
    if cfg!(windows) {
        if plain("\\/:._-") {
            format!("{path} {ARGUMENT}")
        } else {
            format!("\"{path}\" {ARGUMENT}")
        }
    } else if plain("/._-+") {
        format!("{path} {ARGUMENT}")
    } else {
        format!("{} {ARGUMENT}", quoted(&path))
    }
}

fn write_script(executable: &Path) -> io::Result<PathBuf> {
    let text = script(executable.to_str().ok_or(io::ErrorKind::InvalidInput)?);
    let path = std::env::temp_dir().join(format!(
        "translation-lsp-anthropic-key-{}.command",
        uuid::Uuid::new_v4()
    ));
    let mut options = fs::OpenOptions::new();
    // A new file that only this user can change, under a name no one can guess.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let written = options
        .open(&path)
        .and_then(|mut file| file.write_all(text.as_bytes()));
    if let Err(error) = written {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn spawn(launch: &Launch) -> io::Result<Child> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.arguments)
        // The server's own streams carry the LSP messages.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A signal sent to the server and its group leaves the window open.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        command.creation_flags(CREATE_NEW_CONSOLE);
        // The process still inherits the server's inheritable handles, the
        // LSP pipes among them, and holds them until it ends: `Command` cannot
        // turn inheritance off on stable Rust (rust-lang/rust#146407).
    }
    command.spawn()
}

// Whether the program started and did not fail within `grace`. The program is
// then left to itself; a thread collects its exit status when it ends.
async fn started(mut child: Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => tokio::time::sleep(POLL).await,
            _ => break,
        }
    }
    let _ = thread::Builder::new()
        .name("terminal-exit".into())
        .spawn(move || child.wait());
    true
}

pub(super) async fn first_started(launches: &[Launch], grace: Duration) -> bool {
    for launch in launches {
        if let Ok(child) = spawn(launch)
            && started(child, grace).await
        {
            return true;
        }
    }
    false
}

// Returns whether a terminal took the command.
pub(in crate::providers::anthropic) async fn open(executable: &Path) -> bool {
    if cfg!(target_os = "macos") {
        let Ok(script) = write_script(executable) else {
            return false;
        };
        let opened = first_started(&macos(&script), GRACE).await;
        if !opened {
            let _ = fs::remove_file(script);
        }
        opened
    } else if cfg!(windows) {
        // A started process has its console; there is no failure to wait for.
        first_started(&windows(executable), Duration::ZERO).await
    } else {
        first_started(&linux(executable), GRACE).await
    }
}

#[cfg(test)]
mod tests;
