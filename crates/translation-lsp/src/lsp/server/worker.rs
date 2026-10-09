use super::*;

const ADMIN_CONCURRENCY: usize = 1;

pub(super) struct Job {
    pub(super) ticket: u64,
    pub(super) deadline: watch::Receiver<Option<Instant>>,
    pub(super) task: Task,
}

pub(super) enum Task {
    AccountMenu {
        ui: Ui,
    },
    Translation {
        intent: crate::providers::Intent,
        output: Output,
        generation: u64,
        text: String,
        target_language: String,
        context: Context,
    },
    Administration {
        command: String,
        ui: Ui,
    },
}

pub(super) struct Finished {
    pub(super) ticket: u64,
    pub(super) result: Result<Outcome, ProviderError>,
}

pub(super) enum WorkerEvent {
    Started(u64),
    Finished(Finished),
}

pub(super) enum Outcome {
    AccountChoice(Option<String>),
    Translation(Translated),
    Administration(String),
}

pub(super) async fn worker<P: TranslationProvider>(
    jobs: JobReceiver,
    administrations: JobReceiver,
    finished: Sender<WorkerEvent>,
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
    jobs: JobReceiver,
    finished: Sender<WorkerEvent>,
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
    job: Job,
    finished: &Sender<WorkerEvent>,
    provider: &P,
) {
    let ticket = job.ticket;
    let result = run_operation(job, finished, provider).await;
    // Every dispatched job returns its slot, including cancellation before start.
    let _ = finished.send(WorkerEvent::Finished(Finished { ticket, result }));
}

async fn run_operation<P: TranslationProvider>(
    mut job: Job,
    finished: &Sender<WorkerEvent>,
    provider: &P,
) -> Result<Outcome, ProviderError> {
    let Some(deadline) = *job.deadline.borrow_and_update() else {
        return Err(ProviderError::cancelled());
    };
    if deadline <= Instant::now() {
        return Err(ProviderError::cancelled());
    }
    if matches!(job.task, Task::Translation { .. })
        && finished.send(WorkerEvent::Started(job.ticket)).is_err()
    {
        return Err(ProviderError::cancelled());
    }
    let operation = async {
        match &job.task {
            Task::AccountMenu { ui } => {
                let commands = provider.account_commands();
                let titles: Vec<_> = commands
                    .iter()
                    .map(|command| command.title.to_owned())
                    .collect();
                ui.choose("", &titles).await.map(|choice| {
                    Outcome::AccountChoice(choice.and_then(|title| {
                        commands
                            .into_iter()
                            .find(|command| command.title == title)
                            .map(|command| command.command.to_owned())
                    }))
                })
            }
            Task::Administration { command, ui } => provider
                .administer(command, ui)
                .await
                .map(Outcome::Administration),
            Task::Translation {
                intent,
                output,
                text,
                target_language,
                context,
                generation,
            } => {
                provider.check_admission(context, *generation)?;
                provider
                    .translate(*intent, *output, text, target_language, context.clone())
                    .await
                    .map(Outcome::Translation)
            }
        }
    };
    // Keep the same operation alive when a waiter extends or shortens its deadline.
    tokio::pin!(operation);
    let timer = tokio::time::sleep_until(deadline.into());
    tokio::pin!(timer);
    loop {
        tokio::select! {
            biased;
            changed = job.deadline.changed() => {
                if changed.is_err() {
                    return Err(ProviderError::cancelled());
                }
                let Some(deadline) = *job.deadline.borrow_and_update() else {
                    return Err(ProviderError::cancelled());
                };
                timer.as_mut().reset(deadline.into());
            }
            _ = &mut timer => break Err(ProviderError::failed("Translation timed out")),
            result = &mut operation => break result,
        }
    }
}
