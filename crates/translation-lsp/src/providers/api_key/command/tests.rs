use std::{collections::VecDeque, fs, io, path::Path};

use super::*;
use crate::providers::{
    Intent, Output, TranslationProvider,
    anthropic::{
        AnthropicProvider,
        test_support::{Fixture, KEY, KeyDirectory},
    },
};

const NEW_KEY: &str = "sk-ant-synthetic-entered-key";

/// A terminal whose user types `lines`, then ends the input.
#[derive(Default)]
struct Typed {
    lines: VecDeque<String>,
    shown: String,
    reads: usize,
}

impl Typed {
    fn new(lines: &[&str]) -> Self {
        Self {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            ..Default::default()
        }
    }

    /// Runs the command for `directory`. Nothing typed may be shown.
    fn run(lines: &[&str], directory: &Path) -> (bool, Self) {
        Self::run_for(None, lines, directory)
    }

    fn run_for(destination: Option<&Origin>, lines: &[&str], directory: &Path) -> (bool, Self) {
        let mut console = Self::new(lines);
        let saved = super::run(Ok(directory.into()), destination, &mut console);
        for line in lines.iter().filter(|line| !line.trim().is_empty()) {
            assert!(!console.shown.contains(line.trim()), "{}", console.shown);
        }
        (saved, console)
    }
}

impl Console for Typed {
    fn say(&mut self, text: &str) -> io::Result<()> {
        self.shown.push_str(text);
        Ok(())
    }

    fn read_hidden(&mut self) -> io::Result<String> {
        self.reads += 1;
        self.lines
            .pop_front()
            .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
    }
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The key the provider sends with its next request.
async fn sent(fixture: &Fixture, provider: &impl TranslationProvider) -> String {
    provider
        .translate(
            Intent::Explicit,
            Output::Range,
            "Read this.",
            "ja",
            provider.context().unwrap(),
        )
        .await
        .map(|_| ())
        .unwrap();
    let state = fixture.state.lock().unwrap();
    let key = &state.records.last().unwrap().headers["x-api-key"];
    key.to_str().unwrap().to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn entered_key_is_saved_privately_and_sent_with_the_next_request() {
    let fixture = Fixture::new().await;
    let parent = KeyDirectory::new();
    // The credential directory does not exist before the first key is saved.
    let directory = parent.path.join("translation-lsp");
    let provider = AnthropicProvider::synthetic(format!("{}/v1", fixture.base), &directory);
    assert!(provider.context().is_err());

    let (saved, console) = Typed::run(&[&format!("  {NEW_KEY} ")], &directory);
    assert!(saved);
    assert_eq!(
        fs::read(directory.join("anthropic-api-key")).unwrap(),
        NEW_KEY.as_bytes()
    );
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    #[cfg(unix)]
    {
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join("anthropic-api-key")), 0o600);
    }
    for expected in [
        "The key is saved in the file anthropic-api-key in ",
        "sends it to the Anthropic API and nowhere else",
        "Create the key for a single workspace in the Claude Console",
        "https://platform.claude.com/settings/keys",
        "Paste the Anthropic API key and press Enter (input is hidden)",
        "The key was saved. Return to Zed: the next translation uses it.",
        "Press Enter to finish.",
    ] {
        assert!(console.shown.contains(expected), "{}", console.shown);
    }
    assert!(!console.shown.contains("replaces"));
    // The key, then the Enter that ends the command.
    assert_eq!(console.reads, 2);
    // The provider that was running before the key was saved now uses it.
    assert_eq!(sent(&fixture, &provider).await, NEW_KEY);
}

#[tokio::test(flavor = "current_thread")]
async fn entered_key_replaces_the_saved_one_whatever_its_permissions() {
    let fixture = Fixture::new().await;
    let provider = fixture.provider();
    assert_eq!(sent(&fixture, &provider).await, KEY);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(fixture.keys.file(), fs::Permissions::from_mode(0o644)).unwrap();
    }

    let (saved, console) = Typed::run(&[NEW_KEY], &fixture.keys.path);
    assert!(saved);
    assert!(
        console
            .shown
            .contains("A key is saved there now. The key you enter replaces it."),
        "{}",
        console.shown
    );
    assert!(!console.shown.contains(KEY));
    assert_eq!(fixture.keys.names(), ["anthropic-api-key"]);
    assert_eq!(fs::read(fixture.keys.file()).unwrap(), NEW_KEY.as_bytes());
    #[cfg(unix)]
    assert_eq!(mode(&fixture.keys.file()), 0o600);
    assert_eq!(sent(&fixture, &provider).await, NEW_KEY);
}

#[test]
fn input_that_is_not_a_key_is_asked_for_again_and_saves_nothing() {
    let long = "k".repeat(4097);
    let rejected = [
        (
            "sk-ant synthetic",
            "a key has no whitespace or non-ASCII characters",
        ),
        (
            "sk-ant-合成",
            "a key has no whitespace or non-ASCII characters",
        ),
        (
            "sk-ant\tsynthetic",
            "a key has no whitespace or non-ASCII characters",
        ),
        (long.as_str(), "it is longer than 4096 bytes"),
    ];
    let parent = KeyDirectory::new();
    let absent = parent.path.join("translation-lsp");
    let existing = KeyDirectory::with_key(KEY);
    for (line, reason) in rejected {
        // Giving up leaves things as they were, by Enter alone or by ending
        // the input.
        for ending in [Some(""), Some(" \t"), None] {
            let lines: Vec<&str> = [line].into_iter().chain(ending).collect();
            let (saved, console) = Typed::run(&lines, &absent);
            assert!(!saved);
            assert!(!absent.exists());
            assert!(
                console
                    .shown
                    .contains(&format!("That is not a key: {reason}")),
                "{}",
                console.shown
            );
            assert!(console.shown.ends_with("\nNo key was saved.\n"));
            assert_eq!(
                console.shown.matches("Paste the Anthropic API key").count(),
                2
            );
            assert!(!console.shown.contains("Press Enter to finish"));

            let (saved, _) = Typed::run(&lines, &existing.path);
            assert!(!saved);
            assert_eq!(fs::read(existing.file()).unwrap(), KEY.as_bytes());
            assert_eq!(existing.names(), ["anthropic-api-key"]);
        }
        let (saved, console) = Typed::run(&[line, NEW_KEY], &existing.path);
        assert!(saved);
        assert_eq!(console.reads, 3);
        assert_eq!(fs::read(existing.file()).unwrap(), NEW_KEY.as_bytes());
        existing.replace(KEY);
    }

    // Nothing typed at all.
    for lines in [&[""][..], &[]] {
        let (saved, console) = Typed::run(lines, &absent);
        assert!(!saved);
        assert!(!absent.exists());
        assert_eq!(console.reads, 1);
        assert!(console.shown.ends_with("\nNo key was saved.\n"));
    }
}

/// Runs the command for a place that cannot take the key. It must say so before
/// it asks for the key, and wait for the user to have read it.
fn refused(directory: Result<&Path, CredentialError>) -> String {
    let mut console = Typed::new(&[NEW_KEY]);
    let saved = super::run(directory.map(Path::to_owned), None, &mut console);
    assert!(!saved);
    assert!(!console.shown.contains("Paste the Anthropic API key"));
    assert!(!console.shown.contains(NEW_KEY));
    assert!(console.shown.ends_with("Press Enter to finish."));
    assert_eq!(console.reads, 1);
    console.shown
}

#[test]
fn unlocated_directory_and_wrong_kinds_are_refused_before_the_key_is_asked_for() {
    let shown = refused(Err(CredentialError::MissingDirectory));
    assert!(shown.contains(
        "The key was not saved. The OS reported no local configuration directory for this user."
    ));

    // A file where the directory belongs, and a directory where the file belongs.
    let file = KeyDirectory::with_key(KEY);
    let directory = KeyDirectory::new();
    fs::create_dir(directory.file()).unwrap();
    for place in [file.file(), directory.path.clone()] {
        let shown = refused(Ok(&place));
        assert!(shown.contains("The key was not saved. "), "{shown}");
        assert!(shown.contains(" must be a directory"));
        assert!(shown.contains("a regular file if it exists, neither a symbolic link."));
    }
    assert_eq!(fs::read(file.file()).unwrap(), KEY.as_bytes());
    assert!(directory.file().is_dir());
}

#[cfg(unix)]
#[test]
fn loose_directory_symbolic_links_and_named_pipe_are_refused_and_left_alone() {
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        process::Command,
    };

    let loose = KeyDirectory::with_key(KEY);
    fs::set_permissions(&loose.path, fs::Permissions::from_mode(0o755)).unwrap();
    let shown = refused(Ok(&loose.path));
    assert!(
        shown.contains("must be a directory with mode 0700"),
        "{shown}"
    );
    // The permissions are the user's to correct.
    assert_eq!(mode(&loose.path), 0o755);
    assert_eq!(fs::read(loose.file()).unwrap(), KEY.as_bytes());

    let target = KeyDirectory::with_key(KEY);
    let links = KeyDirectory::new();
    let linked_directory = links.path.join("directory");
    symlink(&target.path, &linked_directory).unwrap();
    refused(Ok(&linked_directory));
    symlink(target.file(), links.file()).unwrap();
    refused(Ok(&links.path));
    assert!(fs::symlink_metadata(links.file()).unwrap().is_symlink());
    assert_eq!(fs::read(target.file()).unwrap(), KEY.as_bytes());
    assert_eq!(target.names(), ["anthropic-api-key"]);

    let pipe = KeyDirectory::new();
    let made = Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(pipe.file())
        .status()
        .unwrap();
    assert!(made.success());
    refused(Ok(&pipe.path));
    assert!(!fs::symlink_metadata(pipe.file()).unwrap().is_file());
    assert_eq!(pipe.names(), ["anthropic-api-key"]);

    // Saving looks again: the place may have changed since the key was asked for.
    let key = ApiKey::parse(NEW_KEY).unwrap();
    for directory in [&loose, &links, &pipe] {
        assert!(matches!(
            save(&directory.path, "anthropic-api-key", &key),
            Err(CredentialError::Permissions)
        ));
    }
    assert_eq!(links.names(), ["anthropic-api-key", "directory"]);
    assert!(fs::symlink_metadata(links.file()).unwrap().is_symlink());
    assert!(!fs::symlink_metadata(pipe.file()).unwrap().is_file());
    assert_eq!(fs::read(loose.file()).unwrap(), KEY.as_bytes());
    assert_eq!(fs::read(target.file()).unwrap(), KEY.as_bytes());
}

#[test]
fn every_text_for_the_terminal_is_ascii() {
    let origin = Origin::openai();
    for text in [
        introduction(None),
        introduction(Some(&origin)),
        prompt(Some(&origin)),
        not_private("api-key-https-api.openai.com-443"),
        REPLACES.into(),
        MALFORMED.into(),
        TOO_LONG.into(),
        CANCELLED.into(),
        SAVED.into(),
        NOT_SAVED.into(),
        NO_DIRECTORY.into(),
        FINISH.into(),
        ORIGIN_ARGUMENT.into(),
    ] {
        assert!(text.is_ascii(), "{text}");
    }
}

#[test]
fn key_for_an_origin_is_saved_in_a_file_of_its_own_and_names_where_it_goes() {
    let directory = KeyDirectory::with_key(KEY);
    let openrouter = Origin::parse("https://openrouter.ai/api/v1").unwrap();
    let (saved, console) = Typed::run_for(Some(&openrouter), &[NEW_KEY], &directory.path);
    assert!(saved);
    for expected in [
        "Translation LSP: API key for https://openrouter.ai\n",
        "The key is saved in the file api-key-https-openrouter.ai-443 in ",
        "The language server sends it to https://openrouter.ai and nowhere else.",
        "Paste the API key and press Enter (input is hidden)",
    ] {
        assert!(console.shown.contains(expected), "{}", console.shown);
    }
    assert!(!console.shown.contains("replaces"));
    assert!(!console.shown.contains("OpenAI Platform"));
    assert_eq!(
        directory.names(),
        ["anthropic-api-key", "api-key-https-openrouter.ai-443"]
    );
    assert_eq!(
        fs::read(directory.path.join("api-key-https-openrouter.ai-443")).unwrap(),
        NEW_KEY.as_bytes()
    );
    // The Anthropic key is another file.
    assert_eq!(fs::read(directory.file()).unwrap(), KEY.as_bytes());

    // The OpenAI key is told where to be created, and replaced like any other.
    let openai = Origin::openai();
    for (key, replaces) in [(NEW_KEY, false), (KEY, true)] {
        let (saved, console) = Typed::run_for(Some(&openai), &[key], &directory.path);
        assert!(saved);
        assert!(
            console
                .shown
                .contains("https://platform.openai.com/api-keys")
        );
        assert_eq!(console.shown.contains("replaces it"), replaces);
    }
    assert_eq!(
        fs::read(directory.path.join("api-key-https-api.openai.com-443")).unwrap(),
        KEY.as_bytes()
    );
}
