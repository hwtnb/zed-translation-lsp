use std::future::Future;

use tokio::time::sleep_until;

use crate::providers::Intent;

use super::*;

const ADMIN_CONCURRENCY: usize = 1;

pub(super) struct Job<C> {
    pub(super) ticket: u64,
    pub(super) deadline: watch::Receiver<Option<Instant>>,
    pub(super) task: Task<C>,
}

pub(super) enum Task<C> {
    AccountMenu {
        ui: Ui,
    },
    Translation {
        intent: Intent,
        output: Output,
        generation: u64,
        text: String,
        target_language: String,
        context: C,
    },
    Administration {
        command: String,
        ui: Ui,
    },
}

/// A job that ended, with the result of its kind of work.
pub(super) enum Finished<C> {
    Translation(Completion<Translated<C>>),
    Administration(Completion<Administered>),
}

pub(super) struct Completion<T> {
    pub(super) ticket: u64,
    pub(super) result: Result<T, ProviderError>,
}

/// What an account operation gives.
pub(super) enum Administered {
    /// The command chosen from the account menu, or none.
    AccountChoice(Option<String>),
    /// A message for the user.
    Message(String),
}

impl<C> Finished<C> {
    pub(super) fn ticket(&self) -> u64 {
        match self {
            Self::Translation(completion) => completion.ticket,
            Self::Administration(completion) => completion.ticket,
        }
    }

    #[cfg(test)]
    pub(super) fn error(&self) -> Option<&ProviderError> {
        match self {
            Self::Translation(completion) => completion.result.as_ref().err(),
            Self::Administration(completion) => completion.result.as_ref().err(),
        }
    }

    #[cfg(test)]
    pub(super) fn is_ok(&self) -> bool {
        self.error().is_none()
    }
}

pub(super) enum WorkerEvent<C> {
    Started(u64),
    Finished(Finished<C>),
}

pub(super) async fn worker<P: TranslationProvider>(
    jobs: JobReceiver<P::Context>,
    administrations: JobReceiver<P::Context>,
    finished: Sender<WorkerEvent<P::Context>>,
    provider: P,
    concurrency: usize,
) {
    tokio::join!(
        run_jobs(
            administrations,
            finished.clone(),
            provider.clone(),
            ADMIN_CONCURRENCY,
        ),
        run_jobs(jobs, finished, provider, concurrency),
    );
}

async fn run_jobs<P: TranslationProvider>(
    jobs: JobReceiver<P::Context>,
    finished: Sender<WorkerEvent<P::Context>>,
    provider: P,
    concurrency: usize,
) {
    stream::unfold(jobs, |mut jobs| async move {
        jobs.recv().await.map(|job| (job, jobs))
    })
    .for_each_concurrent(concurrency, |job| run_job(job, &finished, &provider))
    .await;
}

pub(super) async fn run_job<P: TranslationProvider>(
    job: Job<P::Context>,
    finished: &Sender<WorkerEvent<P::Context>>,
    provider: &P,
) {
    let Job {
        ticket,
        mut deadline,
        task,
    } = job;
    let event = match task {
        Task::Translation {
            intent,
            output,
            generation,
            text,
            target_language,
            context,
        } => {
            let started = || finished.send(WorkerEvent::Started(ticket)).is_ok();
            let result = until_deadline(&mut deadline, started, async {
                provider.check_admission(&context, generation)?;
                provider
                    .translate(intent, output, &text, &target_language, context.clone())
                    .await
            })
            .await;
            Finished::Translation(Completion { ticket, result })
        }
        Task::AccountMenu { ui } => {
            let result = until_deadline(&mut deadline, || true, async {
                let commands = provider.account_commands();
                let titles: Vec<_> = commands
                    .iter()
                    .map(|command| command.title.to_owned())
                    .collect();
                ui.choose("", &titles).await.map(|choice| {
                    Administered::AccountChoice(choice.and_then(|title| {
                        commands
                            .into_iter()
                            .find(|command| command.title == title)
                            .map(|command| command.command.to_owned())
                    }))
                })
            })
            .await;
            Finished::Administration(Completion { ticket, result })
        }
        Task::Administration { command, ui } => {
            let result = until_deadline(&mut deadline, || true, async {
                provider
                    .administer(&command, &ui)
                    .await
                    .map(Administered::Message)
            })
            .await;
            Finished::Administration(Completion { ticket, result })
        }
    };
    // Every dispatched job returns its slot, including cancellation before start.
    let _ = finished.send(WorkerEvent::Finished(event));
}

/// Runs `operation` until it completes or its deadline passes. `start` runs
/// once the deadline is known to be ahead; `false` from it cancels the job.
async fn until_deadline<T>(
    deadline: &mut watch::Receiver<Option<Instant>>,
    start: impl FnOnce() -> bool,
    operation: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    let Some(current) = *deadline.borrow_and_update() else {
        return Err(ProviderError::cancelled());
    };
    if current <= Instant::now() || !start() {
        return Err(ProviderError::cancelled());
    }
    // Keep the same operation alive when a waiter extends or shortens its deadline.
    tokio::pin!(operation);
    let timer = sleep_until(current.into());
    tokio::pin!(timer);
    loop {
        tokio::select! {
            biased;
            changed = deadline.changed() => {
                if changed.is_err() {
                    return Err(ProviderError::cancelled());
                }
                let Some(current) = *deadline.borrow_and_update() else {
                    return Err(ProviderError::cancelled());
                };
                timer.as_mut().reset(current.into());
            }
            _ = &mut timer => break Err(ProviderError::failed("Translation timed out")),
            result = &mut operation => break result,
        }
    }
}
