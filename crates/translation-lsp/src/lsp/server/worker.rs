use std::future::Future;

use tokio::time::sleep_until;

use crate::providers::{
    Intent, Source,
    unset::{NO_SETTINGS_FILE, settings_file},
};

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
    /// A menu of server commands, each a title and the command it runs.
    Menu {
        ui: Ui,
        message: String,
        choices: Vec<(String, String)>,
    },
    /// Opens Zed's settings file in the editor.
    OpenSettings {
        ui: Ui,
    },
    Translation {
        intent: Intent,
        output: Output,
        generation: u64,
        source: Source,
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
    /// The command chosen from the account menu or another menu, or none.
    AccountChoice(Option<String>),
    /// An operation that has nothing to tell.
    Done,
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
            source,
            target_language,
            context,
        } => {
            let started = || finished.send(WorkerEvent::Started(ticket)).is_ok();
            let result = until_deadline(&mut deadline, started, async {
                provider.check_admission(&context, generation)?;
                match &source {
                    Source::Passage(text) => {
                        provider
                            .translate(intent, output, text, &target_language, context.clone())
                            .await
                    }
                    Source::Term { term, line } => {
                        provider
                            .look_up(
                                intent,
                                output,
                                term,
                                line,
                                &target_language,
                                context.clone(),
                            )
                            .await
                    }
                }
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
        Task::Menu {
            ui,
            message,
            choices,
        } => {
            let result = until_deadline(&mut deadline, || true, async {
                let titles: Vec<_> = choices.iter().map(|(title, _)| title.clone()).collect();
                ui.choose(&message, &titles).await.map(|choice| {
                    Administered::AccountChoice(choice.and_then(|title| {
                        choices
                            .into_iter()
                            .find(|(choice, _)| *choice == title)
                            .map(|(_, command)| command)
                    }))
                })
            })
            .await;
            Finished::Administration(Completion { ticket, result })
        }
        Task::OpenSettings { ui } => {
            let result = until_deadline(&mut deadline, || true, async {
                let unopened = || ProviderError::failed(NO_SETTINGS_FILE);
                let path = settings_file().ok_or_else(unopened)?;
                ui.edit(&path).await.map_err(|_| unopened())?;
                Ok(Administered::Done)
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
