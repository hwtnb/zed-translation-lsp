mod administration;
mod lifecycle;
mod scheduler;
mod selection;
mod translation;
mod worker;

use tokio::sync::mpsc as queue;
type JobSender = queue::Sender<Job>;
type JobReceiver = queue::Receiver<Job>;
use scheduler::{PendingReply, Scheduler, Translation, TranslationRequest};
use selection::Selection;
#[cfg(test)]
use worker::run_job;
use worker::{Finished, Job, Outcome, Task, WorkerEvent, worker};

use std::{
    collections::HashMap,
    error::Error,
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Sender, after, bounded, never, select};
use futures_util::{StreamExt, stream};
use lsp_server::{Connection, ErrorCode, Message, Request, RequestId, Response, ResponseError};
use lsp_types::{
    CancelParams, CodeAction, CodeActionKind, CodeActionOptions, CodeActionParams,
    CodeActionProviderCapability, Command, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, ExecuteCommandOptions, ExecuteCommandParams, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, NumberOrString, Position,
    PositionEncodingKind, Range, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, Uri, WorkDoneProgressCancelParams,
    notification::{
        DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Exit, Notification,
    },
    request::{CodeActionRequest, ExecuteCommand, HoverRequest, Request as LspRequest, Shutdown},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::watch;

use crate::{
    config::{ProviderSettings, Settings},
    providers::{
        AuthUiState, Context, Interaction, Translated, TranslationProvider,
        chatgpt::ChatGptProvider,
        error::{Error as ProviderError, ErrorKind},
        mock::{MockOptions, MockProvider},
    },
};

use super::{
    activity::{Activity, Phase, Problem, Work},
    document::Document,
    preview::{
        Preview,
        cache::{Cache, overlaps},
        files::PreviewFiles,
    },
    ui::{Ui, UiCapabilities},
};

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
type RequestResult<T> = Result<T, ResponseError>;

const COMMAND: &str = "translation-lsp.preview";
const RETRY_COMMAND: &str = "translation-lsp.retry";
const DOCUMENT_COMMAND: &str = "translation-lsp.open-translation";
const SETTINGS_COMMAND: &str = "translation-lsp.settings";
const ACTION_KIND: &str = "translation.preview";
const TRANSLATION_READY: &str = "Translation ready. Hover over the selected text to view it.";
const WORKER_EVENT_CHANNEL_CAPACITY: usize = 8;
const CANCEL_COMMAND: &str = "translation-lsp.cancel-range";
const CANCEL_ALL_COMMAND: &str = "translation-lsp.cancel-all";

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranslationTarget {
    uri: Uri,
    version: i32,
    session: u64,
    range: Range,
    target_language: String,
}

struct Administration {
    command: String,
    ticket: u64,
    stop: watch::Sender<Option<Instant>>,
    deadline: Instant,
}

struct Server<P: TranslationProvider> {
    provider: P,
    settings: Settings,
    ui: Ui,
    activity: Activity,
    preview_files: PreviewFiles,
    cache: Cache,
    problem_target: Option<(TranslationTarget, Option<Context>)>,
    administrations: HashMap<RequestId, Administration>,
    documents: HashMap<Uri, Document>,
    scheduler: Scheduler,
    selection: Option<Selection>,
    previous_selection: Option<TranslationTarget>,
    adjusting_ticket: Option<u64>,
    next_session: u64,
    next_ticket: u64,
    jobs: Option<JobSender>,
    worker: Option<thread::JoinHandle<()>>,
    admin_jobs: Option<JobSender>,
}

pub fn run_stdio() -> ServerResult<()> {
    let options = MockOptions::from_args()?;
    let (connection, io_threads) = Connection::stdio();
    let result = serve(&connection, options);
    drop(connection);
    // Flush initialize errors too; the reader stops at the client's exit or EOF.
    let io_result = io_threads.join();
    result?;
    io_result?;
    Ok(())
}

fn serve(connection: &Connection, options: MockOptions) -> ServerResult<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let settings =
        Settings::from_options(params.initialization_options).inspect_err(|message| {
            let _ = respond(
                connection,
                id.clone(),
                Err(failure(ErrorCode::InvalidParams, &message.to_string())),
            );
        })?;
    let chatgpt = matches!(settings.provider, ProviderSettings::Chatgpt { .. });
    let ui = Ui::new(
        connection.sender.clone(),
        UiCapabilities::from(&params.capabilities),
    );
    let initialized = if chatgpt {
        capabilities::<ChatGptProvider>()
    } else {
        capabilities::<MockProvider>()
    };
    connection.initialize_finish(id, serde_json::to_value(initialized)?)?;

    match settings.provider.clone() {
        ProviderSettings::Mock {} => {
            let mut settings = settings;
            options.apply_timeouts(&mut settings);
            serve_initialized(connection, settings, ui, MockProvider::new(options))
        }
        ProviderSettings::Chatgpt {
            model,
            reasoning_effort,
        } => {
            let provider = ChatGptProvider::new(
                model,
                reasoning_effort,
                settings.source_language.clone(),
                settings.translation_limits.output_kib,
            )?;
            serve_initialized(connection, settings, ui, provider)
        }
    }
}

pub(crate) fn capabilities<P: TranslationProvider>() -> InitializeResult {
    let provider = P::capabilities();
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: Some(PositionEncodingKind::UTF16),
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::INCREMENTAL),
                    ..Default::default()
                },
            )),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::new(ACTION_KIND)]),
                resolve_provider: Some(false),
                ..Default::default()
            })),
            execute_command_provider: Some(ExecuteCommandOptions {
                commands: [
                    COMMAND.to_owned(),
                    RETRY_COMMAND.to_owned(),
                    DOCUMENT_COMMAND.to_owned(),
                    CANCEL_COMMAND.to_owned(),
                    CANCEL_ALL_COMMAND.to_owned(),
                ]
                .into_iter()
                .chain(
                    provider
                        .commands
                        .iter()
                        .map(|command| command.command.to_owned()),
                )
                .chain(provider.account_menu.map(|_| SETTINGS_COMMAND.to_owned()))
                .collect(),
                ..Default::default()
            }),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: provider.server_name.into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    }
}

pub(crate) fn serve_initialized<P: TranslationProvider>(
    connection: &Connection,
    settings: Settings,
    ui: Ui,
    provider: P,
) -> ServerResult<()> {
    let (finished_sender, finished) = bounded(WORKER_EVENT_CHANNEL_CAPACITY);
    let mut server = Server::configured(finished_sender, provider, settings, ui)?;
    let result = (|| {
        server.publish_activity();
        loop {
            let deadline = server
                .scheduler
                .pending
                .values()
                .map(|pending| pending.deadline)
                .chain(server.administrations.values().map(|work| work.deadline))
                .chain(
                    server
                        .scheduler
                        .translations
                        .values()
                        .filter_map(Translation::execution_deadline),
                )
                .chain(server.activity.deadline())
                .chain(server.preview_files.deadline())
                .chain(
                    server
                        .selection
                        .as_ref()
                        .filter(|selection| server.can_start_selection(selection))
                        .and_then(|selection| selection.start),
                )
                .min()
                .map(|deadline| after(deadline.saturating_duration_since(Instant::now())))
                .unwrap_or_else(never);
            select! {
                recv(connection.receiver) -> message => match message? {
                    Message::Request(request) => {
                        if request.method == Shutdown::METHOD {
                            server.cancel_all(connection)?;
                            server.activity.shutdown();
                            server.preview_files.clear();
                            connection.handle_shutdown(&request)?;
                            break;
                        }
                        server.request(connection, request)?;
                    }
                    Message::Notification(notification) => {
                        if notification.method == Exit::METHOD {
                            return Err("exit received before shutdown".into());
                        }
                        server.notification(connection, notification)?;
                    }
                    Message::Response(response) => {
                        if !server.activity.response(&response)
                            && !server.preview_files.response(&server.ui, &response)
                        {
                            server.ui.response(response);
                        }
                    },
                },
                recv(finished) -> event => match event? {
                    WorkerEvent::Started(ticket) => { if let Some(work) = server.scheduler.translations.get_mut(&ticket) { work.phase = Phase::Running; } },
                    WorkerEvent::Finished(completion) => server.finish(connection, completion)?,
                },
                recv(deadline) -> _ => server.expire(connection)?,
            }
            server.dispatch().map_err(|e| e.message)?;
            server.publish_activity();
        }
        Ok(())
    })();
    // Release a worker blocked on completion before canceling and joining it.
    drop(finished);
    drop(server);
    result
}

impl Server<MockProvider> {
    #[cfg(test)]
    fn new(options: MockOptions, finished: Sender<WorkerEvent>) -> ServerResult<Self> {
        let mut settings = Settings::default();
        options.apply_timeouts(&mut settings);
        let (sender, _) = bounded(1);
        Self::configured(
            finished,
            MockProvider::new(options),
            settings,
            Ui::new(sender, UiCapabilities::default()),
        )
    }
}

impl<P: TranslationProvider> Server<P> {
    #[cfg(test)]
    fn new_with_provider(finished: Sender<WorkerEvent>, provider: P) -> ServerResult<Self> {
        let (sender, _) = bounded(1);
        Self::configured(
            finished,
            provider,
            Settings::default(),
            Ui::new(sender, UiCapabilities::default()),
        )
    }

    fn configured(
        finished: Sender<WorkerEvent>,
        provider: P,
        settings: Settings,
        ui: Ui,
    ) -> ServerResult<Self> {
        let (sender, jobs) = queue::channel(settings.translation_limits.work.concurrent);
        let (admin_sender, admin_jobs) = queue::channel(1);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let worker_provider = provider.clone();
        let concurrency = settings.translation_limits.work.concurrent;
        let worker = thread::Builder::new()
            .name("translation-worker".into())
            .spawn(move || {
                runtime.block_on(worker(
                    jobs,
                    admin_jobs,
                    finished,
                    worker_provider,
                    concurrency,
                ))
            })?;
        let activity = Activity::new(ui.sender.clone(), ui.capabilities);
        Ok(Self {
            provider,
            cache: Cache::new(settings.translation_limits),
            settings,
            ui,
            activity,
            preview_files: PreviewFiles::default(),
            problem_target: None,
            administrations: HashMap::new(),
            documents: HashMap::new(),
            scheduler: Scheduler::default(),
            selection: None,
            previous_selection: None,
            adjusting_ticket: None,
            next_session: 0,
            next_ticket: 0,
            jobs: Some(sender),
            worker: Some(worker),
            admin_jobs: Some(admin_sender),
        })
    }

    fn clear_problem(&mut self) {
        self.problem_target = None;
        self.activity.clear_problem();
    }

    fn target_is_current(&self, target: &TranslationTarget) -> bool {
        self.documents.get(&target.uri).is_some_and(|document| {
            document.version == target.version
                && document.session == target.session
                && document.text().is_some()
        })
    }

    fn publish_activity(&mut self) {
        if !self.ui.capabilities.work_done_progress && !self.ui.capabilities.server_status {
            return;
        }
        if self
            .problem_target
            .as_ref()
            .is_some_and(|(target, previous)| {
                !self.target_is_current(target)
                    || self.provider.context().map_or_else(
                        |_| {
                            P::capabilities().account_menu.is_some()
                                && matches!(
                                    self.provider.auth_ui_state(),
                                    AuthUiState::SignedOut | AuthUiState::PlanDisabled
                                )
                        },
                        |current| previous.as_ref() != Some(&current),
                    )
            })
        {
            self.clear_problem();
        }
        let mut activity = Work::default();
        for work in self.scheduler.translations.values() {
            if work.phase == Phase::Running {
                activity.running += 1;
            } else {
                activity.queued += 1;
            }
        }
        activity.waiting = self.selection.as_ref().is_some_and(|selection| {
            selection.start.is_some()
                && self.target_is_current(&selection.target)
                && !self.scheduler.ticket(&selection.target).is_some()
        });
        self.activity.publish(activity, Instant::now());
    }

    fn record_problem(&mut self, work: &Translation, problem: Problem) {
        if !self.ui.capabilities.server_status {
            return;
        }
        let context = self.provider.context().ok();
        if self.target_is_current(&work.target)
            && context
                .as_ref()
                .is_none_or(|current| work.context == *current)
        {
            self.problem_target = Some((work.target.clone(), context));
            self.activity.problem(problem);
        }
    }

    fn clear_previews(&mut self) {
        self.clear_problem();
        self.cache.clear();
    }

    fn check_cache(&mut self) -> RequestResult<Context> {
        if !self.administrations.is_empty() {
            return Err(failure(
                ErrorCode::RequestFailed,
                "An account operation is in progress",
            ));
        }
        let context = match self.provider.context() {
            Ok(context) => context,
            Err(error) => {
                if error.kind() != ErrorKind::Unavailable {
                    self.clear_previews();
                }
                return Err(provider_failure(error));
            }
        };
        self.cache.retain_context(&context);
        Ok(context)
    }

    fn request(&mut self, connection: &Connection, request: Request) -> ServerResult<()> {
        self.expire(connection)?;
        let command = request.method == ExecuteCommand::METHOD;
        let id = request.id;
        if self.scheduler.pending.contains_key(&id) || self.administrations.contains_key(&id) {
            // A duplicate live ID is invalid. Retire it once, allowing later reuse
            // without a completed old ticket answering a new request.
            if command {
                self.command_failure(&failure(
                    ErrorCode::InvalidRequest,
                    "Duplicate live request ID",
                ))?;
            }
            self.cancel(connection, &id, ErrorCode::InvalidRequest)
                .map_err(|error| error.message)?;
            return Ok(());
        }
        let result = match request.method.as_str() {
            CodeActionRequest::METHOD => parse(request.params)
                .and_then(|params| self.code_actions(params))
                .and_then(to_value)
                .map(Some),
            HoverRequest::METHOD => {
                parse(request.params).and_then(|params| self.hover(id.clone(), params))
            }
            ExecuteCommand::METHOD => parse(request.params)
                .and_then(|params| self.execute_command(connection, id.clone(), params)),
            _ => Err(failure(ErrorCode::MethodNotFound, "Unsupported request")),
        };
        match result {
            Ok(None) => Ok(()),
            Ok(Some(value)) => respond(connection, id, Ok(value)),
            Err(error) => {
                if command {
                    self.command_failure(&error)?;
                }
                respond(connection, id, Err(error))
            }
        }
    }

    fn command_failure(&self, error: &ResponseError) -> ServerResult<()> {
        if error.code != ErrorCode::RequestCanceled as i32
            && error.code != ErrorCode::ContentModified as i32
        {
            self.ui.error(&error.message)?;
        }
        Ok(())
    }

    fn code_actions(&mut self, params: CodeActionParams) -> RequestResult<Vec<CodeAction>> {
        if params.context.only.as_ref().is_some_and(|kinds| {
            !kinds.iter().any(|kind| {
                let kind = kind.as_str();
                kind.is_empty()
                    || ACTION_KIND == kind
                    || ACTION_KIND.starts_with(&format!("{kind}."))
            })
        }) {
            return Ok(Vec::new());
        }
        let state = self.provider.auth_ui_state();
        if P::capabilities().account_menu.is_some()
            && matches!(state, AuthUiState::SignedOut | AuthUiState::PlanDisabled)
        {
            self.clear_problem();
        }
        self.observe_selection(&params, state);
        if !self.administrations.is_empty() {
            return self.cancel_actions(&params);
        }
        let mut actions = self.cancel_actions(&params)?;
        actions.extend(if let Some(title) = P::capabilities().account_menu {
            vec![CodeAction {
                title: title.into(),
                kind: Some(CodeActionKind::new(ACTION_KIND)),
                command: Some(Command {
                    title: title.into(),
                    command: SETTINGS_COMMAND.into(),
                    arguments: None,
                }),
                ..Default::default()
            }]
        } else {
            Vec::new()
        });
        if P::capabilities().account_menu.is_some() && state != AuthUiState::Ready {
            return Ok(actions);
        }
        let retry_range = if params.range.start == params.range.end && self.check_cache().is_ok() {
            self.cache
                .failed_range(&params.text_document.uri, params.range.start)
        } else {
            None
        };
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(actions);
        };
        let mut translations = Vec::new();
        let mut add_action = |range: Range, command: &str, title: String| -> RequestResult<()> {
            let target = TranslationTarget {
                uri: params.text_document.uri.clone(),
                version: document.version,
                session: document.session,
                range,
                target_language: self.settings.target_language.clone(),
            };
            translations.push(CodeAction {
                title: title.clone(),
                kind: Some(CodeActionKind::new(ACTION_KIND)),
                command: Some(Command {
                    title,
                    command: command.into(),
                    arguments: Some(vec![to_value(target)?]),
                }),
                ..Default::default()
            });
            Ok(())
        };
        if let Some(range) = retry_range
            && document.selection(range).is_ok_and(|text| {
                text.len() <= self.settings.translation_limits.input_kib.hover.bytes()
            })
            && !self.scheduler.translations.values().any(|work| {
                work.target.uri == params.text_document.uri && work.target.range == range
            })
        {
            add_action(
                range,
                RETRY_COMMAND,
                format!(
                    "Retry translation of lines {}-{}",
                    range.start.line + 1,
                    range.end.line + 1
                ),
            )?;
        }
        if document.selection(params.range).is_ok_and(|text| {
            text.len() <= self.settings.translation_limits.input_kib.hover.bytes()
        }) {
            add_action(
                params.range,
                COMMAND,
                format!("Translate selection to {}", self.settings.target_language),
            )?;
        }
        if document.supports_document_preview() && self.ui.capabilities.show_document {
            let (range, subject) = if params.range.start == params.range.end {
                (document.full_range(), "document")
            } else {
                (Some(params.range), "selection")
            };
            if let Some(range) = range
                && document.selection(range).is_ok()
            {
                add_action(
                    range,
                    DOCUMENT_COMMAND,
                    format!(
                        "Translate {subject} to {} and open in tab",
                        self.settings.target_language
                    ),
                )?;
            }
        }
        translations.extend(actions);
        Ok(translations)
    }

    fn hover(&mut self, id: RequestId, params: HoverParams) -> RequestResult<Option<Value>> {
        // Invalidate stale previews before reading them. Only translation targets
        // propagate authorization failures; unrelated Hover requests return null.
        let authorization = self.check_cache();
        let position = params.text_document_position_params;
        let Some(document) = self.documents.get_mut(&position.text_document.uri) else {
            return Ok(Some(Value::Null));
        };
        let Some(text) = document.text() else {
            return Ok(Some(Value::Null));
        };
        let Ok(offset) = document.offset(position.position) else {
            return Ok(Some(Value::Null));
        };
        let end_is_clear = text[offset..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace);
        let range = document.comment_range(&position.text_document.uri, offset);
        let eligible = |preview: &Preview| {
            !preview.is_comment
                || preview.range.end != position.position
                || (end_is_clear && !range.is_some_and(|range| range.start == position.position))
        };
        if let Some(hover) = self.cache.hover(
            &position.text_document.uri,
            position.position,
            false,
            eligible,
        ) {
            authorization?;
            return to_value(hover).map(Some);
        }
        if let Some(target) = self
            .scheduler
            .at(&position.text_document.uri, position.position)
        {
            return self.queue(id, target, PendingReply::Hover(position.position));
        }
        if let Some(hover) =
            self.cache
                .hover(&position.text_document.uri, position.position, true, |_| {
                    true
                })
        {
            authorization?;
            return to_value(hover).map(Some);
        }
        let Some(range) = range else {
            return Ok(Some(Value::Null));
        };
        let target = TranslationTarget {
            uri: position.text_document.uri,
            version: document.version,
            session: document.session,
            range,
            target_language: self.settings.target_language.clone(),
        };
        // Keep this request open. Its one response arrives from finish/expire/cancel.
        self.queue(id, target, PendingReply::Hover(position.position))
    }

    fn notification(
        &mut self,
        connection: &Connection,
        notification: lsp_server::Notification,
    ) -> ServerResult<()> {
        let result: RequestResult<()> = (|| {
            match notification.method.as_str() {
                DidOpenTextDocument::METHOD => {
                    let params: DidOpenTextDocumentParams = parse(notification.params)?;
                    let document = params.text_document;
                    self.invalidate(connection, &document.uri)?;
                    self.next_session += 1;
                    self.documents.insert(
                        document.uri,
                        Document::new(
                            document.version,
                            self.next_session,
                            document.language_id,
                            document.text,
                        ),
                    );
                }
                DidChangeTextDocument::METHOD => {
                    let params: DidChangeTextDocumentParams = parse(notification.params)?;
                    self.invalidate(connection, &params.text_document.uri)?;
                    if let Some(document) = self.documents.get_mut(&params.text_document.uri) {
                        document
                            .change(params.text_document.version, params.content_changes)
                            .map_err(|message| failure(ErrorCode::InvalidParams, message))?;
                    }
                }
                DidCloseTextDocument::METHOD => {
                    let params: DidCloseTextDocumentParams = parse(notification.params)?;
                    self.invalidate(connection, &params.text_document.uri)?;
                    self.documents.remove(&params.text_document.uri);
                }
                "window/workDoneProgress/cancel" => {
                    let params: WorkDoneProgressCancelParams = parse(notification.params)?;
                    if self.activity.matches_cancel(&params.token) {
                        self.cancel_activity(connection)?;
                    }
                }
                "$/cancelRequest" => {
                    let params: CancelParams = parse(notification.params)?;
                    let id = match params.id {
                        NumberOrString::Number(id) => RequestId::from(id),
                        NumberOrString::String(id) => RequestId::from(id),
                    };
                    self.cancel(connection, &id, ErrorCode::RequestCanceled)?;
                }
                _ => (),
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Notifications have no response. Do not log their text or document contents.
            eprintln!("translation preview: {}", error.message);
        }
        Ok(())
    }
}

impl<P: TranslationProvider> Drop for Server<P> {
    fn drop(&mut self) {
        self.activity.shutdown();
        for work in self.scheduler.translations.values() {
            let _ = work.deadline.send(None);
        }
        for work in self.administrations.values() {
            let _ = work.stop.send(None);
        }
        self.jobs.take();
        self.admin_jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn provider_failure(error: ProviderError) -> ResponseError {
    failure(
        if error.kind() == ErrorKind::Modified {
            ErrorCode::ContentModified
        } else if error.kind() == ErrorKind::Cancelled {
            ErrorCode::RequestCanceled
        } else {
            ErrorCode::RequestFailed
        },
        &error.to_string(),
    )
}

fn parse<T: DeserializeOwned>(value: Value) -> RequestResult<T> {
    serde_json::from_value(value)
        .map_err(|_| failure(ErrorCode::InvalidParams, "Invalid request parameters"))
}

fn to_value<T: Serialize>(value: T) -> RequestResult<Value> {
    serde_json::to_value(value)
        .map_err(|_| failure(ErrorCode::InternalError, "Unable to serialize response"))
}

fn failure(code: ErrorCode, message: &str) -> ResponseError {
    ResponseError {
        code: code as i32,
        message: message.into(),
        data: None,
    }
}

fn respond(
    connection: &Connection,
    id: RequestId,
    result: RequestResult<Value>,
) -> ServerResult<()> {
    let response = match result {
        Ok(value) => Response::new_ok(id, value),
        Err(error) => Response {
            id,
            response_result: Err(error),
        },
    };
    connection.sender.send(response.into())?;
    Ok(())
}

#[cfg(test)]
mod chatgpt_tests;
#[cfg(test)]
mod tests;
