use std::{fs, path::Path};

use super::{PAUSED, translate};
use crate::{
    config::{AnthropicEffort, limits::TranslationLimits},
    providers::{
        Context, Intent, OutputOptions, TranslationProvider,
        anthropic::{
            AnthropicProvider, ModelOptions,
            key::ApiKey,
            test_support::{Fixture, KeyDirectory},
        },
        error::{ErrorKind, credentials::CredentialError},
    },
};

// Differs from the key of `Fixture::provider`, so a request shows where its key came from.
const FILE_KEY: &str = "sk-ant-synthetic-file-key";
const ENVIRONMENT_KEY: &str = "sk-ant-synthetic-environment-key";
const VARIABLES: [&str; 2] = ["TRANSLATION_LSP_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"];
const CHILD: &str = "providers::anthropic::tests::key::environment_child_process";

const MISSING: &str = "Anthropic API key file was not found.";
const NOT_PRIVATE: &str = "Anthropic API key file cannot be used.";
const EMPTY: &str = "Anthropic API key file is empty.";
const INVALID: &str = "Anthropic API key file must hold only the key";

fn provider(fixture: &Fixture, directory: &Path) -> AnthropicProvider {
    AnthropicProvider::synthetic(format!("{}/v1", fixture.base), None).with_key_directory(directory)
}

// What every request is answered with when the key in `directory` cannot be
// used. The API is not contacted.
async fn refusal(fixture: &Fixture, directory: &Path) -> String {
    let calls = fixture.calls();
    let provider = provider(fixture, directory);
    assert_eq!(provider.context().unwrap(), Context::Anthropic);
    let mut messages = Vec::new();
    for intent in [Intent::Automatic, Intent::Explicit] {
        for error in [
            provider.admit(&Context::Anthropic, intent).unwrap_err(),
            translate(&provider, intent).await.unwrap_err(),
        ] {
            assert_eq!(error.kind(), ErrorKind::Configuration);
            let message = error.to_string();
            assert!(message.contains("anthropic-api-key"), "{message}");
            assert!(message.contains("restart the language server"), "{message}");
            // Only a restart can supply the key, so no retry is suggested.
            assert!(!message.contains(PAUSED));
            assert!(format!("{error:?}").contains(&message));
            messages.push(message);
        }
    }
    assert_eq!(fixture.calls(), calls);
    messages.dedup();
    assert_eq!(messages.len(), 1);
    messages.remove(0)
}

fn names(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn key_file_supplies_the_key_and_is_left_as_it_was() {
    let fixture = Fixture::new().await;
    // The line ending of the file is not part of the key, whichever kind it is.
    for contents in [
        FILE_KEY.to_owned(),
        format!("{FILE_KEY}\n"),
        format!(" {FILE_KEY}\r\n"),
    ] {
        let directory = KeyDirectory::with_key(&contents);
        let before = fs::metadata(directory.file()).unwrap();
        let provider = provider(&fixture, &directory.path);
        translate(&provider, Intent::Explicit).await.unwrap();
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.records.last().unwrap().headers["x-api-key"], FILE_KEY);
        assert_eq!(names(&directory.path), ["anthropic-api-key"]);
        assert_eq!(fs::read(directory.file()).unwrap(), contents.as_bytes());
        let after = fs::metadata(directory.file()).unwrap();
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert_eq!(after.permissions(), before.permissions());
    }
    assert_eq!(fixture.calls(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn missing_key_file_is_reported_and_nothing_is_created() {
    let fixture = Fixture::new().await;
    let directory = KeyDirectory::new();
    assert!(
        refusal(&fixture, &directory.path)
            .await
            .starts_with(MISSING)
    );
    assert!(names(&directory.path).is_empty());

    let absent = directory.path.join("absent");
    assert!(refusal(&fixture, &absent).await.starts_with(MISSING));
    assert!(!absent.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn empty_and_malformed_key_files_are_told_apart_without_showing_their_contents() {
    let fixture = Fixture::new().await;
    for (contents, expected) in [
        (Vec::new(), EMPTY),
        (b" \r\n\t".to_vec(), EMPTY),
        (b"sk-ant synthetic".to_vec(), INVALID),
        (b"sk-ant-synthetic\nsecond-line".to_vec(), INVALID),
        ("sk-ant-合成".as_bytes().to_vec(), INVALID),
        // A byte order mark, as some editors write before UTF-8 text.
        (format!("\u{feff}{FILE_KEY}").into_bytes(), INVALID),
        // UTF-16, as some shells write a redirected string.
        (
            "\u{feff}sk-ant-synthetic"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
            INVALID,
        ),
        (b"synthetic-".repeat(1000), INVALID),
    ] {
        let directory = KeyDirectory::with_key(&contents);
        let message = refusal(&fixture, &directory.path).await;
        assert!(message.starts_with(expected), "{message}");
        if let Ok(text) = std::str::from_utf8(&contents)
            && !text.trim().is_empty()
        {
            assert!(!message.contains(text.trim()));
            assert!(!message.contains("synthetic"));
        }
        assert_eq!(fs::read(directory.file()).unwrap(), contents);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn directory_or_file_of_the_wrong_kind_is_refused() {
    let fixture = Fixture::new().await;
    let directory = KeyDirectory::new();
    fs::create_dir(directory.file()).unwrap();
    assert!(
        refusal(&fixture, &directory.path)
            .await
            .starts_with(NOT_PRIVATE)
    );

    let file = KeyDirectory::with_key(FILE_KEY);
    assert!(
        refusal(&fixture, &file.file())
            .await
            .starts_with(NOT_PRIVATE)
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn key_file_and_its_directory_must_be_private() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = Fixture::new().await;
    let set = |path: &Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    for (directory_mode, file_mode) in [
        (0o755, 0o600),
        (0o750, 0o600),
        (0o700, 0o644),
        (0o700, 0o640),
        // The modes are compared exactly, as the ChatGPT store compares them.
        (0o700, 0o400),
        (0o500, 0o600),
    ] {
        let directory = KeyDirectory::with_key(FILE_KEY);
        set(&directory.file(), file_mode);
        set(&directory.path, directory_mode);
        let message = refusal(&fixture, &directory.path).await;
        assert!(
            message.starts_with(NOT_PRIVATE),
            "{directory_mode:o} {file_mode:o} {message}"
        );
        assert!(message.contains("0700") && message.contains("0600"));
        // The server reports the modes and leaves them for the user to correct.
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&directory.path), directory_mode);
        set(&directory.path, 0o700);
        assert_eq!(mode(&directory.file()), file_mode);
        set(&directory.file(), 0o600);
        translate(&provider(&fixture, &directory.path), Intent::Explicit)
            .await
            .unwrap();
    }

    let real = KeyDirectory::with_key(FILE_KEY);
    let links = KeyDirectory::new();
    let linked_directory = links.path.join("directory");
    symlink(&real.path, &linked_directory).unwrap();
    let message = refusal(&fixture, &linked_directory).await;
    assert!(message.starts_with(NOT_PRIVATE), "{message}");
    assert!(message.contains("symbolic link"));
    symlink(real.file(), links.file()).unwrap();
    assert!(
        refusal(&fixture, &links.path)
            .await
            .starts_with(NOT_PRIVATE)
    );

    // Opening a named pipe waits for a writer, so it is refused before it is opened.
    let pipe = KeyDirectory::new();
    let made = std::process::Command::new("mkfifo")
        .args(["-m", "600"])
        .arg(pipe.file())
        .status()
        .unwrap();
    assert!(made.success());
    assert!(refusal(&fixture, &pipe.path).await.starts_with(NOT_PRIVATE));
}

#[test]
fn unlocated_directory_and_read_failure_are_settings_errors() {
    for (error, expected) in [
        (
            CredentialError::MissingDirectory,
            "Anthropic API key file could not be located",
        ),
        (
            CredentialError::io("inspect path", std::io::Error::other("synthetic failure")),
            "Anthropic API key file could not be read.",
        ),
    ] {
        let error = ApiKey::read(Err(error)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
        assert!(error.to_string().starts_with(expected), "{error}");
        assert!(!format!("{error:?}").contains("synthetic failure"));
    }
}

// The provider as the server builds it, with only the directory replaced.
fn production(directory: &Path) -> AnthropicProvider {
    AnthropicProvider::new(
        OutputOptions::new(
            ModelOptions {
                model: "claude-haiku-5-5".into(),
                effort: AnthropicEffort::Low,
                thinking: false,
            },
            None,
        ),
        "auto".into(),
        TranslationLimits::default().output_kib,
        Ok(directory.into()),
    )
    .unwrap()
}

#[test]
#[ignore = "child entry invoked by the environment test"]
fn environment_child_process() {
    for name in VARIABLES {
        assert_eq!(std::env::var(name).unwrap(), ENVIRONMENT_KEY);
    }
    let empty = KeyDirectory::new();
    let error = production(&empty.path).key().unwrap_err();
    assert!(error.to_string().starts_with(MISSING), "{error}");
    let directory = KeyDirectory::with_key(FILE_KEY);
    assert_eq!(&*production(&directory.path).key().unwrap().text, FILE_KEY);
}

// Environment variables belong to the whole process, so a child gets them.
#[test]
fn environment_variables_supply_no_key() {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child.args(["--ignored", "--exact", CHILD, "--nocapture"]);
    for name in VARIABLES {
        child.env(name, ENVIRONMENT_KEY);
    }
    let output = child.output().unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}{stderr}"
    );
}
