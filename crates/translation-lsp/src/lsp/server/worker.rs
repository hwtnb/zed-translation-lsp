use super::*;

const TRANSLATION_CONCURRENCY: usize = 2;
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
    jobs: mpsc::Receiver<Job>,
    administrations: mpsc::Receiver<Job>,
    finished: Sender<WorkerEvent>,
    provider: P,
) {
    tokio::join!(
        run_jobs(
            administrations,
            finished.clone(),
            provider.clone(),
            ADMIN_CONCURRENCY,
        ),
        run_jobs(jobs, finished, provider, TRANSLATION_CONCURRENCY),
    );
}

async fn run_jobs<P: TranslationProvider>(
    jobs: mpsc::Receiver<Job>,
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
    mut job: Job,
    finished: &Sender<WorkerEvent>,
    provider: &P,
) {
    let Some(deadline) = *job.deadline.borrow_and_update() else {
        return;
    };
    if deadline <= Instant::now() {
        return;
    }
    if matches!(job.task, Task::Translation { .. })
        && finished.send(WorkerEvent::Started(job.ticket)).is_err()
    {
        return;
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
                text,
                target_language,
                context,
            } => provider
                .translate(*intent, text, target_language, context.clone())
                .await
                .map(Outcome::Translation),
        }
    };
    // Keep the same operation alive when a waiter extends or shortens its deadline.
    tokio::pin!(operation);
    let timer = tokio::time::sleep_until(deadline.into());
    tokio::pin!(timer);
    let result = loop {
        tokio::select! {
            biased;
            changed = job.deadline.changed() => {
                if changed.is_err() {
                    return;
                }
                let Some(deadline) = *job.deadline.borrow_and_update() else {
                    return;
                };
                timer.as_mut().reset(deadline.into());
            }
            _ = &mut timer => break Err(ProviderError::failed("Translation timed out")),
            result = &mut operation => break result,
        }
    };
    let _ = finished.send(WorkerEvent::Finished(Finished {
        ticket: job.ticket,
        result,
    }));
}
