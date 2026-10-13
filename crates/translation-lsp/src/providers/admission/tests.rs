use std::time::Instant;

use super::Admission;
use crate::providers::{
    Intent,
    error::{Error, ErrorKind, Notification, RemoteError, RemoteKind, Service},
};

const REPLACED: u8 = 1;
const IN_USE: u8 = 2;

fn stop() -> Error {
    Error::configuration("synthetic stop")
}

fn temporary() -> Error {
    RemoteError {
        service: Service::Anthropic,
        kind: RemoteKind::Temporary,
        status: 529,
        detail: "Overloaded".into(),
        code: None,
        param: None,
        request_id: None,
        advice: None,
    }
    .into()
}

fn automatic(admission: &Admission<u8>, key: u8) -> Result<u64, Error> {
    admission.admit(&key, Intent::Automatic)
}

fn explicit(admission: &Admission<u8>, key: u8) -> u64 {
    admission.admit(&key, Intent::Explicit).unwrap()
}

// The error as the work admitted at `generation` reports it.
fn fail(admission: &Admission<u8>, key: u8, generation: u64, error: Error) -> Error {
    let mut result: Result<(), Error> = Err(error);
    admission.settle(&key, Intent::Automatic, generation, &mut result);
    result.unwrap_err()
}

fn succeed(admission: &Admission<u8>, key: u8, intent: Intent, generation: u64) {
    admission.settle(&key, intent, generation, &mut Ok(()));
}

fn keys(admission: &Admission<u8>) -> Vec<u8> {
    let state = admission.state.lock().unwrap();
    state.keys.iter().map(|(key, _)| *key).collect()
}

#[test]
fn late_result_of_one_key_keeps_the_stop_of_another() {
    for (late, intent) in [
        (Some(temporary()), Intent::Automatic),
        (Some(stop()), Intent::Automatic),
        (None, Intent::Explicit),
    ] {
        let admission = Admission::default();
        let delayed = admission.admit(&REPLACED, intent).unwrap();
        let drained = automatic(&admission, IN_USE).unwrap();
        fail(&admission, IN_USE, drained, stop());

        match late {
            Some(error) => {
                let kind = error.kind();
                let reported = fail(&admission, REPLACED, delayed, error);
                // The stop of the other key is no notice of this failure.
                assert_eq!(reported.notification(), Notification::Show);
                assert_eq!(automatic(&admission, REPLACED).unwrap_err().kind(), kind);
            }
            None => succeed(&admission, REPLACED, intent, delayed),
        }

        assert_eq!(
            automatic(&admission, IN_USE).unwrap_err().kind(),
            ErrorKind::Configuration
        );
        let suppressed = admission.check(&IN_USE, drained).unwrap_err();
        assert_eq!(suppressed.notification(), Notification::Suppress);
    }
}

#[test]
fn stop_of_one_key_keeps_the_stop_of_a_key_stopped_before_it() {
    let admission = Admission::default();
    let drained = automatic(&admission, IN_USE).unwrap();
    fail(&admission, IN_USE, drained, stop());
    let fresh = automatic(&admission, REPLACED).unwrap();
    for error in [temporary(), stop()] {
        fail(&admission, REPLACED, fresh, error);
        assert_eq!(
            automatic(&admission, IN_USE).unwrap_err().kind(),
            ErrorKind::Configuration
        );
        assert!(admission.check(&IN_USE, drained).is_err());
    }
    assert!(admission.check(&REPLACED, fresh).is_err());
}

#[test]
fn stop_of_one_key_leaves_the_work_of_another_as_it_was() {
    let admission = Admission::default();
    let running = automatic(&admission, IN_USE).unwrap();
    let late = automatic(&admission, REPLACED).unwrap();
    fail(&admission, REPLACED, late, stop());

    automatic(&admission, IN_USE).unwrap();
    admission.check(&IN_USE, running).unwrap();
    // Its own stop is its first: it is shown, and it pauses the key.
    let reported = fail(&admission, IN_USE, running, stop());
    assert_eq!(reported.notification(), Notification::Show);
    assert!(automatic(&admission, IN_USE).is_err());

    // An explicit translation admitted before the other key stopped again
    // still resumes its own key.
    let retried = explicit(&admission, IN_USE);
    let again = explicit(&admission, REPLACED);
    fail(&admission, REPLACED, again, stop());
    succeed(&admission, IN_USE, Intent::Explicit, retried);
    automatic(&admission, IN_USE).unwrap();
    assert!(automatic(&admission, REPLACED).is_err());
}

#[test]
fn explicit_success_resumes_only_its_own_key_and_only_after_its_last_stop() {
    let admission = Admission::default();
    for key in [REPLACED, IN_USE] {
        let generation = automatic(&admission, key).unwrap();
        fail(&admission, key, generation, stop());
    }
    let early = explicit(&admission, IN_USE);
    let stopped_again = fail(&admission, IN_USE, early, stop());
    assert_eq!(stopped_again.notification(), Notification::Show);
    succeed(&admission, IN_USE, Intent::Explicit, early);
    assert!(automatic(&admission, IN_USE).is_err());
    succeed(
        &admission,
        IN_USE,
        Intent::Automatic,
        explicit(&admission, IN_USE),
    );
    assert!(automatic(&admission, IN_USE).is_err());

    succeed(
        &admission,
        IN_USE,
        Intent::Explicit,
        explicit(&admission, IN_USE),
    );
    automatic(&admission, IN_USE).unwrap();
    assert!(automatic(&admission, REPLACED).is_err());
    // Work admitted before the stops stays drained after the key resumed.
    assert!(admission.check(&IN_USE, 0).is_err());
}

// The server gives queued work the generation an explicit request was admitted
// with under the key in use, whichever key the work was prepared with.
#[test]
fn generation_admitted_under_one_key_is_held_against_the_stop_of_another() {
    let admission = Admission::default();
    let before = explicit(&admission, IN_USE);
    let drained = automatic(&admission, REPLACED).unwrap();
    fail(&admission, REPLACED, drained, stop());
    let after = explicit(&admission, IN_USE);

    assert!(admission.check(&REPLACED, before).is_err());
    admission.check(&REPLACED, after).unwrap();
    for generation in [before, after] {
        admission.check(&IN_USE, generation).unwrap();
    }
}

#[test]
fn temporary_failure_pauses_its_key_without_replacing_its_stop() {
    let admission = Admission::default();
    let generation = automatic(&admission, IN_USE).unwrap();
    fail(&admission, IN_USE, generation, temporary());
    assert_eq!(
        automatic(&admission, IN_USE).unwrap_err().kind(),
        ErrorKind::Temporary
    );
    automatic(&admission, REPLACED).unwrap();
    explicit(&admission, IN_USE);
    // No generation starts: the work admitted before is not drained.
    admission.check(&IN_USE, generation).unwrap();

    fail(&admission, IN_USE, generation, stop());
    fail(
        &admission,
        IN_USE,
        explicit(&admission, IN_USE),
        temporary(),
    );
    assert_eq!(
        automatic(&admission, IN_USE).unwrap_err().kind(),
        ErrorKind::Configuration
    );
}

#[test]
fn only_paused_and_stopped_keys_are_remembered() {
    let admission = Admission::default();
    for key in 0..50 {
        let generation = automatic(&admission, key).unwrap();
        fail(&admission, key, generation, temporary());
        succeed(&admission, key, Intent::Automatic, generation);
        fail(
            &admission,
            key,
            generation,
            Error::failed("synthetic failure"),
        );
    }
    assert_eq!(keys(&admission).len(), 50);

    // The pauses run out, which the next failure of any key finds.
    let now = Instant::now();
    for (_, standing) in &mut admission.state.lock().unwrap().keys {
        let (_, until) = standing.paused.as_mut().unwrap();
        *until = Some(now);
    }
    for key in 0..50 {
        automatic(&admission, key).unwrap();
    }
    let stopped = automatic(&admission, IN_USE).unwrap();
    fail(&admission, IN_USE, stopped, stop());
    assert_eq!(keys(&admission), [IN_USE]);

    // A stop is kept after the key resumed, for the work it drained.
    succeed(
        &admission,
        IN_USE,
        Intent::Explicit,
        explicit(&admission, IN_USE),
    );
    fail(&admission, REPLACED, 0, temporary());
    assert_eq!(keys(&admission), [IN_USE, REPLACED]);
    assert!(admission.check(&IN_USE, stopped).is_err());
}
