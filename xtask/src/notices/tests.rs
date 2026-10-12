use std::cell::Cell;

use super::*;

const NOTICES: &str = "Third-party licenses for Translation LSP\n\nlibm 0.2.16\n";

// Lines as cargo-about 0.9.2 writes them, with its colors.
const CHANGED: &str = "2026-10-12T20:36:03.806369Z [\u{1b}[33mWARN\u{1b}[0m] failed to validate all files specified in clarification for crate libm 0.2.16: checksum mismatch, expected '39'\u{1b}[0m\n";
const NOT_DOWNLOADED: &str = "2026-10-12T20:35:35.156804Z [\u{1b}[33mWARN\u{1b}[0m] failed to validate all files specified in clarification for crate tree-sitter-cpp 0.23.4: unable to retrieve 'LICENSE' for crate 'tree-sitter-cpp 0.23.4' from remote git host: failed to fetch contents of 'LICENSE' from repo 'https://github.com/tree-sitter/tree-sitter-cpp': failed to send request: io: Connection refused\u{1b}[0m\n";

fn output(standard: &[&str], warnings: &str) -> Output {
    let listed: String = standard
        .iter()
        .map(|item| format!("standard text: {item}\n"))
        .collect();
    Output {
        text: listed + NOTICES,
        warnings: warnings.to_owned(),
    }
}

// Runs `retry` on these outputs in turn and counts the runs and the pauses.
fn generate(outputs: Vec<Output>) -> (Result<Vec<u8>>, usize, usize) {
    let runs = Cell::new(0);
    let pauses = Cell::new(0);
    let mut outputs = outputs.into_iter();
    let result = retry(
        || {
            runs.set(runs.get() + 1);
            Ok(outputs.next().expect("No further run was expected"))
        },
        || pauses.set(pauses.get() + 1),
    );
    (result, runs.get(), pauses.get())
}

#[test]
fn the_crates_under_a_standard_text_are_listed_before_the_notices() {
    assert_eq!(split(NOTICES), (vec![], NOTICES));
    assert_eq!(
        split(&format!(
            "standard text: a 1.0.0\nstandard text: b-c 2.0.0\r\n{NOTICES}"
        )),
        (vec!["a 1.0.0", "b-c 2.0.0"], NOTICES)
    );
}

#[test]
fn only_the_listed_crates_may_stand_under_a_standard_text() {
    assert_eq!(unexpected(&[]), Vec::<&str>::new());
    assert_eq!(
        unexpected(&["eventsource-stream 0.2.3"]),
        Vec::<&str>::new()
    );
    assert_eq!(
        unexpected(&[
            "eventsource-stream 0.2.3",
            "libm 0.2.16",
            "eventsource-stream-x 1.0.0"
        ]),
        ["libm 0.2.16", "eventsource-stream-x 1.0.0"]
    );
}

#[test]
fn notices_with_the_text_of_every_author_are_returned_without_the_list() {
    let (result, runs, pauses) = generate(vec![output(&["eventsource-stream 0.2.3"], "")]);
    assert_eq!(result.unwrap(), NOTICES.as_bytes());
    assert_eq!((runs, pauses), (1, 0));
}

#[test]
fn a_changed_license_file_fails_at_once_and_names_the_crate() {
    let (result, runs, pauses) = generate(vec![output(&["libm 0.2.16"], CHANGED)]);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("authors, for: libm 0.2.16\n"), "{error}");
    assert!(
        error.contains("  libm 0.2.16: checksum mismatch, expected '39'\n"),
        "{error}"
    );
    assert!(
        error.contains("update its checksum in about.toml"),
        "{error}"
    );
    assert_eq!((runs, pauses), (1, 0));
}

#[test]
fn a_crate_that_about_toml_does_not_name_fails_at_once() {
    let (result, runs, pauses) = generate(vec![output(&["new-crate 1.0.0"], "")]);
    let error = result.unwrap_err().to_string();
    assert!(error.contains("authors, for: new-crate 1.0.0\n"), "{error}");
    assert_eq!((runs, pauses), (1, 0));
}

#[test]
fn a_failed_download_is_tried_again() {
    let (result, runs, pauses) = generate(vec![
        output(&["tree-sitter-cpp 0.23.4"], NOT_DOWNLOADED),
        output(&["tree-sitter-cpp 0.23.4"], NOT_DOWNLOADED),
        output(&[], ""),
    ]);
    assert_eq!(result.unwrap(), NOTICES.as_bytes());
    assert_eq!((runs, pauses), (3, 2));
}

#[test]
fn a_download_that_keeps_failing_ends_and_names_the_network() {
    let failed = || output(&["tree-sitter-cpp 0.23.4"], NOT_DOWNLOADED);
    let (result, runs, pauses) = generate(vec![failed(), failed(), failed()]);
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("authors, for: tree-sitter-cpp 0.23.4\n"),
        "{error}"
    );
    assert!(error.contains("io: Connection refused\n"), "{error}");
    assert!(error.contains("needs network access"), "{error}");
    assert_eq!((runs, pauses), (3, 2));
}
