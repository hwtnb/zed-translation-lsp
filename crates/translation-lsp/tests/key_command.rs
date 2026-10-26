//! `translation-lsp anthropic-key` as the process the server starts in a new
//! terminal window. A pseudo-terminal stands for the window, and the process
//! gets no standard stream, as when the server starts it.
//!
//! The command saves in the credential directory of the user who runs it, so
//! every process here gets a home directory of the test's own. Windows takes the
//! directory from the OS and not from the environment, so nothing here runs
//! there: the unit tests of the command cover what it saves and refuses.
#![cfg(unix)]

use std::{
    env,
    ffi::{CStr, CString, OsStr},
    fs::{self, File},
    io::{self, Read, Write},
    mem,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Mutex,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

const KEY: &str = "sk-ant-synthetic-typed-key";
const PROMPT: &str = "Press Enter alone to cancel: ";
const FINISH: &str = "Press Enter to finish.";
/// Ends a test that would otherwise hang. Nothing waits this long when the
/// command works: each step goes on as soon as what it waits for is seen.
const LIMIT: Duration = Duration::from_secs(60);

/// A home directory that holds nothing of the user.
struct Home {
    root: PathBuf,
}

impl Home {
    fn new() -> Self {
        let root = env::temp_dir().join(format!("translation-lsp-key-command-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self { root }
    }

    /// Where the command looks for the credential directory under this home.
    fn directory(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.root
                .join("home/Library/Application Support/translation-lsp")
        } else {
            self.root.join("config/translation-lsp")
        }
    }

    fn key_file(&self) -> PathBuf {
        self.directory().join("anthropic-api-key")
    }

    /// The executable with this home, and without the streams of the test.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_translation-lsp"));
        command
            .args(arguments)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn untouched(&self) -> bool {
        fs::read_dir(&self.root).unwrap().next().is_none()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `ptsname` answers in a buffer that all threads share.
static PTSNAME: Mutex<()> = Mutex::new(());

/// The side of a pseudo-terminal that a terminal window holds: what is written
/// here is typed, and what is read here is shown.
struct Window {
    master: File,
    /// The terminal as the shell that started the command holds it. While it is
    /// open, the terminal keeps the settings that the command leaves behind.
    shell: File,
    shown: Receiver<Vec<u8>>,
    seen: String,
    /// Where the text that `wait_for` found last ends. One read may bring more
    /// than that text, so what follows it is still to be found.
    found: usize,
}

/// How the terminal treats what is typed: whether it shows it, hands it over
/// line by line, and turns Ctrl+C into a signal.
#[derive(Debug, PartialEq, Eq)]
struct Settings {
    echo: bool,
    lines: bool,
    signals: bool,
}

/// What a fresh terminal does, and what a shell expects to find again.
const USUAL: Settings = Settings {
    echo: true,
    lines: true,
    signals: true,
};

impl Window {
    /// Opens a pseudo-terminal and starts `command` with it as the controlling
    /// terminal of a session of its own.
    #[allow(
        unsafe_code,
        reason = "the standard library opens no pseudo-terminal and starts no session"
    )]
    fn start(mut command: Command) -> (Self, Child) {
        // SAFETY: `posix_openpt` takes no pointer. It returns a new descriptor
        // or -1, which is checked before the descriptor is used.
        let descriptor = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        assert!(descriptor >= 0, "{}", io::Error::last_os_error());
        // SAFETY: the descriptor was just opened, is valid, and has no other
        // owner, so the `File` may own and close it.
        let master = unsafe { File::from_raw_fd(descriptor) };
        let slave = {
            let _only = PTSNAME.lock().unwrap();
            // SAFETY: `master` is an open pseudo-terminal master for all three
            // calls. `ptsname` returns null or a NUL-terminated string in a
            // static buffer; the lock keeps other threads of this test from
            // overwriting it before it is copied.
            unsafe {
                assert_eq!(libc::grantpt(descriptor), 0);
                assert_eq!(libc::unlockpt(descriptor), 0);
                let name = libc::ptsname(descriptor);
                assert!(!name.is_null());
                CString::from(CStr::from_ptr(name))
            }
        };
        // The test never becomes a session leader, and asks not to get the
        // terminal as its own either.
        let shell = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(OsStr::from_bytes(slave.to_bytes()))
            .unwrap();
        // SAFETY: the closure runs in the child between `fork` and `exec`,
        // where only async-signal-safe functions may be called. `setsid` and
        // `open` are, and the closure allocates nothing: `slave` was built
        // before the fork, and `last_os_error` only reads `errno`. A session
        // leader without a controlling terminal gets the terminal it opens
        // without `O_NOCTTY` as its controlling terminal, on Linux and macOS.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::open(slave.as_ptr(), libc::O_RDWR) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let (sender, shown) = mpsc::channel();
        let mut reader = master.try_clone().unwrap();
        thread::spawn(move || {
            let mut buffer = [0; 1024];
            // The read fails or returns nothing once the child has closed the terminal.
            while let Ok(count @ 1..) = reader.read(&mut buffer) {
                if sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        (
            Self {
                master,
                shell,
                shown,
                seen: String::new(),
                found: 0,
            },
            child,
        )
    }

    /// Waits until the window has shown `text` after the text found before.
    fn wait_for(&mut self, text: &str) {
        let begun = Instant::now();
        loop {
            if let Some(at) = self.seen[self.found..].find(text) {
                self.found += at + text.len();
                return;
            }
            let left = LIMIT.saturating_sub(begun.elapsed());
            let bytes = self.shown.recv_timeout(left).unwrap_or_else(|_| {
                panic!("{text:?} was not shown after: {}", &self.seen[self.found..])
            });
            self.seen.push_str(&String::from_utf8_lossy(&bytes));
        }
    }

    #[allow(
        unsafe_code,
        reason = "the standard library does not read the settings of a terminal"
    )]
    fn settings(&self) -> Settings {
        // SAFETY: `termios` is plain data, so all zeros is a valid value, and
        // `tcgetattr` fills it for the open terminal that `master` refers to.
        let settings = unsafe {
            let mut settings: libc::termios = mem::zeroed();
            assert_eq!(libc::tcgetattr(self.master.as_raw_fd(), &mut settings), 0);
            settings
        };
        let set = |flag| settings.c_lflag & flag != 0;
        Settings {
            echo: set(libc::ECHO),
            lines: set(libc::ICANON),
            signals: set(libc::ISIG),
        }
    }

    /// Types `text` once the terminal has stopped showing what is typed. The
    /// prompt is shown a moment before that, so the prompt alone is too early.
    fn type_hidden(&mut self, text: &str) {
        let begun = Instant::now();
        while self.settings().echo {
            assert!(begun.elapsed() < LIMIT, "Input stayed visible");
            thread::sleep(Duration::from_millis(2));
        }
        self.master.write_all(text.as_bytes()).unwrap();
    }

    /// How the process ended, everything the window showed, and the settings
    /// the process left to the shell.
    fn finish(mut self, mut child: Child) -> (ExitStatus, String, Settings) {
        let begun = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if begun.elapsed() >= LIMIT {
                child.kill().unwrap();
                panic!("The command did not end after: {}", self.seen);
            }
            thread::sleep(Duration::from_millis(2));
        };
        let left = self.settings();
        // With the terminal closed on every side, the reader ends, which ends
        // the channel.
        drop(self.shell);
        drop(self.master);
        while let Ok(bytes) = self.shown.recv_timeout(LIMIT) {
            self.seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        (status, self.seen, left)
    }
}

/// The harness shows what a terminal shows: what is typed while the terminal
/// echoes appears among what the window read. Without this, a harness that
/// could never see typed input would pass the tests below.
#[test]
fn window_shows_what_is_typed_while_the_terminal_echoes() {
    let mut command = Command::new("/bin/cat");
    command
        .stdin(Stdio::null())
        .arg("/dev/tty")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (mut window, mut child) = Window::start(command);
    assert_eq!(window.settings(), USUAL);
    window.master.write_all(b"typed-and-visible\n").unwrap();
    window.wait_for("typed-and-visible");
    child.kill().unwrap();
    window.finish(child);
}

#[test]
fn typed_key_is_saved_without_being_shown() {
    let home = Home::new();
    let (mut window, child) = Window::start(home.command(&["anthropic-key"]));
    window.wait_for("The key is saved in the file anthropic-api-key in ");
    window.wait_for(PROMPT);
    // An input that is not a key is not shown either, and is asked for again.
    window.type_hidden("sk-ant-synthetic two-words\r");
    window.wait_for("That is not a key");
    assert!(!home.directory().exists());
    window.wait_for(PROMPT);
    window.type_hidden(&format!("{KEY}\r"));
    window.wait_for("The key was saved.");
    window.wait_for(FINISH);
    // The command waits for Enter, so that the window stays to be read.
    assert_eq!(fs::read(home.key_file()).unwrap(), KEY.as_bytes());
    window.type_hidden("\r");
    let (status, shown, left) = window.finish(child);
    assert!(status.success(), "{shown}");
    assert_eq!(left, USUAL);
    for hidden in [KEY, "two-words"] {
        assert!(!shown.contains(hidden), "{shown}");
    }
    assert_eq!(mode(&home.directory()), 0o700);
    assert_eq!(mode(&home.key_file()), 0o600);
    assert_eq!(fs::read_dir(home.directory()).unwrap().count(), 1);

    // A second run says that it replaces the key, and does.
    let replacing = "sk-ant-synthetic-second-typed-key";
    let (mut window, child) = Window::start(home.command(&["anthropic-key"]));
    window.wait_for("A key is saved there now. The key you enter replaces it.");
    window.wait_for(PROMPT);
    window.type_hidden(&format!("{replacing}\r"));
    window.wait_for(FINISH);
    // Ctrl+C ends the wait as Enter does. The key stays saved.
    window.type_hidden("\u{3}");
    let (status, shown, left) = window.finish(child);
    assert_eq!(left, USUAL);
    assert!(status.success(), "{shown}");
    assert!(
        !shown.contains(replacing) && !shown.contains(KEY),
        "{shown}"
    );
    assert_eq!(fs::read(home.key_file()).unwrap(), replacing.as_bytes());
    assert_eq!(mode(&home.key_file()), 0o600);
}

#[test]
fn giving_up_saves_nothing_fails_and_leaves_the_terminal_as_it_was() {
    // Enter alone, the end of the input, and Ctrl+C. The command ends by
    // itself each time, so that it gives the terminal back as it found it:
    // a shell that got it back without `signals` could no longer be
    // interrupted.
    for typed in ["\r", "\u{4}", "\u{3}"] {
        let home = Home::new();
        let (mut window, child) = Window::start(home.command(&["anthropic-key"]));
        assert_eq!(window.settings(), USUAL);
        window.wait_for(PROMPT);
        window.type_hidden(typed);
        let (status, shown, left) = window.finish(child);
        assert_eq!(left, USUAL, "{typed:?}");
        assert_eq!(status.code(), Some(1), "{typed:?} {status} {shown}");
        assert!(shown.contains("No key was saved."), "{typed:?} {shown}");
        assert!(!shown.contains(FINISH), "{shown}");
        assert!(home.untouched());
    }
}

#[test]
#[allow(
    unsafe_code,
    reason = "the standard library starts no session, and only a new session has no terminal"
)]
fn without_a_terminal_the_command_fails_and_changes_nothing() {
    let home = Home::new();
    let mut command = home.command(&["anthropic-key"]);
    command.stderr(Stdio::piped());
    // SAFETY: the closure runs in the child between `fork` and `exec`. It
    // calls only `setsid`, which is async-signal-safe, and allocates nothing.
    // A new session has no controlling terminal, whatever the terminal of
    // whoever runs the tests.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(
        message.contains("asks for the key in a terminal")
            && message.contains("Run it in a terminal window"),
        "{message}"
    );
    assert!(home.untouched());
}

#[test]
fn key_command_is_only_the_first_argument_alone() {
    let home = Home::new();
    // Neither asks for a key: the terminal is not even opened.
    for (arguments, expected) in [
        (&["anthropic-key", "extra"][..], "takes no arguments"),
        (
            &["--stdio", "anthropic-key"],
            "Unknown argument: anthropic-key",
        ),
    ] {
        let mut command = home.command(arguments);
        command.stderr(Stdio::piped());
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let message = String::from_utf8(output.stderr).unwrap();
        assert!(message.contains(expected), "{message}");
    }
    assert!(home.untouched());
}
