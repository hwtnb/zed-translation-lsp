use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::time::sleep;

use super::{
    Intent,
    error::{Error, ErrorKind},
};

/// Pauses automatic translation per provider key after a stop or temporary error.
/// Each stop starts a new generation, so work admitted for its key before it
/// fails without a new request or a second notification.
pub(crate) struct Admission<K> {
    state: Arc<Mutex<State<K>>>,
}

struct State<K> {
    /// Counted for all keys together, so that a generation admitted under one
    /// key can be held against the stop of another: an explicit request that
    /// joins queued work gives it the generation of the key in use.
    generation: u64,
    /// Only keys that are paused or were stopped. A stopped key stays: its stop
    /// has no end, and work admitted before it may still be queued.
    keys: Vec<(K, Standing)>,
}

#[derive(Default)]
struct Standing {
    /// Without an instant, until an explicit translation succeeds.
    paused: Option<(Error, Option<Instant>)>,
    /// The generation the last stop started.
    stop: Option<(u64, Error)>,
}

pub(crate) struct Attempt<T> {
    pub result: Result<T, Error>,
    /// The wait the response allows before one retry of a retryable failure.
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
                generation: 0,
                keys: Vec::new(),
            })),
        }
    }
}

impl Standing {
    fn allowed(&self, now: Instant) -> Result<(), Error> {
        match &self.paused {
            Some((error, until)) if until.is_none_or(|until| until > now) => Err(error.clone()),
            _ => Ok(()),
        }
    }

    fn stopped_after(&self, generation: u64) -> Option<&Error> {
        let (started, error) = self.stop.as_ref()?;
        (generation < *started).then_some(error)
    }
}

impl<K: PartialEq> State<K> {
    fn standing(&self, key: &K) -> Option<&Standing> {
        self.keys
            .iter()
            .find_map(|(owner, standing)| (owner == key).then_some(standing))
    }

    fn allowed(&self, key: &K) -> Result<(), Error> {
        self.standing(key)
            .map_or(Ok(()), |standing| standing.allowed(Instant::now()))
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
        if let Some(error) = state
            .standing(key)
            .and_then(|standing| standing.stopped_after(generation))
        {
            let mut error = error.clone();
            error.suppress_notification();
            return Err(error);
        }
        Ok(())
    }

    pub fn record_error(&self, key: &K, error: &mut Error, generation: u64) {
        let now = Instant::now();
        let until = match error.kind() {
            ErrorKind::UsageLimit | ErrorKind::NotEligible | ErrorKind::Configuration => None,
            ErrorKind::Temporary => Some(now + Duration::from_secs(2)),
            _ => return,
        };
        if until.is_none() {
            error.require_explicit_retry();
        }
        let mut state = self.state.lock().unwrap();
        let state = &mut *state;
        // A key that was replaced is not asked about again, so a pause that ran
        // out is dropped here.
        state
            .keys
            .retain(|(_, standing)| standing.stop.is_some() || standing.allowed(now).is_err());
        let index = state
            .keys
            .iter()
            .position(|(owner, _)| owner == key)
            .unwrap_or_else(|| {
                state.keys.push((key.clone(), Standing::default()));
                state.keys.len() - 1
            });
        let standing = &mut state.keys[index].1;
        if until.is_none() {
            if standing.stopped_after(generation).is_some() {
                error.suppress_notification();
                return;
            }
            state.generation += 1;
            standing.stop = Some((state.generation, error.clone()));
        } else if matches!(standing.paused, Some((_, None))) {
            return;
        }
        standing.paused = Some((error.clone(), until));
    }

    /// Only a successful explicit translation admitted after the last stop of
    /// its key resumes automatic translation.
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
            if let Some((_, standing)) = state.keys.iter_mut().find(|(owner, _)| owner == key)
                && standing.stopped_after(generation).is_none()
            {
                standing.paused = None;
            }
        }
    }

    /// Sends the request again at most once, and only after a failure that
    /// `Error::retryable` accepts.
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
                    sleep(delay).await;
                    if intent == Intent::Automatic {
                        self.state.lock().unwrap().allowed(key)?;
                    }
                }
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests;
