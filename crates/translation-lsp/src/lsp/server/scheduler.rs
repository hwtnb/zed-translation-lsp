use super::*;

pub(super) struct Pending {
    pub(super) ticket: u64,
    pub(super) deadline: Instant,
    pub(super) reply: PendingReply,
}

pub(super) enum PendingReply {
    Hover(Position),
    Command,
}

pub(super) enum TranslationRequest {
    Reply(RequestId, PendingReply),
    Selection,
    Document,
}

pub(super) struct Translation {
    pub(super) context: Context,
    pub(super) phase: Phase,
    pub(super) target: TranslationTarget,
    pub(super) is_comment: bool,
    pub(super) deadline: watch::Sender<Option<Instant>>,
    pub(super) failure_notified: bool,
    pub(super) background_deadline: Option<Instant>,
    pub(super) document_deadline: Option<Instant>,
}

impl Translation {
    pub(super) fn detached_deadlines(&self) -> impl Iterator<Item = Instant> {
        [self.background_deadline, self.document_deadline]
            .into_iter()
            .flatten()
    }
}

#[derive(Default)]
pub(super) struct Scheduler {
    pub pending: HashMap<RequestId, Pending>,
    pub translations: HashMap<u64, Translation>,
}

impl Scheduler {
    pub fn owner_count(&self) -> usize {
        self.pending.len()
            + self
                .translations
                .values()
                .map(|work| work.detached_deadlines().count())
                .sum::<usize>()
    }

    pub fn ticket(&self, target: &TranslationTarget) -> Option<u64> {
        self.translations
            .iter()
            .find_map(|(&ticket, work)| (&work.target == target).then_some(ticket))
    }

    pub fn join(
        &mut self,
        ticket: u64,
        request: TranslationRequest,
        deadline: Instant,
    ) -> RequestResult<()> {
        let work = self
            .translations
            .get_mut(&ticket)
            .expect("A joined ticket belongs to the scheduler");
        if work
            .deadline
            .borrow()
            .is_none_or(|deadline| deadline <= Instant::now())
        {
            return Err(failure(ErrorCode::RequestFailed, "Translation timed out"));
        }
        match request {
            TranslationRequest::Reply(id, reply) => {
                self.pending.insert(
                    id,
                    Pending {
                        ticket,
                        deadline,
                        reply,
                    },
                );
            }
            TranslationRequest::Selection => work.background_deadline = Some(deadline),
            TranslationRequest::Document => {
                work.document_deadline.get_or_insert(deadline);
            }
        }
        self.update_deadline(ticket);
        Ok(())
    }

    pub(super) fn update_deadline(&mut self, ticket: u64) {
        let deadline = self
            .pending
            .values()
            .filter(|pending| pending.ticket == ticket)
            .map(|pending| pending.deadline)
            .chain(
                self.translations
                    .get(&ticket)
                    .into_iter()
                    .flat_map(Translation::detached_deadlines),
            )
            .max();
        if let Some(work) = self.translations.get(&ticket) {
            work.deadline.send_replace(deadline);
        }
        if deadline.is_none() {
            self.translations.remove(&ticket);
        }
    }
}
