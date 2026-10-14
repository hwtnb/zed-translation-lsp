use std::{
    fs,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, SystemTime},
};

use tokio::sync::Semaphore;

use super::{PAUSED, automatic, translate};
use crate::{
    config::{AnthropicEffort, limits::TranslationLimits},
    providers::{
        AccountChange, AuthUiState, Intent, Interaction, Output, OutputOptions,
        TranslationProvider,
        anthropic::{
            AnthropicProvider, ModelOptions, REMOVE, SAVE,
            key::{KeyFile, terminal},
            test_support::{Fixture, KEY, KeyDirectory, Reply, Terminal, completed},
        },
        error::{Error, ErrorKind, Notification, credentials::CredentialError},
    },
};

// Differs from the key of `Fixture::provider`, so a request shows where its key came from.
const FILE_KEY: &str = "sk-ant-synthetic-file-key";
const ENVIRONMENT_KEY: &str = "sk-ant-synthetic-environment-key";
const VARIABLES: [&str; 2] = ["TRANSLATION_LSP_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"];
const CHILD: &str = "providers::anthropic::tests::key::environment_child_process";

const MENU: &str = "Run \"Anthropic API key…\" from the code actions";
const MISSING: &str = "Anthropic API key is not saved.";
const NOT_PRIVATE: &str = "Anthropic API key file cannot be used.";
const EMPTY: &str = "Anthropic API key file is empty.";
const INVALID: &str = "Anthropic API key file does not hold a key";

fn provider(fixture: &Fixture, directory: &Path) -> AnthropicProvider {
    AnthropicProvider::synthetic(format!("{}/v1", fixture.base), directory)
}

// The key the last request to the fixture carried.
fn sent(fixture: &Fixture) -> String {
    let state = fixture.state.lock().unwrap();
    let key = &state.records.last().unwrap().headers["x-api-key"];
    key.to_str().unwrap().to_owned()
}

// What every request is answered with when the key in `directory` cannot be
// used. The API is not contacted.
async fn refusal(fixture: &Fixture, directory: &Path) -> String {
    let calls = fixture.calls();
    let provider = provider(fixture, directory);
    assert_eq!(provider.auth_ui_state(), AuthUiState::SignedOut);
    let mut messages = Vec::new();
    for error in [
        provider.context().unwrap_err(),
        translate(&provider, Intent::Automatic).await.unwrap_err(),
        translate(&provider, Intent::Explicit).await.unwrap_err(),
    ] {
        assert_eq!(error.kind(), ErrorKind::Configuration);
        let message = error.to_string();
        // The key is read again at the next request, so no restart is asked for
        // and no explicit translation is needed to resume.
        assert!(!message.contains("restart"), "{message}");
        assert!(!message.contains(PAUSED));
        assert!(format!("{error:?}").contains("Configuration"));
        messages.push(message);
    }
    assert_eq!(fixture.calls(), calls);
    messages.dedup();
    assert_eq!(messages.len(), 1);
    messages.remove(0)
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
        assert_eq!(provider.auth_ui_state(), AuthUiState::Ready);
        translate(&provider, Intent::Explicit).await.unwrap();
        assert_eq!(sent(&fixture), FILE_KEY);
        assert_eq!(directory.names(), ["anthropic-api-key"]);
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
    let message = refusal(&fixture, &directory.path).await;
    assert_eq!(message, format!("{MISSING} {MENU} to save it"));
    assert!(directory.names().is_empty());

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
        assert!(message.ends_with(&format!("{MENU} to save the key")));
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
        let error = KeyFile::new(Err(error)).read().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration);
        assert!(error.to_string().starts_with(expected), "{error}");
        assert!(!format!("{error:?}").contains("synthetic failure"));
    }
}

// The same length as `FILE_KEY`, so that the size of the file tells nothing.
const NEXT_KEY: &str = "sk-ant-synthetic-next-key";

// The key that the file is found to hold now.
fn held(file: &KeyFile) -> Result<String, Error> {
    file.read().map(|(key, _)| key.text.to_string())
}

// Writes into the file that is there, as an editor that saves in place does.
fn rewrite(path: &Path, contents: &str) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    std::io::Write::write_all(&mut file, contents.as_bytes()).unwrap();
}

fn set_modified(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

// Gives the file a modification time long past, whatever the clock of the
// file system counts in, and returns the time as the file system kept it.
fn age(path: &Path) -> SystemTime {
    set_modified(path, SystemTime::now() - Duration::from_secs(3600));
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn settled_file_with_the_same_attributes_is_not_opened_again() {
    assert_eq!(FILE_KEY.len(), NEXT_KEY.len());
    let directory = KeyDirectory::with_key(FILE_KEY);
    let written = age(&directory.file());
    let file = KeyFile::new(Ok(directory.path.clone()));
    let (_, first) = file.read().unwrap();

    // Nothing writes a file like this: another key of the same length in the
    // same file, with the old time put back. Only opening the file could tell,
    // so the key from before shows that it was not opened.
    rewrite(&directory.file(), NEXT_KEY);
    set_modified(&directory.file(), written);
    assert_eq!(fs::read(directory.file()).unwrap(), NEXT_KEY.as_bytes());
    for _ in 0..3 {
        let (key, id) = file.read().unwrap();
        assert_eq!((&*key.text, id), (FILE_KEY, first));
    }

    // Any other time than the one remembered has the file opened.
    set_modified(&directory.file(), written - Duration::from_secs(60));
    let (key, second) = file.read().unwrap();
    assert_eq!(&*key.text, NEXT_KEY);
    assert_ne!(second, first);
}

#[test]
fn file_written_a_moment_ago_is_opened_every_time() {
    let directory = KeyDirectory::with_key(FILE_KEY);
    let file = KeyFile::new(Ok(directory.path.clone()));
    // Two keys of one length written in turn, faster than the modification
    // time of some file systems ticks: neither the time nor the size has to
    // differ from one to the next.
    for round in 0..40 {
        let key = if round % 2 == 0 { NEXT_KEY } else { FILE_KEY };
        rewrite(&directory.file(), key);
        assert_eq!(held(&file).unwrap(), key, "round {round}");
    }
    // The same holds for a time ahead of the clock, however long it stays.
    set_modified(
        &directory.file(),
        SystemTime::now() + Duration::from_secs(3600),
    );
    let ahead = fs::metadata(directory.file()).unwrap().modified().unwrap();
    assert_eq!(held(&file).unwrap(), FILE_KEY);
    rewrite(&directory.file(), NEXT_KEY);
    set_modified(&directory.file(), ahead);
    assert_eq!(held(&file).unwrap(), NEXT_KEY);
}

#[test]
fn remembered_key_gives_way_to_a_file_replaced_or_written_in_place() {
    let directory = KeyDirectory::with_key(FILE_KEY);
    let file = KeyFile::new(Ok(directory.path.clone()));
    let remember = |key: &str| {
        age(&directory.file());
        assert_eq!(held(&file).unwrap(), key);
        assert_eq!(held(&file).unwrap(), key);
    };

    // Replaced by a rename, as the key command saves.
    remember(FILE_KEY);
    directory.replace(NEXT_KEY);
    assert_eq!(held(&file).unwrap(), NEXT_KEY);

    // Written in place with the same length, then with another.
    remember(NEXT_KEY);
    rewrite(&directory.file(), FILE_KEY);
    assert_eq!(held(&file).unwrap(), FILE_KEY);
    remember(FILE_KEY);
    rewrite(&directory.file(), KEY);
    assert_eq!(held(&file).unwrap(), KEY);

    // The same key written again is still the same key.
    let (_, before) = file.read().unwrap();
    remember(KEY);
    directory.replace(&format!("{KEY}\n"));
    assert_eq!(file.read().unwrap().1, before);
}

#[test]
fn remembered_key_is_not_used_once_the_file_is_gone_or_of_another_kind() {
    let directory = KeyDirectory::with_key(FILE_KEY);
    let file = KeyFile::new(Ok(directory.path.clone()));
    age(&directory.file());
    let (_, first) = file.read().unwrap();

    fs::remove_file(directory.file()).unwrap();
    assert!(held(&file).unwrap_err().to_string().starts_with(MISSING));
    fs::create_dir(directory.file()).unwrap();
    assert!(
        held(&file)
            .unwrap_err()
            .to_string()
            .starts_with(NOT_PRIVATE)
    );
    fs::remove_dir(directory.file()).unwrap();

    // The key that comes back is read, and counts as another.
    directory.replace(FILE_KEY);
    age(&directory.file());
    let (key, second) = file.read().unwrap();
    assert_eq!(&*key.text, FILE_KEY);
    assert_ne!(second, first);
}

#[cfg(unix)]
#[test]
fn remembered_key_is_not_used_once_the_file_or_its_directory_stops_being_private() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = KeyDirectory::with_key(FILE_KEY);
    let file = KeyFile::new(Ok(directory.path.clone()));
    let set = |path: &Path, mode| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };
    let refused = || {
        let error = held(&file).unwrap_err().to_string();
        assert!(error.starts_with(NOT_PRIVATE), "{error}");
    };
    // The checks come before the remembered key, each time it is asked for.
    let remember = || {
        age(&directory.file());
        assert_eq!(held(&file).unwrap(), FILE_KEY);
        assert_eq!(held(&file).unwrap(), FILE_KEY);
    };

    remember();
    set(&directory.file(), 0o644);
    refused();
    set(&directory.file(), 0o600);

    remember();
    set(&directory.path, 0o755);
    refused();
    set(&directory.path, 0o700);

    remember();
    let elsewhere = KeyDirectory::with_key(FILE_KEY);
    fs::remove_file(directory.file()).unwrap();
    symlink(elsewhere.file(), directory.file()).unwrap();
    refused();
    fs::remove_file(directory.file()).unwrap();

    directory.replace(FILE_KEY);
    remember();
}

// The file is read for every request: a key saved, replaced, or removed while
// the server runs is the key of the next request, and counts as another key.
#[tokio::test(flavor = "current_thread")]
async fn key_saved_replaced_or_removed_is_followed_without_a_restart() {
    let fixture = Fixture::new().await;
    let directory = KeyDirectory::new();
    let provider = provider(&fixture, &directory.path);
    assert!(translate(&provider, Intent::Explicit).await.is_err());

    directory.replace(FILE_KEY);
    let first = provider.context().unwrap();
    translate(&provider, Intent::Explicit).await.unwrap();
    assert_eq!(sent(&fixture), FILE_KEY);
    // The same key read again is the same context, whatever surrounds it.
    directory.replace(&format!("{FILE_KEY}\n"));
    assert_eq!(provider.context().unwrap(), first);

    directory.replace(KEY);
    let second = provider.context().unwrap();
    assert_ne!(second, first);
    translate(&provider, Intent::Explicit).await.unwrap();
    assert_eq!(sent(&fixture), KEY);

    fs::remove_file(directory.file()).unwrap();
    let error = translate(&provider, Intent::Explicit).await.unwrap_err();
    assert!(error.to_string().starts_with(MISSING), "{error}");
    assert_eq!(provider.auth_ui_state(), AuthUiState::SignedOut);
    // A key that comes back after none could be used is not the key from before.
    directory.replace(KEY);
    let third = provider.context().unwrap();
    assert!(third != first && third != second);
    assert_eq!(fixture.calls(), 2);

    // The context shows a count, and nothing of the key.
    for context in [first, second, third] {
        let shown = format!("{context:?}");
        assert!(shown.starts_with("Anthropic(KeyId("), "{shown}");
        assert!(!shown.contains("sk-ant"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn translation_prepared_under_another_key_is_refused_without_a_request() {
    let fixture = Fixture::new().await;
    let provider = fixture.provider();
    let prepared = provider.context().unwrap();
    fixture.keys.replace(FILE_KEY);
    let error = provider
        .translate(
            Intent::Explicit,
            Output::Range,
            "Read this.",
            "ja",
            prepared,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind(), ErrorKind::Modified);
    assert_eq!(
        error.to_string(),
        "Anthropic API key changed; retry the preview"
    );
    assert_eq!(fixture.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn stop_for_a_rejected_key_is_not_carried_to_the_key_that_replaces_it() {
    let fixture = Fixture::new().await;
    fixture.always(Some(Reply::failure(
        401,
        "authentication_error",
        "invalid x-api-key",
    )));
    let provider = fixture.provider();
    let rejected = provider.context().unwrap();
    let admitted = automatic(&provider).unwrap();
    let error = translate(&provider, Intent::Automatic).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    let message = error.to_string();
    assert!(
        message.contains(&format!(
            "The API key was rejected. {MENU} to replace it. {PAUSED}"
        )),
        "{message}"
    );
    assert_eq!(
        automatic(&provider).unwrap_err().kind(),
        ErrorKind::Configuration
    );
    assert!(provider.check_admission(&rejected, admitted).is_err());

    fixture.keys.replace(FILE_KEY);
    fixture.always(None);
    let replaced = provider.context().unwrap();
    // Neither the pause nor the work it drained belongs to the new key.
    automatic(&provider).unwrap();
    provider.check_admission(&replaced, admitted).unwrap();
    assert_eq!(
        translate(&provider, Intent::Automatic).await.unwrap(),
        format!("訳{}", super::SOURCE)
    );
    assert_eq!(sent(&fixture), FILE_KEY);
    assert_eq!(fixture.calls(), 2);
}

// Holds the responses to requests that carry `key`, or to every request, until
// the test adds a permit.
fn gate(fixture: &Fixture, key: Option<&'static str>) -> Arc<Semaphore> {
    let gate = Arc::new(Semaphore::new(0));
    let mut state = fixture.state.lock().unwrap();
    (state.gate, state.gated_key) = (Some(gate.clone()), key);
    gate
}

// Runs `pending` until the fixture has received `calls` requests.
async fn sent_while_pending<T>(
    fixture: &Fixture,
    calls: usize,
    mut pending: Pin<&mut impl Future<Output = T>>,
) {
    let arrival = async {
        while fixture.calls() < calls {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::select! {
        _ = &mut pending => panic!("A held request must not complete"),
        arrived = tokio::time::timeout(Duration::from_secs(30), arrival) => {
            arrived.expect("The request did not reach the fixture");
        }
    }
}

fn rejection() -> Reply {
    Reply::failure(401, "authentication_error", "invalid x-api-key")
}

#[tokio::test(flavor = "current_thread")]
async fn late_result_of_a_replaced_key_leaves_the_stop_of_the_key_in_use() {
    for (intent, late, kind) in [
        (
            Intent::Automatic,
            Reply::failure(529, "overloaded_error", "Overloaded"),
            Some(ErrorKind::Temporary),
        ),
        (
            Intent::Automatic,
            rejection(),
            Some(ErrorKind::Configuration),
        ),
        (Intent::Explicit, Reply::stream(completed("遅れた訳")), None),
    ] {
        let fixture = Fixture::new().await;
        // A temporary failure is not sent again, so it ends the request.
        fixture.state.lock().unwrap().retry_after = Some("121".into());
        let gate = gate(&fixture, Some(KEY));
        let provider = fixture.provider();
        let delayed = translate(&provider, intent);
        tokio::pin!(delayed);
        sent_while_pending(&fixture, 1, delayed.as_mut()).await;

        fixture.keys.replace(FILE_KEY);
        fixture.reply(rejection());
        let admitted = automatic(&provider).unwrap();
        let stop = translate(&provider, Intent::Automatic).await.unwrap_err();
        assert_eq!(stop.kind(), ErrorKind::Configuration);
        assert_eq!(sent(&fixture), FILE_KEY);

        fixture.reply(late);
        gate.add_permits(1);
        let result = delayed.await;
        assert_eq!(result.as_ref().err().map(Error::kind), kind, "{result:?}");
        if let Err(error) = result {
            // The stop of the key in use is no notice of this failure.
            assert_eq!(error.notification(), Notification::Show);
        }

        let context = provider.context().unwrap();
        for refused in [
            automatic(&provider).unwrap_err(),
            translate(&provider, Intent::Automatic).await.unwrap_err(),
            provider.check_admission(&context, admitted).unwrap_err(),
        ] {
            assert_eq!(refused.to_string(), stop.to_string());
        }
        assert_eq!(fixture.calls(), 2);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn late_stop_of_a_replaced_key_is_not_a_stop_of_the_key_in_use() {
    let fixture = Fixture::new().await;
    let gate = gate(&fixture, None);
    let provider = fixture.provider();
    let delayed = translate(&provider, Intent::Automatic);
    tokio::pin!(delayed);
    sent_while_pending(&fixture, 1, delayed.as_mut()).await;

    fixture.keys.replace(FILE_KEY);
    let context = provider.context().unwrap();
    let admitted = automatic(&provider).unwrap();
    let running = translate(&provider, Intent::Automatic);
    tokio::pin!(running);
    sent_while_pending(&fixture, 2, running.as_mut()).await;

    // The fixture answers in the order the requests arrived.
    fixture.reply(rejection());
    gate.add_permits(1);
    assert_eq!(delayed.await.unwrap_err().kind(), ErrorKind::Configuration);
    automatic(&provider).unwrap();
    provider.check_admission(&context, admitted).unwrap();

    // The first stop of the key in use is still shown, and still pauses it.
    fixture.reply(rejection());
    gate.add_permits(1);
    let stop = running.await.unwrap_err();
    assert_eq!(stop.notification(), Notification::Show);
    assert!(stop.to_string().contains(PAUSED), "{stop}");
    assert_eq!(
        automatic(&provider).unwrap_err().kind(),
        ErrorKind::Configuration
    );
    assert!(provider.check_admission(&context, admitted).is_err());
    assert_eq!(fixture.calls(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_resend_is_not_sent_once_the_key_is_replaced_or_removed() {
    for removed in [false, true] {
        let fixture = Fixture::new().await;
        // The resend is due as soon as the failure arrives, so the key changes
        // while the failure is held back.
        fixture.state.lock().unwrap().retry_after = Some("0".into());
        fixture.reply(Reply::failure(529, "overloaded_error", "Overloaded"));
        let gate = gate(&fixture, None);
        let provider = fixture.provider();
        let prepared = provider.context().unwrap();
        let admitted = automatic(&provider).unwrap();
        let waiting = translate(&provider, Intent::Automatic);
        tokio::pin!(waiting);
        sent_while_pending(&fixture, 1, waiting.as_mut()).await;

        if removed {
            fs::remove_file(fixture.keys.file()).unwrap();
        } else {
            fixture.keys.replace(FILE_KEY);
        }
        // A wrongly repeated request would be answered at once with a translation.
        gate.add_permits(2);
        let error = waiting.await.unwrap_err();
        assert_eq!(fixture.calls(), 1);
        let message = error.to_string();
        if removed {
            assert_eq!(error.kind(), ErrorKind::Configuration);
            assert!(message.starts_with(MISSING), "{message}");
            fixture.keys.replace(FILE_KEY);
        } else {
            assert_eq!(error.kind(), ErrorKind::Modified);
            assert_eq!(message, "Anthropic API key changed; retry the preview");
        }
        assert!(!message.contains(PAUSED), "{message}");

        // Neither the key the request was prepared with nor the new one is
        // paused or stopped.
        provider.admit(&prepared, Intent::Automatic).unwrap();
        provider.check_admission(&prepared, admitted).unwrap();
        automatic(&provider).unwrap();
        assert_eq!(
            translate(&provider, Intent::Automatic).await.unwrap(),
            format!("訳{}", super::SOURCE)
        );
        assert_eq!(sent(&fixture), FILE_KEY);
        assert_eq!(fixture.calls(), 2);
    }
}

fn titles(provider: &AnthropicProvider) -> Vec<&'static str> {
    provider
        .account_commands()
        .iter()
        .map(|command| command.title)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn menu_offers_saving_without_a_usable_key_and_replacing_or_removing_with_one() {
    let capabilities = AnthropicProvider::capabilities();
    assert_eq!(capabilities.account_menu, Some("Anthropic API key…"));
    let commands: Vec<_> = capabilities
        .commands
        .iter()
        .map(|command| (command.command, command.changes_account))
        .collect();
    // Saving happens in the terminal; removing asks first.
    assert_eq!(
        commands,
        [
            (SAVE, AccountChange::Never),
            (REMOVE, AccountChange::IfConfirmed)
        ]
    );

    let fixture = Fixture::new().await;
    let directory = KeyDirectory::new();
    let provider = provider(&fixture, &directory.path);
    assert_eq!(titles(&provider), ["Save Anthropic API key"]);
    // A file that is there and cannot be used is saved over, like no file.
    fs::write(directory.file(), "not a key").unwrap();
    assert_eq!(titles(&provider), ["Save Anthropic API key"]);
    directory.replace(FILE_KEY);
    assert_eq!(
        titles(&provider),
        ["Replace Anthropic API key", "Remove Anthropic API key"]
    );
    assert_eq!(provider.account_commands()[0].command, SAVE);
    assert_eq!(provider.account_commands()[1].command, REMOVE);
}

// Answers a choice the way a test says, and remembers what was asked.
#[derive(Default)]
struct Editor {
    accepts: bool,
    asked: Mutex<Vec<(String, Vec<String>)>>,
}

impl Interaction for Editor {
    async fn open(&self, _uri: &str) -> Result<(), Error> {
        panic!("The key commands open no page")
    }

    async fn choose(&self, message: &str, choices: &[String]) -> Result<Option<String>, Error> {
        self.asked
            .lock()
            .unwrap()
            .push((message.to_owned(), choices.to_vec()));
        Ok(self.accepts.then(|| choices[0].clone()))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn saving_opens_a_terminal_for_this_executable_and_names_the_command_to_run_by_hand() {
    let fixture = Fixture::new().await;
    let terminal = Terminal::default();
    let provider = fixture.provider().with_terminal(terminal.clone());
    let editor = Editor::default();
    let executable = std::env::current_exe().unwrap();
    let command = terminal::manual_command(&executable);
    assert!(command.ends_with(" anthropic-key"), "{command}");

    let notice = provider.administer(SAVE, &editor).await.unwrap();
    assert_eq!(
        notice,
        format!(
            "A terminal window was opened. Enter the Anthropic API key there. If no window appeared, run this in a terminal: {command}"
        )
    );
    assert_eq!(*terminal.opened.lock().unwrap(), [executable]);

    terminal.fails.store(true, Ordering::SeqCst);
    let error = provider.administer(SAVE, &editor).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Failed);
    assert_eq!(
        error.to_string(),
        format!(
            "No terminal window could be opened. To save the Anthropic API key, run this in a terminal: {command}"
        )
    );
    assert_eq!(terminal.opened.lock().unwrap().len(), 2);

    // Opening the terminal asks nothing, changes nothing, and sends nothing.
    assert!(editor.asked.lock().unwrap().is_empty());
    assert_eq!(fixture.keys.names(), ["anthropic-api-key"]);
    assert_eq!(fs::read(fixture.keys.file()).unwrap(), KEY.as_bytes());
    assert_eq!(fixture.calls(), 0);
    for text in [notice, error.to_string(), format!("{error:?}")] {
        assert!(!text.contains(KEY));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn removing_asks_first_and_deletes_only_the_key_file() {
    let fixture = Fixture::new().await;
    let provider = fixture.provider();
    // What the ChatGPT store keeps in the same directory.
    for name in ["auth.json", "auth.lock"] {
        fs::write(fixture.keys.path.join(name), "synthetic").unwrap();
    }
    let all = ["anthropic-api-key", "auth.json", "auth.lock"];

    let declining = Editor::default();
    assert_eq!(
        provider.administer(REMOVE, &declining).await.unwrap(),
        "The Anthropic API key was not removed."
    );
    assert_eq!(
        *declining.asked.lock().unwrap(),
        [(
            concat!(
                "Remove the saved Anthropic API key from this computer? The key stays valid ",
                "until you delete it in the Claude Console."
            )
            .to_owned(),
            vec!["Remove".to_owned()]
        )]
    );
    assert_eq!(fixture.keys.names(), all);
    translate(&provider, Intent::Explicit).await.unwrap();

    let accepting = Editor {
        accepts: true,
        ..Default::default()
    };
    assert_eq!(
        provider.administer(REMOVE, &accepting).await.unwrap(),
        "The Anthropic API key was removed."
    );
    assert_eq!(fixture.keys.names(), ["auth.json", "auth.lock"]);
    for name in ["auth.json", "auth.lock"] {
        assert_eq!(
            fs::read(fixture.keys.path.join(name)).unwrap(),
            b"synthetic"
        );
    }
    let error = translate(&provider, Intent::Explicit).await.unwrap_err();
    assert!(error.to_string().starts_with(MISSING), "{error}");
    assert_eq!(titles(&provider), ["Save Anthropic API key"]);
    assert_eq!(fixture.calls(), 1);

    assert_eq!(
        provider.administer(REMOVE, &accepting).await.unwrap(),
        "No Anthropic API key is saved."
    );
    assert!(provider.administer("unknown", &accepting).await.is_err());
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn removing_refuses_what_is_not_a_private_directory_with_a_regular_file() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = Fixture::new().await;
    let accepting = Editor {
        accepts: true,
        ..Default::default()
    };
    let target = KeyDirectory::with_key(FILE_KEY);
    let linked = KeyDirectory::new();
    symlink(target.file(), linked.file()).unwrap();
    let loose = KeyDirectory::with_key(FILE_KEY);
    fs::set_permissions(&loose.path, fs::Permissions::from_mode(0o755)).unwrap();
    for directory in [&linked, &loose] {
        let error = provider(&fixture, &directory.path)
            .administer(REMOVE, &accepting)
            .await
            .unwrap_err();
        assert!(error.to_string().starts_with(NOT_PRIVATE), "{error}");
        assert_eq!(directory.names(), ["anthropic-api-key"]);
    }
    assert_eq!(fs::read(target.file()).unwrap(), FILE_KEY.as_bytes());
}

// The provider as the server builds it, with only the directory replaced.
fn production(directory: &Path) -> AnthropicProvider {
    AnthropicProvider::new(
        OutputOptions::new(
            ModelOptions {
                model: "claude-haiku-5-5".into(),
                effort: AnthropicEffort::Low,
                thinking: false,
                speed: None,
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
    let error = production(&empty.path).key.read().unwrap_err();
    assert!(error.to_string().starts_with(MISSING), "{error}");
    let directory = KeyDirectory::with_key(FILE_KEY);
    let (key, _) = production(&directory.path).key.read().unwrap();
    assert_eq!(&*key.text, FILE_KEY);
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
