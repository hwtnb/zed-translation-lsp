use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use lsp_server::{Message, Request, RequestId, Response};
use lsp_types::ProgressToken;
use serde_json::{Value, json};

use super::ui::UiCapabilities;

const REPORT_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Queued,
    Running,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Work {
    pub queued: usize,
    pub running: usize,
    pub waiting: bool,
}

impl Work {
    fn busy(self) -> bool {
        self.queued + self.running > 0 || self.waiting
    }

    fn message(self) -> String {
        let mut message = if self.running > 0 {
            format!("Translating: {}, queued: {}", self.running, self.queued)
        } else {
            format!("Queued: {}", self.queued)
        };
        if self.waiting {
            message.push_str("; selection not yet accepted");
        }
        message
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Problem {
    Failed(String),
    TimedOut,
}

enum Progress {
    Idle,
    Creating {
        id: RequestId,
        token: ProgressToken,
        generation: u64,
    },
    Registered {
        token: ProgressToken,
        generation: u64,
    },
    Visible {
        token: ProgressToken,
        message: String,
        sent_at: Instant,
    },
}

pub(super) struct Activity {
    sender: Sender<Message>,
    capabilities: UiCapabilities,
    progress: Progress,
    generation: u64,
    work: Work,
    problem: Option<Problem>,
    last_status: Option<Value>,
}

impl Activity {
    pub fn new(sender: Sender<Message>, capabilities: UiCapabilities) -> Self {
        Self {
            sender,
            capabilities,
            progress: Progress::Idle,
            generation: 0,
            work: Work::default(),
            problem: None,
            last_status: None,
        }
    }

    pub fn clear_problem(&mut self) {
        self.problem = None;
    }

    pub fn problem(&mut self, status: Problem) {
        self.problem = Some(status);
    }

    pub fn response(&mut self, response: &Response) -> bool {
        let Progress::Creating {
            id,
            token,
            generation,
        } = &self.progress
        else {
            return false;
        };
        if id != &response.id {
            return false;
        }
        self.progress = if response.response_result.is_ok() {
            Progress::Registered {
                token: token.clone(),
                generation: *generation,
            }
        } else {
            // A rejected create must never receive progress for that token.
            self.capabilities.work_done_progress = false;
            Progress::Idle
        };
        true
    }

    pub fn matches_cancel(&self, token: &ProgressToken) -> bool {
        self.work.busy()
            && match &self.progress {
                Progress::Creating {
                    token: current,
                    generation,
                    ..
                }
                | Progress::Registered {
                    token: current,
                    generation,
                } => current == token && *generation == self.generation,
                Progress::Visible { token: current, .. } => current == token,
                Progress::Idle => false,
            }
    }

    pub fn deadline(&self) -> Option<Instant> {
        match &self.progress {
            Progress::Visible {
                message, sent_at, ..
            } if self.work.busy() && message != &self.work.message() => {
                Some(*sent_at + REPORT_INTERVAL)
            }
            _ => None,
        }
    }

    pub fn publish(&mut self, work: Work, now: Instant) {
        if work.busy() && !self.work.busy() {
            self.generation += 1;
        }
        self.work = work;
        let progress = std::mem::replace(&mut self.progress, Progress::Idle);
        self.progress = match progress {
            Progress::Registered { token, generation } => {
                if work.busy() && generation == self.generation {
                    let message = work.message();
                    self.begin(&token, Some(&message));
                    Progress::Visible {
                        token,
                        message,
                        sent_at: now,
                    }
                } else {
                    // Close a late successful create without reviving an old result.
                    self.begin(&token, None);
                    self.end(&token);
                    Progress::Idle
                }
            }
            Progress::Visible {
                token,
                mut message,
                mut sent_at,
            } => {
                if work.busy() {
                    let current = work.message();
                    if message != current && now >= sent_at + REPORT_INTERVAL {
                        self.notify(
                            "$/progress",
                            json!({"token":token,"value":{"kind":"report","cancellable":true,"message":current}}),
                        );
                        message = current;
                        sent_at = now;
                    }
                    Progress::Visible {
                        token,
                        message,
                        sent_at,
                    }
                } else {
                    self.end(&token);
                    Progress::Idle
                }
            }
            other => other,
        };
        if work.busy()
            && self.capabilities.work_done_progress
            && matches!(self.progress, Progress::Idle)
        {
            let identity = format!("translation-progress-{}", uuid::Uuid::new_v4());
            let id = RequestId::from(identity.clone());
            let token = ProgressToken::String(identity);
            let _ = self.sender.send(
                Request {
                    id: id.clone(),
                    method: "window/workDoneProgress/create".into(),
                    params: json!({"token":token}),
                }
                .into(),
            );
            self.progress = Progress::Creating {
                id,
                token,
                generation: self.generation,
            };
        }
        self.publish_status();
    }

    pub fn shutdown(&mut self) {
        match std::mem::replace(&mut self.progress, Progress::Idle) {
            Progress::Visible { token, .. } => self.end(&token),
            Progress::Registered { token, .. } => {
                self.begin(&token, None);
                self.end(&token);
            }
            Progress::Creating { id, .. } => {
                self.notify("$/cancelRequest", json!({"id":id}));
            }
            Progress::Idle => (),
        }
        self.work = Work::default();
        self.clear_problem();
        self.publish_status();
    }

    fn begin(&self, token: &ProgressToken, message: Option<&str>) {
        let mut value = json!({"kind":"begin","title":"Translation LSP","cancellable":true});
        if let Some(message) = message {
            value["message"] = json!(message);
        }
        self.notify("$/progress", json!({"token":token,"value":value}));
    }

    fn end(&self, token: &ProgressToken) {
        self.notify("$/progress", json!({"token":token,"value":{"kind":"end"}}));
    }

    fn publish_status(&mut self) {
        if !self.capabilities.server_status {
            return;
        }
        let params = match &self.problem {
            Some(Problem::Failed(message)) => Some(json!({
                "health":"warning", "quiescent":!self.work.busy(),
                "message":message
            })),
            Some(Problem::TimedOut) => Some(json!({
                "health":"warning", "quiescent":!self.work.busy(),
                "message":"Translation timed out. Try a shorter selection or adjust the timeout in Zed settings."
            })),
            None => None,
        };
        if self.last_status != params {
            self.notify(
                "experimental/serverStatus",
                params
                    .clone()
                    .unwrap_or_else(|| json!({"health":"ok","quiescent":true})),
            );
            self.last_status = params;
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self
            .sender
            .send(lsp_server::Notification::new(method.into(), params).into());
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::{Receiver, unbounded};

    use super::*;

    fn fixture(progress: bool, status: bool) -> (Activity, Receiver<Message>) {
        let (sender, receiver) = unbounded();
        let capabilities = UiCapabilities {
            work_done_progress: progress,
            server_status: status,
            ..Default::default()
        };
        (Activity::new(sender, capabilities), receiver)
    }

    fn create(receiver: &Receiver<Message>) -> Request {
        let Message::Request(request) = receiver.try_recv().unwrap() else {
            panic!("Expected a progress registration request");
        };
        assert_eq!(request.method, "window/workDoneProgress/create");
        request
    }

    fn notification(receiver: &Receiver<Message>, method: &str) -> Value {
        let Message::Notification(notification) = receiver.try_recv().unwrap() else {
            panic!("Expected {method}");
        };
        assert_eq!(notification.method, method);
        notification.params
    }

    fn acknowledge(activity: &mut Activity, request: &Request) {
        assert!(activity.response(&Response::new_ok(request.id.clone(), Value::Null)));
    }

    #[test]
    fn registered_progress_reports_latest_counts_after_throttle_and_ends_without_success_status() {
        let (mut activity, output) = fixture(true, true);
        let now = Instant::now();
        activity.publish(
            Work {
                queued: 2,
                ..Default::default()
            },
            now,
        );
        let request = create(&output);
        assert!(output.is_empty(), "No begin before registration");
        acknowledge(&mut activity, &request);
        activity.publish(
            Work {
                running: 1,
                queued: 1,
                ..Default::default()
            },
            now,
        );
        let begin = notification(&output, "$/progress");
        assert_eq!(begin["token"], request.params["token"]);
        assert_eq!(
            begin["value"],
            json!({
                "kind":"begin", "title":"Translation LSP", "cancellable":true,
                "message":"Translating: 1, queued: 1"
            })
        );
        activity.publish(
            Work {
                running: 1,
                ..Default::default()
            },
            now + Duration::from_millis(20),
        );
        assert!(output.is_empty());
        assert_eq!(activity.deadline(), Some(now + REPORT_INTERVAL));
        activity.publish(
            Work {
                running: 1,
                queued: 2,
                ..Default::default()
            },
            now + REPORT_INTERVAL,
        );
        let report = notification(&output, "$/progress");
        assert_eq!(
            report["value"],
            json!({
                "kind":"report", "cancellable":true, "message":"Translating: 1, queued: 2"
            })
        );
        activity.publish(
            Work {
                running: 1,
                ..Default::default()
            },
            now + REPORT_INTERVAL * 2,
        );
        assert_eq!(
            notification(&output, "$/progress")["value"]["message"],
            "Translating: 1, queued: 0"
        );
        activity.publish(Work::default(), now + REPORT_INTERVAL * 2);
        assert_eq!(
            notification(&output, "$/progress"),
            json!({
                "token":request.params["token"], "value":{"kind":"end"}
            })
        );
        assert!(output.is_empty());
        assert!(activity.deadline().is_none());
    }

    #[test]
    fn delayed_create_closes_retired_token_before_registering_current_work() {
        let (mut activity, output) = fixture(true, true);
        let now = Instant::now();
        let queued = Work {
            queued: 1,
            ..Default::default()
        };
        activity.publish(queued, now);
        let old = create(&output);
        activity.publish(Work::default(), now);
        activity.publish(queued, now);
        assert!(output.is_empty(), "At most one pending create");
        let old_token: ProgressToken = serde_json::from_value(old.params["token"].clone()).unwrap();
        assert!(!activity.matches_cancel(&old_token));
        acknowledge(&mut activity, &old);
        activity.publish(queued, now);
        let begin = notification(&output, "$/progress");
        assert_eq!(begin["token"], old.params["token"]);
        assert!(begin["value"].get("message").is_none());
        assert_eq!(notification(&output, "$/progress")["value"]["kind"], "end");
        let current = create(&output);
        assert_ne!(current.params["token"], old.params["token"]);
        assert!(!activity.response(&Response::new_ok(old.id, Value::Null)));
        acknowledge(&mut activity, &current);
        activity.publish(queued, now);
        assert_eq!(
            notification(&output, "$/progress")["value"]["message"],
            "Queued: 1"
        );
        assert!(output.is_empty());
    }

    #[test]
    fn rejected_create_falls_back_and_shutdown_cancels_unacknowledged_registration() {
        let (mut activity, output) = fixture(true, false);
        let now = Instant::now();
        let work = Work {
            running: 1,
            ..Default::default()
        };
        activity.publish(work, now);
        let request = create(&output);
        assert!(activity.response(&Response::new_err(request.id, -32601, "unsupported".into())));
        activity.publish(work, now);
        activity.publish(Work::default(), now);
        activity.publish(work, now);
        assert!(output.is_empty());
        let (mut activity, output) = fixture(true, false);
        activity.publish(work, now);
        let request = create(&output);
        activity.shutdown();
        assert_eq!(
            notification(&output, "$/cancelRequest"),
            json!({"id":request.id})
        );
        activity.shutdown();
        assert!(output.is_empty());
    }

    #[test]
    fn capabilities_gate_independent_progress_and_problem_channels_without_automatic_info() {
        for (progress, status) in [(false, false), (false, true), (true, false), (true, true)] {
            let (mut activity, output) = fixture(progress, status);
            let now = Instant::now();
            activity.publish(Work::default(), now);
            assert!(output.is_empty(), "Healthy idle is silent");
            activity.problem(Problem::Failed("Translation response was rejected.".into()));
            activity.publish(Work::default(), now);
            if status {
                assert_eq!(
                    notification(&output, "experimental/serverStatus"),
                    json!({
                        "health":"warning", "quiescent":true, "message":"Translation response was rejected."
                    })
                );
            }
            activity.publish(Work::default(), now);
            assert!(
                output.is_empty(),
                "Repeated problem does not flood notifications"
            );
            activity.clear_problem();
            activity.publish(
                Work {
                    waiting: true,
                    ..Default::default()
                },
                now,
            );
            if progress {
                let request = create(&output);
                acknowledge(&mut activity, &request);
            }
            if status {
                assert_eq!(
                    notification(&output, "experimental/serverStatus"),
                    json!({
                        "health":"ok", "quiescent":true
                    })
                );
            }
            activity.publish(
                Work {
                    waiting: true,
                    ..Default::default()
                },
                now,
            );
            if progress {
                assert_eq!(
                    notification(&output, "$/progress")["value"]["message"],
                    "Queued: 0; selection not yet accepted"
                );
            }
            activity.shutdown();
            if progress {
                assert_eq!(notification(&output, "$/progress")["value"]["kind"], "end");
            }
            assert!(output.is_empty());
        }
    }
}
