use crate::providers::Intent;

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

pub(super) struct PreparedTranslation<C> {
    pub target: TranslationTarget,
    pub context: C,
    pub is_comment: bool,
    pub task: Task<C>,
}

/// The requests that keep a translation going besides Hover waiters.
#[derive(Clone, Copy, Default)]
pub(super) struct Owners {
    /// A stable automatic selection.
    selection: bool,
    /// A comment Hover, which keeps its translation past its response.
    comment_hover: bool,
    /// A selection translation or retry run from a Code Action.
    manual: bool,
    /// A tab translation.
    tab: bool,
}

impl Owners {
    fn add(&mut self, request: &TranslationRequest, is_comment: bool) {
        match request {
            TranslationRequest::Reply(_, PendingReply::Hover(_)) => {
                self.comment_hover |= is_comment;
            }
            TranslationRequest::Reply(_, PendingReply::Command | PendingReply::Retry) => {
                self.manual = true;
            }
            TranslationRequest::Selection => self.selection = true,
            TranslationRequest::Document => self.tab = true,
        }
    }
    /// Whether the translation survives without Hover waiters.
    pub fn retained(self) -> bool {
        self.range_output() || self.tab
    }
    /// Whether the result is read in Hover.
    pub fn range_output(self) -> bool {
        self.selection || self.comment_hover || self.manual
    }
    /// Whether only automatic requests own it, so it counts toward the
    /// automatic limit.
    pub fn automatic_only(self) -> bool {
        (self.selection || self.comment_hover) && !self.manual && !self.tab
    }
    /// Whether a stable automatic selection is its only owner.
    pub fn selection_only(self) -> bool {
        self.selection && !self.comment_hover && !self.manual && !self.tab
    }
    #[cfg(test)]
    pub fn selection(self) -> bool {
        self.selection
    }
    pub fn manual(self) -> bool {
        self.manual
    }
    pub fn tab(self) -> bool {
        self.tab
    }
}

pub(super) struct Translation<C> {
    pub context: C,
    pub target: TranslationTarget,
    pub is_comment: bool,
    pub deadline: watch::Sender<Option<Instant>>,
    pub owners: Owners,
    pub phase: Phase,
    pub started: Option<Instant>,
    pub limit: Duration,
    pub task: Option<Task<C>>,
}

impl<C> Translation<C> {
    pub fn execution_deadline(&self) -> Option<Instant> {
        self.started.map(|start| start + self.limit)
    }
    pub fn serves(&self, output: Output, sharing: Sharing) -> bool {
        match sharing {
            Sharing::Both => true,
            Sharing::UntilSent => {
                output == Output::Range || self.owners.tab() || self.task.is_some()
            }
            Sharing::Never => self.owners.tab() == (output == Output::Document),
        }
    }
}

/// Which operation of a target serves a request for the other output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sharing {
    /// One operation serves both outputs.
    Both,
    /// A range request asks for less output than a tab request, so a tab
    /// shares a range operation only until it is sent.
    UntilSent,
    /// With `provider.document`, tab operations and range operations never
    /// share work, even for the same target.
    Never,
}

pub(super) struct Scheduler<C> {
    /// Hover requests waiting for a translation.
    pending: HashMap<RequestId, Pending>,
    translations: HashMap<u64, Translation<C>>,
    /// Slots keep their dispatch category; adding another output does not start another operation.
    /// A canceled job occupies its slot until the worker acknowledges termination.
    dispatched: HashMap<u64, bool>,
}

impl<C> Default for Scheduler<C> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            translations: HashMap::new(),
            dispatched: HashMap::new(),
        }
    }
}

impl<C> Scheduler<C> {
    pub fn translations(&self) -> &HashMap<u64, Translation<C>> {
        &self.translations
    }

    pub fn pending(&self) -> &HashMap<RequestId, Pending> {
        &self.pending
    }

    pub fn has_waiters(&self, ticket: u64) -> bool {
        self.pending
            .values()
            .any(|pending| pending.ticket == ticket)
    }

    /// Gives a queued translation the admission generation of a later request.
    pub fn set_generation(&mut self, ticket: u64, fresh: u64) {
        if let Some(Task::Translation { generation, .. }) = self
            .translations
            .get_mut(&ticket)
            .and_then(|work| work.task.as_mut())
        {
            *generation = fresh;
        }
    }

    #[cfg(test)]
    pub fn set_waiter_deadline(&mut self, id: &RequestId, deadline: Instant) {
        self.pending.get_mut(id).unwrap().deadline = deadline;
    }

    pub fn mark_running(&mut self, ticket: u64) {
        if let Some(work) = self.translations.get_mut(&ticket) {
            work.phase = Phase::Running;
        }
    }

    /// The earliest deadline of a waiter or of a running translation.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .values()
            .map(|pending| pending.deadline)
            .chain(
                self.translations
                    .values()
                    .filter_map(Translation::execution_deadline),
            )
            .min()
    }

    /// Removes the waiters of `ticket`, which the caller answers.
    pub fn take_waiters(&mut self, ticket: u64) -> Vec<(RequestId, Pending)> {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.ticket == ticket)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .map(|id| {
                let pending = self.pending.remove(&id).unwrap();
                (id, pending)
            })
            .collect()
    }

    /// Removes one waiter. Its translation stops when nothing else keeps it.
    pub fn remove_waiter(&mut self, id: &RequestId) -> bool {
        let Some(pending) = self.pending.remove(id) else {
            return false;
        };
        self.update_deadline(pending.ticket);
        true
    }

    /// Removes the waiters whose deadline passed, which the caller answers.
    pub fn take_expired_waiters(&mut self, now: Instant) -> Vec<RequestId> {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.remove_waiter(id);
        }
        ids
    }

    /// Stops and removes the translations whose execution deadline passed.
    pub fn take_expired(&mut self, now: Instant) -> Vec<(u64, Translation<C>)> {
        let tickets: Vec<_> = self
            .translations
            .iter()
            .filter(|(_, work)| {
                work.execution_deadline()
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|(&ticket, _)| ticket)
            .collect();
        tickets
            .into_iter()
            .map(|ticket| (ticket, self.remove(ticket).unwrap()))
            .collect()
    }

    /// Stops every translation and removes every waiter, which the caller
    /// answers.
    pub fn stop_all(&mut self) -> Vec<RequestId> {
        for (_, work) in self.translations.drain() {
            work.deadline.send_replace(None);
        }
        self.pending.drain().map(|(id, _)| id).collect()
    }

    /// Stops the translations of `uri` and removes their waiters, which the
    /// caller answers.
    pub fn stop_document(&mut self, uri: &Uri) -> Vec<RequestId> {
        let tickets: Vec<_> = self
            .translations
            .iter()
            .filter(|(_, work)| &work.target.uri == uri)
            .map(|(&ticket, _)| ticket)
            .collect();
        let mut waiters = Vec::new();
        for ticket in tickets {
            waiters.extend(self.take_waiters(ticket).into_iter().map(|(id, _)| id));
            self.remove(ticket);
        }
        waiters
    }

    /// The operation a request for `output` joins.
    pub fn ticket(
        &self,
        target: &TranslationTarget,
        output: Output,
        sharing: Sharing,
    ) -> Option<u64> {
        self.translations.iter().find_map(|(&ticket, work)| {
            (&work.target == target && work.serves(output, sharing)).then_some(ticket)
        })
    }

    pub fn at(&self, uri: &Uri, position: Position) -> Option<TranslationTarget> {
        self.translations
            .iter()
            .filter(|(_, w)| {
                &w.target.uri == uri
                    && (!w.owners.tab() || w.owners.range_output())
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
            .filter(|w| w.owners.automatic_only())
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
        work.owners.add(&request, work.is_comment);
        if let Some(Task::Translation { intent, output, .. }) = work.task.as_mut() {
            if request.explicit() {
                *intent = Intent::Explicit;
            }
            // A tab that joins before sending asks for its own output.
            if request.output() == Output::Document {
                *output = Output::Document;
            }
        }
        if let TranslationRequest::Reply(id, PendingReply::Hover(position)) = request {
            self.pending.insert(
                id,
                Pending {
                    ticket,
                    deadline: now + Duration::from_millis(settings.hover_timeout_ms),
                    position,
                },
            );
        }
        self.update_deadline(ticket);
        Ok(())
    }

    pub fn insert(
        &mut self,
        ticket: u64,
        prepared: PreparedTranslation<C>,
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
                owners: Owners::default(),
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

    pub fn remove(&mut self, ticket: u64) -> Option<Translation<C>> {
        let work = self.translations.remove(&ticket)?;
        work.deadline.send_replace(None);
        Some(work)
    }

    pub fn retire_automatic(&mut self, ticket: u64) {
        if let Some(work) = self.translations.get_mut(&ticket) {
            work.owners.selection = false;
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
        if !work.owners.retained() && waiter_deadline.is_none() {
            self.remove(ticket);
            return;
        }
        let deadline = work.execution_deadline().map(|cap| {
            if work.owners.retained() {
                cap
            } else {
                cap.min(waiter_deadline.unwrap())
            }
        });
        work.deadline.send_replace(deadline);
    }

    pub fn dispatch(
        &mut self,
        jobs: &JobSender<C>,
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
                    work.task.is_some() && (!work.owners.tab() || tabs < limits.documents)
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
            let tab = work.owners.tab();
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
    use crate::providers::{Intent, mock::MockContext};

    fn add(
        s: &mut Scheduler<MockContext>,
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
            intent: Intent::Explicit,
            output: Output::Range,
            generation: 0,
            text: "source".into(),
            target_language: "ja".into(),
            context: MockContext,
        };
        s.insert(
            ticket,
            PreparedTranslation {
                target,
                context: MockContext,
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
    use crate::providers::{Intent, mock::MockContext};

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
                intent: Intent::Explicit,
                output: Output::Range,
                generation: 0,
                text: "source".into(),
                target_language: "ja".into(),
                context: MockContext,
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
                        context: MockContext,
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
    use std::iter;

    use super::*;
    use crate::providers::{Intent, anthropic::key::KeyId};

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
        scheduler: &mut Scheduler<KeyId>,
        ticket: u64,
        line: u32,
        request: TranslationRequest,
        settings: &Settings,
        now: Instant,
    ) {
        let task = Task::Translation {
            intent: Intent::Explicit,
            output: request.output(),
            generation: 0,
            text: "source".into(),
            target_language: "ja".into(),
            context: KeyId::SYNTHETIC,
        };
        scheduler
            .insert(
                ticket,
                PreparedTranslation {
                    target: target(line),
                    context: KeyId::SYNTHETIC,
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
            scheduler.ticket(&target(0), Output::Document, Sharing::Never),
            Some(1)
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, Sharing::Never),
            None
        );
        add(&mut scheduler, 2, 0, manual(), &separate, now);
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, Sharing::Never),
            Some(1)
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, Sharing::Never),
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
        let dispatched: Vec<_> = iter::from_fn(|| received.try_recv().ok())
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
            scheduler.ticket(&target(0), Output::Range, Sharing::Never),
            Some(2)
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, Sharing::Never),
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
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, Sharing::Never),
            None
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, Sharing::Never),
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
                assert_eq!(scheduler.ticket(&target(0), output, Sharing::Both), Some(1));
                assert_eq!(scheduler.ticket(&target(1), output, Sharing::Both), None);
            }
        }
    }

    #[test]
    fn a_tab_shares_a_bounded_range_operation_only_until_it_is_sent() {
        let now = Instant::now();
        let shared = settings(false);
        let mut scheduler = Scheduler::default();
        let (jobs, mut received) = queue::channel(3);
        add(&mut scheduler, 1, 0, manual(), &shared, now);
        add(&mut scheduler, 2, 1, manual(), &shared, now);
        scheduler.dispatch(&jobs, &shared, now).unwrap();
        add(&mut scheduler, 3, 2, manual(), &shared, now);
        add(
            &mut scheduler,
            4,
            3,
            TranslationRequest::Document,
            &shared,
            now,
        );

        // A sent range operation serves ranges only; a tab operation serves both.
        assert_eq!(
            scheduler.ticket(&target(0), Output::Document, Sharing::UntilSent),
            None
        );
        assert_eq!(
            scheduler.ticket(&target(0), Output::Range, Sharing::UntilSent),
            Some(1)
        );
        for output in [Output::Range, Output::Document] {
            assert_eq!(
                scheduler.ticket(&target(3), output, Sharing::UntilSent),
                Some(4)
            );
        }

        // A tab joins a range operation before it is sent and asks for its own output.
        assert_eq!(
            scheduler.ticket(&target(2), Output::Document, Sharing::UntilSent),
            Some(3)
        );
        scheduler
            .join(3, TranslationRequest::Document, &shared, now)
            .unwrap();
        scheduler.terminated(1);
        scheduler.terminated(2);
        scheduler.dispatch(&jobs, &shared, now).unwrap();
        let outputs: Vec<_> = iter::from_fn(|| received.try_recv().ok())
            .map(|job| match job.task {
                Task::Translation { output, .. } => (job.ticket, output),
                _ => panic!("Expected a translation"),
            })
            .collect();
        assert_eq!(
            outputs,
            [
                (1, Output::Range),
                (2, Output::Range),
                (3, Output::Document)
            ]
        );
    }
}
