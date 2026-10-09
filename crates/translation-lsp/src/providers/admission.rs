use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{
    Intent,
    error::{Error, ErrorKind},
};

// Pauses automatic translation per provider key after a stop or temporary error.
// Each stop starts a new generation, so work admitted before it fails without a
// new request or a second notification.
pub(crate) struct Admission<K> {
    state: Arc<Mutex<State<K>>>,
}

struct State<K> {
    paused: Option<(K, Error, Option<Instant>)>,
    generation: u64,
    last_stop: Option<(K, Error)>,
}

pub(crate) struct Attempt<T> {
    pub result: Result<T, Error>,
    // The wait the response allows before one retry of a retryable failure.
    pub retry_after: Option<Duration>,
}

impl<T> Attempt<T> {
    pub fn without_response(error: Error) -> Self {
        Self {
            result: Err(error),
            retry_after: None,
        }
    }
}

impl<K> Clone for Admission<K> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<K> Default for Admission<K> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                paused: None,
                generation: 0,
                last_stop: None,
            })),
        }
    }
}

impl<K: PartialEq> State<K> {
    fn allowed(&self, key: &K) -> Result<(), Error> {
        if let Some((blocked, error, until)) = &self.paused
            && blocked == key
            && until.is_none_or(|until| until > Instant::now())
        {
            return Err(error.clone());
        }
        Ok(())
    }
}

impl<K: Clone + PartialEq> Admission<K> {
    pub fn admit(&self, key: &K, intent: Intent) -> Result<u64, Error> {
        let state = self.state.lock().unwrap();
        if intent == Intent::Automatic {
            state.allowed(key)?;
        }
        Ok(state.generation)
    }

    pub fn check(&self, key: &K, generation: u64) -> Result<(), Error> {
        let state = self.state.lock().unwrap();
        if generation < state.generation
            && let Some((stopped, error)) = &state.last_stop
            && stopped == key
        {
            let mut error = error.clone();
            error.suppress_notification();
            return Err(error);
        }
        Ok(())
    }

    pub fn record_error(&self, key: &K, error: &mut Error, generation: u64) {
        let pause = match error.kind() {
            ErrorKind::UsageLimit | ErrorKind::NotEligible | ErrorKind::Configuration => Some(None),
            ErrorKind::Temporary => Some(Some(Instant::now() + Duration::from_secs(2))),
            _ => None,
        };
        if let Some(until) = pause {
            if until.is_none() {
                error.require_explicit_retry();
            }
            let mut state = self.state.lock().unwrap();
            if until.is_none() {
                if generation < state.generation {
                    error.suppress_notification();
                    return;
                }
                state.generation += 1;
                state.last_stop = Some((key.clone(), error.clone()));
            }
            if until.is_some()
                && state
                    .paused
                    .as_ref()
                    .is_some_and(|(blocked, _, deadline)| blocked == key && deadline.is_none())
            {
                return;
            }
            state.paused = Some((key.clone(), error.clone(), until));
        }
    }

    // Only a successful explicit translation from the current generation resumes
    // automatic translation.
    pub fn settle<T>(
        &self,
        key: &K,
        intent: Intent,
        generation: u64,
        result: &mut Result<T, Error>,
    ) {
        if let Err(error) = result {
            self.record_error(key, error, generation);
        } else if intent == Intent::Explicit {
            let mut state = self.state.lock().unwrap();
            if state.generation == generation
                && state
                    .paused
                    .as_ref()
                    .is_some_and(|(blocked, _, _)| blocked == key)
            {
                state.paused = None;
            }
        }
    }

    // Sends the request again at most once, and only after a failure that
    // `Error::retryable` accepts.
    pub async fn retrying<T>(
        &self,
        key: &K,
        intent: Intent,
        mut attempt: impl AsyncFnMut() -> Attempt<T>,
    ) -> Result<T, Error> {
        let mut retried = false;
        loop {
            let Attempt {
                result,
                retry_after,
            } = attempt().await;
            match result {
                Err(error) if !retried && error.retryable() => {
                    let Some(delay) = retry_after else {
                        return Err(error);
                    };
                    retried = true;
                    // The worker's existing deadline and cancellation cover this wait too.
                    tokio::time::sleep(delay).await;
                    if intent == Intent::Automatic {
                        self.state.lock().unwrap().allowed(key)?;
                    }
                }
                result => return result,
            }
        }
    }
}
