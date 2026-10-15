use super::*;

pub(super) struct Pending {
    pub ticket: u64,
    pub deadline: Instant,
    pub position: Position,
}

pub(super) enum PendingReply {
    Hover(Position),
    Command,
    Retry,
}

pub(super) enum TranslationRequest {
    Reply(RequestId, PendingReply),
    Selection,
    Document,
}

impl TranslationRequest {
    pub fn explicit(&self) -> bool {
        matches!(
            self,
            Self::Document | Self::Reply(_, PendingReply::Command | PendingReply::Retry)
        )
    }

    pub fn output(&self) -> Output {
        if matches!(self, Self::Document) {
            Output::Document
        } else {
            Output::Range
        }
    }

    pub fn reuses_completed(&self) -> bool {
        !matches!(self, Self::Reply(_, PendingReply::Retry))
    }

    fn timeout(&self, settings: &Settings) -> Duration {
        Duration::from_millis(match self {
            Self::Document => settings.document_timeout_ms,
            Self::Reply(_, PendingReply::Hover(_)) => settings.hover_timeout_ms,
            _ => settings.selection_timeout_ms,
        })
    }
}

pub(super) struct PreparedTranslation {
    pub target: TranslationTarget,
    pub context: Context,
    pub is_comment: bool,
    pub task: Task,
}

pub(super) struct Translation {
    pub context: Context,
    pub target: TranslationTarget,
    pub is_comment: bool,
    pub deadline: watch::Sender<Option<Instant>>,
    pub automatic: bool,
    pub comment_hover: bool,
    pub manual: bool,
    pub document: bool,
    pub phase: Phase,
    pub started: Option<Instant>,
    pub limit: Duration,
    pub task: Option<Task>,
}

impl Translation {
    pub fn retained(&self) -> bool {
        self.range_output() || self.document
    }
    pub fn range_output(&self) -> bool {
        self.automatic || self.comment_hover || self.manual
    }
    pub fn automatic_only(&self) -> bool {
        (self.automatic || self.comment_hover) && !self.manual && !self.document
    }
    pub fn execution_deadline(&self) -> Option<Instant> {
        self.started.map(|start| start + self.limit)
    }
    /// With `provider.document`, tab operations and range operations never
    /// share work, even for the same target. Without it, one operation serves
    /// every output.
    pub fn serves(&self, output: Output, settings: &Settings) -> bool {
        !settings.provider.separates_documents() || self.document == (output == Output::Document)
    }
}

#[derive(Default)]
pub(super) struct Scheduler {
    pub pending: HashMap<RequestId, Pending>,
    pub translations: HashMap<u64, Translation>,
    /// Slots keep their dispatch category; adding another output does not start another operation.
    /// A canceled job occupies its slot until the worker acknowledges termination.
    dispatched: HashMap<u64, bool>,
}

impl Scheduler {
    /// The operation a request for `output` joins.
    pub fn ticket(
        &self,
        target: &TranslationTarget,
        output: Output,
        settings: &Settings,
    ) -> Option<u64> {
        self.translations.iter().find_map(|(&ticket, work)| {
            (&work.target == target && work.serves(output, settings)).then_some(ticket)
        })
    }

    pub fn at(&self, uri: &Uri, position: Position) -> Option<TranslationTarget> {
        self.translations
            .iter()
            .filter(|(_, w)| {
                &w.target.uri == uri
                    && (!w.document || w.range_output())
                    && w.target.range.start <= position
                    && position <= w.target.range.end
            })
            .max_by_key(|(ticket, _)| *ticket)
            .map(|(_, w)| w.target.clone())
    }

    pub fn admitted(&self) -> usize {
        self.translations.len()
            + self
                .dispatched
                .keys()
                .filter(|ticket| !self.translations.contains_key(ticket))
                .count()
    }
    pub fn automatic_count(&self) -> usize {
        self.translations
            .values()
            .filter(|w| w.automatic_only())
            .count()
    }
    pub fn available(&self, settings: &Settings, automatic: bool) -> bool {
        self.admitted() < settings.translation_limits.work.admitted
            && (!automatic || self.automatic_count() < settings.translation_limits.work.automatic)
    }

    pub fn join(
        &mut self,
        ticket: u64,
        request: TranslationRequest,
        settings: &Settings,
        now: Instant,
    ) -> RequestResult<()> {
        let work = self
            .translations
            .get_mut(&ticket)
            .expect("Joined translation exists");
        if work
            .execution_deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            return Err(failure(ErrorCode::RequestFailed, "Translation timed out"));
        }
        if matches!(
            request,
            TranslationRequest::Reply(_, PendingReply::Hover(_))
        ) && self.pending.len() >= settings.translation_limits.work.hover_waiters
        {
            return Err(failure(
                ErrorCode::RequestFailed,
                "Too many pending Hover requests",
            ));
        }
        // Repeated Hover joins never renew the operation's execution budget.
        if !matches!(
            request,
            TranslationRequest::Reply(_, PendingReply::Hover(_))
        ) {
            work.limit = work.limit.max(request.timeout(settings));
        }
        match request {
            TranslationRequest::Reply(id, PendingReply::Hover(position)) => {
                work.comment_hover |= work.is_comment;
                self.pending.insert(
                    id,
                    Pending {
                        ticket,
                        deadline: now + Duration::from_millis(settings.hover_timeout_ms),
                        position,
                    },
                );
            }
            TranslationRequest::Reply(_, PendingReply::Command | PendingReply::Retry) => {
                work.manual = true;
                if let Some(Task::Translation { intent, .. }) = work.task.as_mut() {
                    *intent = crate::providers::Intent::Explicit;
                }
            }
            TranslationRequest::Selection => work.automatic = true,
            TranslationRequest::Document => {
                work.document = true;
                if let Some(Task::Translation { intent, .. }) = work.task.as_mut() {
                    *intent = crate::providers::Intent::Explicit;
                }
            }
        }
        self.update_deadline(ticket);
        Ok(())
    }

    pub fn insert(
        &mut self,
        ticket: u64,
        prepared: PreparedTranslation,
        request: TranslationRequest,
        settings: &Settings,
        now: Instant,
    ) -> RequestResult<()> {
        let PreparedTranslation {
            target,
            context,
            is_comment,
            task,
        } = prepared;
        let (deadline, _) = watch::channel(None);
        self.translations.insert(
            ticket,
            Translation {
                context,
                target,
                is_comment,
                deadline,
                automatic: false,
                comment_hover: false,
                manual: false,
                document: false,
                phase: Phase::Queued,
                started: None,
                limit: request.timeout(settings),
                task: Some(task),
            },
        );
        if let Err(error) = self.join(ticket, request, settings, now) {
            self.translations.remove(&ticket);
            return Err(error);
        }
        Ok(())
    }

    pub fn remove(&mut self, ticket: u64) -> Option<Translation> {
        let work = self.translations.remove(&ticket)?;
        work.deadline.send_replace(None);
        Some(work)
    }

    pub fn retire_automatic(&mut self, ticket: u64) {
        if let Some(work) = self.translations.get_mut(&ticket) {
            work.automatic = false;
        }
        self.update_deadline(ticket);
    }

    pub fn update_deadline(&mut self, ticket: u64) {
        let waiter_deadline = self
            .pending
            .values()
            .filter(|p| p.ticket == ticket)
            .map(|p| p.deadline)
            .max();
        let Some(work) = self.translations.get(&ticket) else {
            return;
        };
        if !work.retained() && waiter_deadline.is_none() {
            self.remove(ticket);
            return;
        }
        let deadline = work.execution_deadline().map(|cap| {
            if work.retained() {
                cap
            } else {
                cap.min(waiter_deadline.unwrap())
            }
        });
        work.deadline.send_replace(deadline);
    }

    pub fn dispatch(
        &mut self,
        jobs: &JobSender,
        settings: &Settings,
        now: Instant,
    ) -> RequestResult<()> {
        let limits = settings.translation_limits.work;
        while self.dispatched.len() < limits.concurrent {
            let tabs = self.dispatched.values().filter(|tab| **tab).count();
            let next = self
                .translations
                .iter()
                .filter(|(_, work)| {
                    work.task.is_some() && (!work.document || tabs < limits.documents)
                })
                .map(|(&ticket, _)| ticket)
                .min();
            let Some(ticket) = next else {
                break;
            };
            let work = self.translations.get_mut(&ticket).unwrap();
            work.started = Some(now);
            let task = work.task.take().unwrap();
            let deadline = work.deadline.subscribe();
            let tab = work.document;
            self.update_deadline(ticket);
            jobs.try_send(Job {
                ticket,
                deadline,
                task,
            })
            .map_err(|_| {
                failure(
                    ErrorCode::InternalError,
                    "Translation worker is unavailable",
                )
            })?;
            self.dispatched.insert(ticket, tab);
        }
        Ok(())
    }

    pub fn terminated(&mut self, ticket: u64) {
        self.dispatched.remove(&ticket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(
        s: &mut Scheduler,
        ticket: u64,
        request: TranslationRequest,
        settings: &Settings,
        now: Instant,
    ) {
        let target = TranslationTarget {
            uri: "file:///same.md".parse().unwrap(),
            version: 1,
            session: 1,
            range: Range::new(
                Position::new(ticket as u32, 0),
                Position::new(ticket as u32, 5),
            ),
            target_language: "ja".into(),
        };
        let task = Task::Translation {
            intent: crate::providers::Intent::Explicit,
            output: Output::Range,
            generation: 0,
            text: "source".into(),
            target_language: "ja".into(),
            context: Context::Mock,
        };
        s.insert(
            ticket,
            PreparedTranslation {
                target,
                context: Context::Mock,
                is_comment: false,
                task,
            },
            request,
            settings,
            now,
        )
        .unwrap();
    }

    fn manual() -> TranslationRequest {
        TranslationRequest::Reply(0.into(), PendingReply::Command)
    }

    #[test]
    fn dispatch_starts_absolute_budgets_and_only_terminal_events_release_slots() {
        let settings = Settings::default();
        let now = Instant::now();
        let mut s = Scheduler::default();
        let (jobs, mut received) = queue::channel(2);
        for ticket in 1..=3 {
            add(&mut s, ticket, manual(), &settings, now);
        }
        s.dispatch(&jobs, &settings, now).unwrap();
        assert_eq!(received.try_recv().unwrap().ticket, 1);
        assert_eq!(received.try_recv().unwrap().ticket, 2);
        assert!(s.translations[&3].execution_deadline().is_none());
        s.remove(1);
        s.dispatch(&jobs, &settings, now + Duration::from_secs(20))
            .unwrap();
        assert!(received.try_recv().is_err());
        s.terminated(1);
        let later = now + Duration::from_secs(25);
        s.dispatch(&jobs, &settings, later).unwrap();
        assert_eq!(received.try_recv().unwrap().ticket, 3);
        assert_eq!(
            s.translations[&3].execution_deadline(),
            Some(later + Duration::from_secs(30))
        );
    }

    #[test]
    fn hover_joins_do_not_renew_and_promotion_uses_original_start() {
        let settings = Settings::default();
        let now = Instant::now();
        let mut s = Scheduler::default();
        let (jobs, mut received) = queue::channel(2);
        add(
            &mut s,
            1,
            TranslationRequest::Reply(1.into(), PendingReply::Hover(Position::new(1, 2))),
            &settings,
            now,
        );
        s.dispatch(&jobs, &settings, now).unwrap();
        received.try_recv().unwrap();
        for id in 2..10 {
            s.join(
                1,
                TranslationRequest::Reply(id.into(), PendingReply::Hover(Position::new(1, 2))),
                &settings,
                now + Duration::from_secs(10),
            )
            .unwrap();
        }
        assert_eq!(
            s.translations[&1].execution_deadline(),
            Some(now + Duration::from_secs(20))
        );
        s.join(1, manual(), &settings, now + Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            s.translations[&1].execution_deadline(),
            Some(now + Duration::from_secs(30))
        );
        s.join(
            1,
            TranslationRequest::Document,
            &settings,
            now + Duration::from_secs(15),
        )
        .unwrap();
        assert_eq!(
            s.translations[&1].execution_deadline(),
            Some(now + Duration::from_secs(180))
        );
        s.join(
            1,
            TranslationRequest::Document,
            &settings,
            now + Duration::from_secs(25),
        )
        .unwrap();
        assert_eq!(
            s.translations[&1].execution_deadline(),
            Some(now + Duration::from_secs(180))
        );
        assert!(
            s.at(&"file:///same.md".parse().unwrap(), Position::new(1, 2))
                .is_some(),
            "A shared tab must not hide its retained range from Hover"
        );
        s.pending.clear();
        s.update_deadline(1);
        assert!(s.translations.contains_key(&1));
    }

    #[test]
    fn only_tab_slot_waiters_can_be_bypassed_and_automatic_limits_are_separate() {
        let settings = Settings::default();
        let now = Instant::now();
        let mut s = Scheduler::default();
        let (jobs, mut received) = queue::channel(2);
        add(&mut s, 1, TranslationRequest::Document, &settings, now);
        add(&mut s, 2, TranslationRequest::Document, &settings, now);
        add(&mut s, 3, manual(), &settings, now);
        s.dispatch(&jobs, &settings, now).unwrap();
        assert_eq!(received.try_recv().unwrap().ticket, 1);
        assert_eq!(received.try_recv().unwrap().ticket, 3);
        s.remove(1);
        s.terminated(1);
        s.dispatch(&jobs, &settings, now).unwrap();
        assert_eq!(received.try_recv().unwrap().ticket, 2);
        for ticket in 4..=6 {
            add(
                &mut s,
                ticket,
                TranslationRequest::Selection,
                &settings,
                now,
            );
        }
        assert!(!s.available(&settings, true));
        assert!(s.available(&settings, false));
        s.join(4, manual(), &settings, now).unwrap();
        assert!(s.available(&settings, true));
    }
}

#[cfg(test)]
mod shared_outputs_tests {
    use super::*;

    #[test]
    fn a_tab_subscriber_shares_a_running_range_without_another_slot() {
        let settings = Settings::default();
        let now = Instant::now();
        let mut scheduler = Scheduler::default();
        let (jobs, mut receiver) = queue::channel(2);
        for ticket in 1..=2 {
            let target = TranslationTarget {
                uri: "file:///shared.md".parse().unwrap(),
                version: 1,
                session: 1,
                range: Range::new(Position::new(ticket, 0), Position::new(ticket, 5)),
                target_language: "ja".into(),
            };
            let task = Task::Translation {
                intent: crate::providers::Intent::Explicit,
                output: Output::Range,
                generation: 0,
                text: "source".into(),
                target_language: "ja".into(),
                context: Context::Mock,
            };
            let request = if ticket == 1 {
                TranslationRequest::Document
            } else {
                TranslationRequest::Reply(1.into(), PendingReply::Command)
            };
            scheduler
                .insert(
                    ticket as u64,
                    PreparedTranslation {
                        target,
                        context: Context::Mock,
                        is_comment: false,
                        task,
                    },
                    request,
                    &settings,
                    now,
                )
                .unwrap();
        }
        scheduler.dispatch(&jobs, &settings, now).unwrap();
        receiver.try_recv().unwrap();
        receiver.try_recv().unwrap();
        scheduler
            .join(2, TranslationRequest::Document, &settings, now)
            .unwrap();
        assert_eq!(scheduler.dispatched.len(), 2);
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            scheduler.translations[&2].execution_deadline(),
            Some(now + Duration::from_millis(settings.document_timeout_ms))
        );
        assert!(
            scheduler
                .at(&"file:///shared.md".parse().unwrap(), Position::new(2, 3))
                .is_some()
        );
    }
}

#[cfg(test)]
mod separate_outputs_tests {
    use super::*;

    fn settings(separate: bool) -> Settings {
        let mut provider = serde_json::json!({"name": "anthropic"});
        if separate {
            provider["document"] = serde_json::json!({"model": "synthetic-model"});
        }
        Settings::from_options(Some(serde_json::json!({
            "provider": provider,
            "translation_limits": {"work": {"concurrent": 3, "documents": 1}}
        })))
        .unwrap()
    }

    fn target(line: u32) -> TranslationTarget {
        TranslationTarget {
            uri: "file:///separate.md".parse().unwrap(),
            version: 1,
            session: 1,
            range: Range::new(Position::new(line, 0), Position::new(line, 5)),
            target_language: "ja".into(),
        }
    }

    fn add(
        scheduler: &mut Scheduler,
        ticket: u64,
        line: u32,
        request: TranslationRequest,
        settings: &Settings,
        now: Instant,
    ) {
        let task = Task::Translation {
            intent: crate::providers::Intent::Explicit,
            output: request.output(),
            generation: 0,
            text: "source".into(),
            target_language: "ja".into(),
            context: Context::Anthropic(crate::providers::anthropic::key::KeyId::SYNTHETIC),
        };
        scheduler
            .insert(
                ticket,
                PreparedTranslation {
                    target: target(line),
                    context: Context::Anthropic(crate::providers::anthropic::key::KeyId::SYNTHETIC),
                    is_comment: false,
                    task,
                },
                request,
                settings,
                now,
            )
            .unwrap();
    }

    fn manual() -> TranslationRequest {
        TranslationRequest::Reply(0.into(), PendingReply::Command)
    }

    #[test]
    fn tab_settings_give_one_target_a_tab_operation_and_a_range_operation() {
        let now = Instant::now();
        let separate = settings(true);
        let mut scheduler = Scheduler::default();
        let (jobs, mut received) = queue::channel(3);
        add(
            &mut scheduler,
            1,
            0,
            TranslationRequest::Document,
            &separate,
            now,
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, &separate),
            Some(1)
        );
        assert_eq!(scheduler.ticket(&target(0), Output::Range, &separate), None);
        add(&mut scheduler, 2, 0, manual(), &separate, now);
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, &separate),
            Some(1)
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, &separate),
            Some(2)
        );
        assert_eq!(scheduler.admitted(), 2);

        // Each takes an execution slot, and the tab limit still applies to tabs.
        add(
            &mut scheduler,
            3,
            1,
            TranslationRequest::Document,
            &separate,
            now,
        );
        scheduler.dispatch(&jobs, &separate, now).unwrap();
        let dispatched: Vec<_> = std::iter::from_fn(|| received.try_recv().ok())
            .map(|job| (job.ticket, matches!(job.task, Task::Translation { output, .. } if output == Output::Document)))
            .collect();
        assert_eq!(dispatched, [(1, true), (2, false)]);
        assert_eq!(scheduler.dispatched.len(), 2);
        assert!(scheduler.translations[&3].started.is_none());

        // Hover and automatic selection join the range operation only.
        scheduler
            .join(
                2,
                TranslationRequest::Reply(7.into(), PendingReply::Hover(Position::new(0, 2))),
                &separate,
                now,
            )
            .unwrap();
        scheduler
            .join(2, TranslationRequest::Selection, &separate, now)
            .unwrap();
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, &separate),
            Some(2)
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, &separate),
            Some(1)
        );
        assert_eq!(
            scheduler
                .at(&target(0).uri, Position::new(0, 2))
                .map(|found| found.range),
            Some(target(0).range)
        );
        scheduler.remove(2);
        assert!(scheduler.at(&target(0).uri, Position::new(0, 2)).is_none());

        // Canceling one leaves the other, and its slot returns only on termination.
        assert_eq!(scheduler.ticket(&target(0), Output::Range, &separate), None);
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, &separate),
            Some(1)
        );
        assert_eq!(scheduler.admitted(), 3);
        scheduler.terminated(2);
        assert_eq!(scheduler.admitted(), 2);
    }

    #[test]
    fn without_tab_settings_one_operation_serves_every_output() {
        let now = Instant::now();
        let shared = settings(false);
        for first in [TranslationRequest::Document, manual()] {
            let mut scheduler = Scheduler::default();
            add(&mut scheduler, 1, 0, first, &shared, now);
            for output in [Output::Range, Output::Document] {
                assert_eq!(scheduler.ticket(&target(0), output, &shared), Some(1));
                assert_eq!(scheduler.ticket(&target(1), output, &shared), None);
            }
        }
    }
}
