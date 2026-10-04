use std::{
    collections::HashMap,
    error::Error,
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Sender, after, bounded, never, select};
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
use tokio::sync::{mpsc, watch};

use crate::{
    config::{ProviderSettings, Settings},
    providers::{
        AuthUiState, Context, Translated, TranslationProvider,
        chatgpt::ChatGptProvider,
        error::{Error as ProviderError, ErrorKind},
        mock::MockProvider,
    },
};

use super::{
    activity::{Activity, Phase, Problem, Work},
    document::{Document, utf16_offset},
    preview::Preview,
    ui::{Ui, UiCapabilities},
};

type ServerResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
type RequestResult<T> = Result<T, ResponseError>;

const COMMAND: &str = "zed-code-translation.preview";
const SETTINGS_COMMAND: &str = "zed-code-translation.settings";
const ACTION_KIND: &str = "translation.preview";
const TRANSLATION_READY: &str = "Translation ready. Hover over the selected text to view it.";
const ADMIN_COMMANDS: [(&str, &str); 4] = [
    (
        "zed-code-translation.connect",
        "Continue with ChatGPT / Choose account",
    ),
    ("zed-code-translation.enable", "Enable ChatGPT plan use"),
    ("zed-code-translation.sign-out", "Sign out of ChatGPT"),
    ("zed-code-translation.usage", "ChatGPT Usage"),
];
const MAX_JOBS: usize = 4;
const MAX_PENDING: usize = 16;
const MAX_SELECTION_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(crate) struct MockOptions {
    delay: Duration,
    timeout_ms: Option<u64>,
    fail: bool,
}

impl MockOptions {
    fn from_args() -> ServerResult<Self> {
        let mut options = Self::default();
        let mut args = std::env::args().skip(1);
        while let Some(argument) = args.next() {
            match argument.as_str() {
                "--stdio" => (),
                "--mock-delay-ms" => {
                    let milliseconds: u64 = args.next().ok_or("Missing mock delay")?.parse()?;
                    if milliseconds > 30_000 {
                        return Err("Mock delay must be between 0 and 30000 milliseconds".into());
                    }
                    options.delay = Duration::from_millis(milliseconds);
                }
                "--mock-fail" => options.fail = true,
                "--mock-timeout-ms" => {
                    let milliseconds: u64 = args.next().ok_or("Missing mock timeout")?.parse()?;
                    if !(1..=30_000).contains(&milliseconds) {
                        return Err("Mock timeout must be between 1 and 30000 milliseconds".into());
                    }
                    // This fixture-only override caps both kinds of mock request.
                    options.timeout_ms = Some(milliseconds);
                }
                _ => return Err(format!("Unknown argument: {argument}").into()),
            }
        }
        Ok(options)
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranslationTarget {
    uri: Uri,
    version: i32,
    session: u64,
    range: Range,
    target_language: String,
}

struct Selection {
    target: TranslationTarget,
    context: Option<Context>,
    start: Option<Instant>,
}

struct Job {
    ticket: u64,
    deadline: watch::Receiver<Option<Instant>>,
    task: Task,
}

enum Task {
    AccountMenu {
        ui: Ui,
    },
    Translation {
        text: String,
        target_language: String,
        context: Context,
    },
    Administration {
        command: String,
        ui: Ui,
    },
}

struct Finished {
    ticket: u64,
    result: Result<Outcome, ProviderError>,
}

enum WorkerEvent {
    Started(u64),
    Finished(Finished),
}

enum Outcome {
    AccountChoice(Option<String>),
    Translation(Translated),
    Administration(String),
}

struct Administration {
    command: String,
    ticket: u64,
    stop: watch::Sender<Option<Instant>>,
    deadline: Instant,
}

struct Pending {
    ticket: u64,
    deadline: Instant,
    reply: PendingReply,
}

enum PendingReply {
    Hover(Position),
    Command,
}

struct Translation {
    context: Context,
    phase: Phase,
    target: TranslationTarget,
    is_comment: bool,
    deadline: watch::Sender<Option<Instant>>,
    failure_notified: bool,
    background_deadline: Option<Instant>,
}

struct Server<P: TranslationProvider> {
    provider: P,
    settings: Settings,
    ui: Ui,
    activity: Activity,
    problem_target: Option<(TranslationTarget, Option<Context>)>,
    administrations: HashMap<RequestId, Administration>,
    documents: HashMap<Uri, Document>,
    pending: HashMap<RequestId, Pending>,
    translations: HashMap<u64, Translation>,
    selection: Option<Selection>,
    next_session: u64,
    next_ticket: u64,
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<thread::JoinHandle<()>>,
    admin_jobs: Option<mpsc::Sender<Job>>,
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
                Err(failure(ErrorCode::InvalidParams, message)),
            );
        })?;
    let chatgpt = matches!(settings.provider, ProviderSettings::Chatgpt { .. });
    let ui = Ui::new(
        connection.sender.clone(),
        UiCapabilities::from(&params.capabilities),
    );
    let initialized = capabilities(chatgpt);
    connection.initialize_finish(id, serde_json::to_value(initialized)?)?;

    match settings.provider.clone() {
        ProviderSettings::Mock {} => {
            serve_initialized(connection, options, settings, ui, MockProvider)
        }
        ProviderSettings::Chatgpt {
            model,
            reasoning_effort,
        } => {
            let provider =
                ChatGptProvider::new(model, reasoning_effort, settings.source_language.clone())?;
            serve_initialized(connection, MockOptions::default(), settings, ui, provider)
        }
    }
}

pub(crate) fn capabilities(chatgpt: bool) -> InitializeResult {
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
                commands: std::iter::once(COMMAND.to_owned())
                    .chain(if chatgpt {
                        ADMIN_COMMANDS
                            .iter()
                            .map(|(command, _)| command.to_string())
                            .chain(std::iter::once(SETTINGS_COMMAND.to_owned()))
                            .collect()
                    } else {
                        Vec::new()
                    })
                    .collect(),
                ..Default::default()
            }),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: if chatgpt {
                "Translation Preview (ChatGPT)"
            } else {
                "Translation Preview (Mock)"
            }
            .into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    }
}

pub(crate) fn serve_initialized<P: TranslationProvider>(
    connection: &Connection,
    options: MockOptions,
    settings: Settings,
    ui: Ui,
    provider: P,
) -> ServerResult<()> {
    let (finished_sender, finished) = bounded(MAX_JOBS);
    let mut server = Server::configured(options, finished_sender, provider, settings, ui)?;
    let result = (|| {
        server.publish_activity();
        loop {
            let deadline = server
                .pending
                .values()
                .map(|pending| pending.deadline)
                .chain(server.administrations.values().map(|work| work.deadline))
                .chain(
                    server
                        .translations
                        .values()
                        .filter_map(|work| work.background_deadline),
                )
                .chain(server.activity.deadline())
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
                        if !server.activity.response(&response) {
                            server.ui.response(response);
                        }
                    },
                },
                recv(finished) -> event => match event? {
                    WorkerEvent::Started(ticket) => {
                        if let Some(work) = server.translations.get_mut(&ticket) {
                            work.phase = Phase::Running;
                        }
                    }
                    WorkerEvent::Finished(completion) => server.finish(connection, completion)?,
                },
                recv(deadline) -> _ => server.expire(connection)?,
            }
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
        Self::new_with_provider(options, finished, MockProvider)
    }
}

impl<P: TranslationProvider> Server<P> {
    #[cfg(test)]
    fn new_with_provider(
        options: MockOptions,
        finished: Sender<WorkerEvent>,
        provider: P,
    ) -> ServerResult<Self> {
        let (sender, _) = bounded(1);
        Self::configured(
            options,
            finished,
            provider,
            Settings::default(),
            Ui::new(sender, UiCapabilities::default()),
        )
    }

    fn configured(
        options: MockOptions,
        finished: Sender<WorkerEvent>,
        provider: P,
        mut settings: Settings,
        ui: Ui,
    ) -> ServerResult<Self> {
        let (sender, jobs) = mpsc::channel::<Job>(MAX_JOBS - 1);
        let (admin_sender, admin_jobs) = mpsc::channel::<Job>(1);
        if !provider.is_chatgpt()
            && let Some(milliseconds) = options.timeout_ms
        {
            settings.hover_timeout_ms = milliseconds;
            settings.selection_timeout_ms = milliseconds;
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let worker_provider = provider.clone();
        let worker = thread::Builder::new()
            .name("translation-worker".into())
            .spawn(move || {
                runtime.block_on(worker(jobs, admin_jobs, finished, options, worker_provider))
            })?;
        let activity = Activity::new(ui.sender.clone(), ui.capabilities);
        Ok(Self {
            provider,
            settings,
            ui,
            activity,
            problem_target: None,
            administrations: HashMap::new(),
            documents: HashMap::new(),
            pending: HashMap::new(),
            translations: HashMap::new(),
            selection: None,
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
                && document.text.is_some()
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
                            self.provider.is_chatgpt()
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
        for work in self.translations.values() {
            match work.phase {
                Phase::Queued => activity.queued += 1,
                Phase::Running => activity.running += 1,
            }
        }
        activity.waiting = self.selection.as_ref().is_some_and(|selection| {
            selection.start.is_some()
                && self.target_is_current(&selection.target)
                && !self
                    .translations
                    .values()
                    .any(|work| work.target == selection.target)
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
                .is_none_or(|current| same_account(&work.context, current))
        {
            self.problem_target = Some((work.target.clone(), context));
            self.activity.problem(problem);
        }
    }

    fn clear_previews(&mut self) {
        self.clear_problem();
        for document in self.documents.values_mut() {
            document.preview = None;
        }
    }

    fn check_cache(&mut self) -> RequestResult<Context> {
        if !self.administrations.is_empty() {
            return Err(failure(
                ErrorCode::RequestFailed,
                "A ChatGPT operation is in progress",
            ));
        }
        let context = match self.provider.context() {
            Ok(context) => context,
            Err(error) => {
                self.clear_previews();
                return Err(provider_failure(error));
            }
        };
        for document in self.documents.values_mut() {
            if document
                .preview
                .as_ref()
                .is_some_and(|preview| preview.context != context)
            {
                document.preview = None;
            }
        }
        Ok(context)
    }

    fn request(&mut self, connection: &Connection, request: Request) -> ServerResult<()> {
        self.expire(connection)?;
        let command = request.method == ExecuteCommand::METHOD;
        let id = request.id;
        if self.pending.contains_key(&id) || self.administrations.contains_key(&id) {
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
            Ok(Some(value)) => {
                if command {
                    self.ui.inform(TRANSLATION_READY)?;
                }
                respond(connection, id, Ok(value))
            }
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
        let state = self.provider.auth_ui_state();
        if self.provider.is_chatgpt()
            && matches!(state, AuthUiState::SignedOut | AuthUiState::PlanDisabled)
        {
            self.clear_problem();
        }
        self.observe_selection(&params, state);
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
        if !self.administrations.is_empty() {
            return Ok(Vec::new());
        }
        let actions = if self.provider.is_chatgpt() {
            vec![CodeAction {
                title: "ChatGPT account…".into(),
                kind: Some(CodeActionKind::new(ACTION_KIND)),
                command: Some(Command {
                    title: "ChatGPT account…".into(),
                    command: SETTINGS_COMMAND.into(),
                    arguments: None,
                }),
                ..Default::default()
            }]
        } else {
            Vec::new()
        };
        if self.provider.is_chatgpt() && state != AuthUiState::Ready {
            return Ok(actions);
        }
        let Some(document) = self.documents.get(&params.text_document.uri) else {
            return Ok(actions);
        };
        let Ok(original) = document.selection(params.range) else {
            return Ok(actions);
        };
        if original.len() > MAX_SELECTION_BYTES {
            return Ok(actions);
        }
        let target = TranslationTarget {
            uri: params.text_document.uri,
            version: document.version,
            session: document.session,
            range: params.range,
            target_language: self.settings.target_language.clone(),
        };
        let title = format!("Translate selection to {}", self.settings.target_language);
        let mut actions = actions;
        actions.insert(
            0,
            CodeAction {
                title: title.clone(),
                kind: Some(CodeActionKind::new(ACTION_KIND)),
                command: Some(Command {
                    title,
                    command: COMMAND.into(),
                    arguments: Some(vec![to_value(target)?]),
                }),
                ..Default::default()
            },
        );
        Ok(actions)
    }

    fn observe_selection(&mut self, params: &CodeActionParams, state: AuthUiState) {
        let target = self
            .documents
            .get_mut(&params.text_document.uri)
            .and_then(|document| {
                if !self.settings.markdown_auto_translate
                    || !self.provider.is_chatgpt()
                    || state != AuthUiState::Ready
                    || !document.language_id.eq_ignore_ascii_case("markdown")
                {
                    return None;
                }
                let source = document.selection(params.range).ok()?;
                if source.len() > MAX_SELECTION_BYTES {
                    return None;
                }
                if !document
                    .markdown_selection_is_translatable(params.range)
                    .ok()?
                {
                    return None;
                }
                Some(TranslationTarget {
                    uri: params.text_document.uri.clone(),
                    version: document.version,
                    session: document.session,
                    range: params.range,
                    target_language: self.settings.target_language.clone(),
                })
            });
        let Some(target) = target else {
            self.retire_selection();
            return;
        };
        let context = self.provider.context().ok();
        if self.selection.as_ref().is_some_and(|selection| {
            selection.target == target && (context.is_none() || selection.context == context)
        }) {
            return;
        }
        self.retire_selection();
        let start = context
            .is_some()
            .then(|| Instant::now() + Duration::from_millis(self.settings.selection_debounce_ms));
        self.selection = Some(Selection {
            target,
            context,
            start,
        });
    }

    fn can_start_selection(&self, selection: &Selection) -> bool {
        self.administrations.is_empty()
            && self.pending.len()
                + self
                    .translations
                    .values()
                    .filter(|work| work.background_deadline.is_some())
                    .count()
                < MAX_PENDING
            && (self
                .translations
                .values()
                .any(|work| work.target == selection.target)
                || (self.translations.len() < MAX_JOBS
                    && !self
                        .translations
                        .values()
                        .any(|work| work.target.uri == selection.target.uri)))
    }

    fn retire_selection(&mut self) {
        if self.selection.is_some() {
            self.clear_problem();
        }
        let Some(selection) = self.selection.take() else {
            return;
        };
        let ticket = self
            .translations
            .iter()
            .find(|(_, work)| work.target == selection.target)
            .map(|(&ticket, _)| ticket);
        if let Some(ticket) = ticket {
            self.translations
                .get_mut(&ticket)
                .unwrap()
                .background_deadline = None;
            self.update_deadline(ticket);
        }
    }

    fn hover(&mut self, id: RequestId, params: HoverParams) -> RequestResult<Option<Value>> {
        let _ = self.check_cache();
        let position = params.text_document_position_params;
        let Some(document) = self.documents.get_mut(&position.text_document.uri) else {
            return Ok(Some(Value::Null));
        };
        let Some(text) = document.text.as_deref() else {
            return Ok(Some(Value::Null));
        };
        let Ok(offset) = utf16_offset(text, position.position) else {
            return Ok(Some(Value::Null));
        };
        let end_is_clear = text[offset..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace);
        let cached_is_comment = document
            .preview
            .as_ref()
            .is_some_and(|preview| preview.is_comment);
        let cached = document
            .preview
            .as_ref()
            .and_then(|preview| preview.hover(position.position));
        if let Some(hover) = &cached
            && (!cached_is_comment
                || !hover
                    .range
                    .is_some_and(|range| range.end == position.position))
        {
            return to_value(hover).map(Some);
        }
        if document.language_id.eq_ignore_ascii_case("markdown")
            && let Some(selection) = self.selection.as_ref().filter(|selection| {
                selection.target.uri == position.text_document.uri
                    && selection.target.version == document.version
                    && selection.target.session == document.session
                    && position.position >= selection.target.range.start
                    && position.position <= selection.target.range.end
                    && self
                        .translations
                        .values()
                        .any(|work| work.target == selection.target)
            })
        {
            let target = selection.target.clone();
            return self.queue(id, target, PendingReply::Hover(position.position));
        }
        // A parsed comment's end must not consume adjacent source. A native
        // comment starting here wins; whitespace/EOF still permits the caret.
        let range = document.comment_range(&position.text_document.uri, offset);
        if let Some(hover) = cached
            && end_is_clear
            && !range.is_some_and(|range| range.start == position.position)
        {
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

    fn execute_command(
        &mut self,
        connection: &Connection,
        id: RequestId,
        params: ExecuteCommandParams,
    ) -> RequestResult<Option<Value>> {
        if self.provider.is_chatgpt()
            && (params.command == SETTINGS_COMMAND
                || ADMIN_COMMANDS
                    .iter()
                    .any(|(command, _)| *command == params.command))
        {
            if !params.arguments.is_empty() {
                return Err(failure(
                    ErrorCode::InvalidParams,
                    "ChatGPT account commands take no arguments",
                ));
            }
            if !self.administrations.is_empty() {
                return Err(failure(
                    ErrorCode::RequestFailed,
                    "A ChatGPT operation is in progress",
                ));
            }
            if changes_account(&params.command) {
                self.cancel_all(connection).map_err(|_| {
                    failure(ErrorCode::InternalError, "Could not cancel pending work")
                })?;
                self.clear_previews();
            }
            self.next_ticket += 1;
            let ticket = self.next_ticket;
            let deadline = Instant::now() + Duration::from_secs(180);
            let (stop, receiver) = watch::channel(Some(deadline));
            self.admin_jobs
                .as_ref()
                .unwrap()
                .try_send(Job {
                    ticket,
                    deadline: receiver,
                    task: if params.command == SETTINGS_COMMAND {
                        Task::AccountMenu {
                            ui: self.ui.clone(),
                        }
                    } else {
                        Task::Administration {
                            command: params.command.clone(),
                            ui: self.ui.clone(),
                        }
                    },
                })
                .map_err(|_| {
                    failure(
                        ErrorCode::RequestFailed,
                        "Translation worker is busy; retry the account command",
                    )
                })?;
            self.administrations.insert(
                id,
                Administration {
                    command: params.command,
                    ticket,
                    stop,
                    deadline,
                },
            );
            return Ok(None);
        }
        if params.command != COMMAND {
            return Err(failure(ErrorCode::InvalidParams, "Unknown preview command"));
        }
        if params.arguments.len() != 1 {
            return Err(failure(
                ErrorCode::InvalidParams,
                "Expected one selection argument",
            ));
        }
        let target: TranslationTarget = parse(params.arguments.into_iter().next().unwrap())?;
        if target.target_language != self.settings.target_language {
            return Err(failure(
                ErrorCode::InvalidParams,
                "The target language must match initialization_options",
            ));
        }
        self.queue(id, target, PendingReply::Command)
    }

    fn queue(
        &mut self,
        id: RequestId,
        target: TranslationTarget,
        reply: PendingReply,
    ) -> RequestResult<Option<Value>> {
        self.queue_translation(target, Some((id, reply)))
    }

    fn queue_translation(
        &mut self,
        target: TranslationTarget,
        client: Option<(RequestId, PendingReply)>,
    ) -> RequestResult<Option<Value>> {
        // No client reply means an independent background owner.
        let timeout = match &client {
            Some((_, PendingReply::Hover(_))) => self.settings.hover_timeout_ms,
            _ => self.settings.selection_timeout_ms,
        };
        let deadline = Instant::now() + Duration::from_millis(timeout);
        let context = self.check_cache();
        let document = self
            .documents
            .get_mut(&target.uri)
            .ok_or_else(|| failure(ErrorCode::ContentModified, "Document was closed"))?;
        if document.version != target.version || document.session != target.session {
            return Err(failure(
                ErrorCode::ContentModified,
                "Translation target belongs to an older document",
            ));
        }
        let original = document
            .selection(target.range)
            .map_err(|message| failure(ErrorCode::InvalidParams, message))?;
        if original.len() > MAX_SELECTION_BYTES {
            return Err(failure(
                ErrorCode::InvalidParams,
                "Selection exceeds 16 KiB",
            ));
        }
        if let Some(preview) = document
            .preview
            .as_ref()
            .filter(|preview| preview.range == target.range)
        {
            return match client {
                Some((_, PendingReply::Hover(position))) => {
                    to_value(preview.hover(position)).map(Some)
                }
                _ => Ok(Some(Value::Null)),
            };
        }
        if self.pending.len()
            + self
                .translations
                .values()
                .filter(|work| work.background_deadline.is_some())
                .count()
            >= MAX_PENDING
        {
            return Err(failure(
                ErrorCode::RequestFailed,
                "Too many pending translation requests",
            ));
        }
        if let Some((&ticket, work)) = self
            .translations
            .iter()
            .find(|(_, work)| work.target == target)
        {
            if work
                .deadline
                .borrow()
                .is_none_or(|deadline| deadline <= Instant::now())
            {
                return Err(failure(ErrorCode::RequestFailed, "Translation timed out"));
            }
            if let Some((id, reply)) = client {
                self.pending.insert(
                    id,
                    Pending {
                        ticket,
                        deadline,
                        reply,
                    },
                );
            } else {
                self.translations
                    .get_mut(&ticket)
                    .unwrap()
                    .background_deadline = Some(deadline);
            }
            self.update_deadline(ticket);
            return Ok(None);
        }
        if self.translations.len() >= MAX_JOBS
            || self
                .translations
                .values()
                .any(|work| work.target.uri == target.uri)
        {
            return Err(failure(
                ErrorCode::RequestFailed,
                "Translation is busy; retry after completion",
            ));
        }
        let context = context?;
        let input = document
            .translation_input(&target.uri, target.range)
            .map_err(|message| failure(ErrorCode::InvalidParams, message))?;
        let (deadline_sender, deadline_receiver) = watch::channel(Some(deadline));
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let job = Job {
            ticket,
            deadline: deadline_receiver,
            task: Task::Translation {
                text: input.text,
                target_language: target.target_language.clone(),
                context: context.clone(),
            },
        };
        self.jobs.as_ref().unwrap().try_send(job).map_err(|_| {
            failure(
                ErrorCode::RequestFailed,
                "Translation queue is full; retry later",
            )
        })?;
        document.preview = None;
        self.activity.clear_problem();
        self.problem_target = None;
        self.translations.insert(
            ticket,
            Translation {
                context,
                phase: Phase::Queued,
                target,
                is_comment: input.is_comment,
                deadline: deadline_sender,
                failure_notified: false,
                background_deadline: client.is_none().then_some(deadline),
            },
        );
        if let Some((id, reply)) = client {
            self.pending.insert(
                id,
                Pending {
                    ticket,
                    deadline,
                    reply,
                },
            );
        }
        Ok(None)
    }

    fn finish(&mut self, connection: &Connection, completion: Finished) -> ServerResult<()> {
        self.expire(connection)?;
        if let Some(id) = self
            .administrations
            .iter()
            .find(|(_, work)| work.ticket == completion.ticket)
            .map(|(id, _)| id.clone())
        {
            let operation = self.administrations.remove(&id).unwrap();
            if changes_account(&operation.command) {
                self.clear_previews();
            }
            if operation.command == SETTINGS_COMMAND
                && operation.deadline > Instant::now()
                && let Ok(Outcome::AccountChoice(choice)) = &completion.result
            {
                let result = match choice {
                    None => Ok(Some(Value::Null)),
                    Some(command) => self.execute_command(
                        connection,
                        id.clone(),
                        ExecuteCommandParams {
                            command: command.clone(),
                            arguments: Vec::new(),
                            work_done_progress_params: Default::default(),
                        },
                    ),
                };
                if let Some(work) = self.administrations.get_mut(&id) {
                    work.deadline = operation.deadline;
                    work.stop.send_replace(Some(operation.deadline));
                }
                return match result {
                    Ok(None) => Ok(()),
                    Ok(Some(value)) => respond(connection, id, Ok(value)),
                    Err(error) => {
                        self.command_failure(&error)?;
                        respond(connection, id, Err(error))
                    }
                };
            }
            let result = match completion.result {
                _ if operation.deadline <= Instant::now() => Err(failure(
                    ErrorCode::RequestFailed,
                    "ChatGPT operation timed out",
                )),
                Ok(Outcome::Administration(message)) => {
                    self.ui.inform(&message)?;
                    Ok(Value::Null)
                }
                Err(error) => Err(provider_failure(error)),
                _ => Err(failure(
                    ErrorCode::InternalError,
                    "Invalid account completion",
                )),
            };
            if let Err(error) = &result {
                self.command_failure(error)?;
            }
            return respond(connection, id, result);
        }
        let Some(work) = self.translations.remove(&completion.ticket) else {
            return Ok(());
        };
        let target = &work.target;
        let result = (|| {
            let document = self
                .documents
                .get_mut(&target.uri)
                .ok_or_else(|| failure(ErrorCode::ContentModified, "Document was closed"))?;
            if document.version != target.version
                || document.session != target.session
                || document.text.is_none()
            {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "Document changed during translation",
                ));
            }
            let translation = match completion.result.map_err(provider_failure)? {
                Outcome::Translation(translation) => translation,
                _ => {
                    return Err(failure(
                        ErrorCode::InternalError,
                        "Invalid translation completion",
                    ));
                }
            };
            if self.provider.context().map_err(provider_failure)? != translation.context {
                return Err(failure(
                    ErrorCode::ContentModified,
                    "ChatGPT account changed during translation",
                ));
            }
            document.preview = Some(Preview {
                range: target.range,
                translation: translation.text,
                context: translation.context,
                is_comment: work.is_comment,
            });
            Ok(())
        })();
        if let Err(error) = &result
            && error.code != ErrorCode::ContentModified as i32
            && error.code != ErrorCode::RequestCanceled as i32
        {
            self.record_problem(&work, Problem::Failed);
        } else if self
            .problem_target
            .as_ref()
            .is_some_and(|(target, _)| target == &work.target)
        {
            self.clear_problem();
        }
        self.complete(connection, completion.ticket, &work, result)
    }

    fn complete(
        &mut self,
        connection: &Connection,
        ticket: u64,
        work: &Translation,
        result: RequestResult<()>,
    ) -> ServerResult<()> {
        if self.pending.values().any(|pending| {
            pending.ticket == ticket && matches!(pending.reply, PendingReply::Command)
        }) {
            match &result {
                Ok(()) => self.ui.inform(TRANSLATION_READY)?,
                Err(error) if !work.failure_notified => self.command_failure(error)?,
                Err(_) => (),
            }
        }
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.ticket == ticket)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let pending = self.pending.remove(&id).unwrap();
            let response = match &result {
                Err(error) => Err(error.clone()),
                Ok(()) => match pending.reply {
                    PendingReply::Command => Ok(Value::Null),
                    PendingReply::Hover(position) => to_value(
                        self.documents
                            .get(&work.target.uri)
                            .and_then(|document| document.preview.as_ref())
                            .and_then(|preview| preview.hover(position)),
                    ),
                },
            };
            respond(connection, id, response)?;
        }
        Ok(())
    }

    fn expire(&mut self, connection: &Connection) -> ServerResult<()> {
        let now = Instant::now();
        let ids: Vec<_> = self
            .administrations
            .iter()
            .filter(|(_, work)| work.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let work = self.administrations.remove(&id).unwrap();
            let _ = work.stop.send(None);
            let error = failure(ErrorCode::RequestFailed, "ChatGPT operation timed out");
            self.command_failure(&error)?;
            respond(connection, id, Err(error))?;
        }
        let tickets: Vec<_> = self
            .translations
            .iter()
            .filter(|(_, work)| {
                work.background_deadline
                    .is_some_and(|deadline| deadline <= now)
            })
            .map(|(&ticket, _)| ticket)
            .collect();
        for ticket in tickets {
            self.translations
                .get_mut(&ticket)
                .unwrap()
                .background_deadline = None;
            self.expire_owner(ticket);
        }
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let pending = self.pending.remove(&id).unwrap();
            let error = failure(ErrorCode::RequestFailed, "Translation timed out");
            if matches!(pending.reply, PendingReply::Command) {
                let work = self.translations.get_mut(&pending.ticket).unwrap();
                if !work.failure_notified {
                    work.failure_notified = true;
                    self.command_failure(&error)?;
                }
            }
            self.expire_owner(pending.ticket);
            respond(connection, id, Err(error))?;
        }
        if self.selection.as_ref().is_some_and(|selection| {
            self.can_start_selection(selection) && selection.start.is_some_and(|start| start <= now)
        }) {
            let selection = self.selection.as_mut().unwrap();
            selection.start = None;
            let target = selection.target.clone();
            let context = selection.context.clone();
            if context.is_some() && self.provider.context().ok() == context {
                let _ = self.queue_translation(target, None);
            }
        }
        Ok(())
    }

    fn expire_owner(&mut self, ticket: u64) {
        if let Some(work) = self.translations.get(&ticket) {
            let has_owner = work.background_deadline.is_some()
                || self
                    .pending
                    .values()
                    .any(|pending| pending.ticket == ticket);
            if !has_owner {
                let work = self.translations.remove(&ticket).unwrap();
                work.deadline.send_replace(None);
                self.record_problem(&work, Problem::TimedOut);
                return;
            }
        }
        self.update_deadline(ticket);
    }

    fn cancel_activity(&mut self, connection: &Connection) -> RequestResult<()> {
        self.retire_selection();
        self.clear_problem();
        let ids: Vec<_> = self.pending.keys().cloned().collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::RequestCanceled)?;
        }
        for (_, work) in self.translations.drain() {
            work.deadline.send_replace(None);
        }
        Ok(())
    }

    fn update_deadline(&mut self, ticket: u64) {
        let deadline = self
            .pending
            .values()
            .filter(|pending| pending.ticket == ticket)
            .map(|pending| pending.deadline)
            .chain(
                self.translations
                    .get(&ticket)
                    .and_then(|work| work.background_deadline),
            )
            .max();
        if let Some(work) = self.translations.get(&ticket) {
            work.deadline.send_replace(deadline);
        }
        if deadline.is_none() {
            self.translations.remove(&ticket);
        }
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

    fn invalidate(&mut self, connection: &Connection, uri: &Uri) -> RequestResult<()> {
        if self
            .problem_target
            .as_ref()
            .is_some_and(|(target, _)| &target.uri == uri)
        {
            self.clear_problem();
        }
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| &selection.target.uri == uri)
        {
            self.retire_selection();
        }
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                self.translations
                    .get(&pending.ticket)
                    .is_some_and(|work| &work.target.uri == uri)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::ContentModified)?;
        }
        Ok(())
    }

    fn cancel_all(&mut self, connection: &Connection) -> ServerResult<()> {
        self.clear_problem();
        self.retire_selection();
        let ids: Vec<_> = self
            .pending
            .keys()
            .chain(self.administrations.keys())
            .cloned()
            .collect();
        for id in ids {
            self.cancel(connection, &id, ErrorCode::RequestCanceled)
                .map_err(|error| error.message)?;
        }
        Ok(())
    }

    fn cancel(
        &mut self,
        connection: &Connection,
        id: &RequestId,
        code: ErrorCode,
    ) -> RequestResult<()> {
        if let Some(work) = self.administrations.remove(id) {
            let _ = work.stop.send(None);
            connection
                .sender
                .send(
                    Response {
                        id: id.clone(),
                        response_result: Err(failure(code, "ChatGPT operation cancelled")),
                    }
                    .into(),
                )
                .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
        }
        if let Some(pending) = self.pending.remove(id) {
            self.update_deadline(pending.ticket);
            let response = Response {
                id: id.clone(),
                response_result: Err(failure(
                    code,
                    "Translation preview canceled or document changed",
                )),
            };
            connection
                .sender
                .send(response.into())
                .map_err(|_| failure(ErrorCode::InternalError, "Client disconnected"))?;
        }
        Ok(())
    }
}

async fn worker<P: TranslationProvider>(
    jobs: mpsc::Receiver<Job>,
    administrations: mpsc::Receiver<Job>,
    finished: Sender<WorkerEvent>,
    options: MockOptions,
    provider: P,
) {
    tokio::join!(
        run_jobs(
            administrations,
            finished.clone(),
            MockOptions::default(),
            provider.clone()
        ),
        run_jobs(jobs, finished, options, provider),
    );
}

async fn run_jobs<P: TranslationProvider>(
    mut jobs: mpsc::Receiver<Job>,
    finished: Sender<WorkerEvent>,
    options: MockOptions,
    provider: P,
) {
    'jobs: while let Some(mut job) = jobs.recv().await {
        let Some(deadline) = *job.deadline.borrow_and_update() else {
            continue;
        };
        if deadline <= Instant::now() {
            continue;
        }
        if matches!(job.task, Task::Translation { .. })
            && finished.send(WorkerEvent::Started(job.ticket)).is_err()
        {
            break;
        }
        let operation = async {
            match &job.task {
                Task::AccountMenu { ui } => {
                    let commands = management_commands(provider.auth_ui_state());
                    let titles: Vec<_> = commands
                        .iter()
                        .map(|command| command.title.clone())
                        .collect();
                    ui.choose("", &titles).await.map(|choice| {
                        Outcome::AccountChoice(choice.and_then(|title| {
                            commands
                                .into_iter()
                                .find(|command| command.title == title)
                                .map(|command| command.command)
                        }))
                    })
                }
                Task::Administration { command, ui } => provider
                    .administer(command, ui)
                    .await
                    .map(Outcome::Administration),
                Task::Translation {
                    text,
                    target_language,
                    context,
                } => {
                    tokio::time::sleep(options.delay).await;
                    if options.fail {
                        Err(ProviderError::failed("Simulated mock translation failure"))
                    } else {
                        provider
                            .translate(text, target_language, context.clone())
                            .await
                            .map(Outcome::Translation)
                    }
                }
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
                        continue 'jobs;
                    }
                    let Some(deadline) = *job.deadline.borrow_and_update() else {
                        continue 'jobs;
                    };
                    timer.as_mut().reset(deadline.into());
                }
                _ = &mut timer => break Err(ProviderError::failed("Translation timed out")),
                result = &mut operation => break result,
            }
        };
        if finished
            .send(WorkerEvent::Finished(Finished {
                ticket: job.ticket,
                result,
            }))
            .is_err()
        {
            break;
        }
    }
}

impl<P: TranslationProvider> Drop for Server<P> {
    fn drop(&mut self) {
        self.activity.shutdown();
        for work in self.translations.values() {
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

fn management_commands(state: AuthUiState) -> Vec<Command> {
    ADMIN_COMMANDS
        .iter()
        .filter_map(|(command, title)| {
            let visible = match *command {
                "zed-code-translation.connect" | "zed-code-translation.usage" => true,
                "zed-code-translation.enable" => state == AuthUiState::PlanDisabled,
                "zed-code-translation.sign-out" => {
                    matches!(state, AuthUiState::Ready | AuthUiState::PlanDisabled)
                }
                _ => false,
            };
            if !visible {
                return None;
            }
            let title = match (*command, state) {
                ("zed-code-translation.connect", AuthUiState::Ready) => "Change ChatGPT account",
                ("zed-code-translation.connect", AuthUiState::Unavailable) => {
                    "Check ChatGPT connection…"
                }
                _ => *title,
            };
            Some(Command {
                title: title.into(),
                command: (*command).into(),
                arguments: None,
            })
        })
        .collect()
}

fn same_account(previous: &Context, current: &Context) -> bool {
    match (previous, current) {
        (Context::Mock, Context::Mock) => true,
        (Context::ChatGpt(previous), Context::ChatGpt(current)) => {
            previous.registration == current.registration
        }
        _ => false,
    }
}

fn changes_account(command: &str) -> bool {
    matches!(
        command,
        "zed-code-translation.connect"
            | "zed-code-translation.enable"
            | "zed-code-translation.sign-out"
    )
}

fn provider_failure(mut error: ProviderError) -> ResponseError {
    if error.kind == ErrorKind::RefreshUnusable {
        error.message.push_str(
            ". Choose Continue with ChatGPT / Choose account and reconnect the saved account.",
        );
    }
    failure(
        if error.kind == ErrorKind::Modified {
            ErrorCode::ContentModified
        } else {
            ErrorCode::RequestFailed
        },
        &error.message,
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
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn generic_worker_receives_comment_bodies_and_adjacent_blocks_take_precedence() {
        #[derive(Clone)]
        struct RecordingProvider(Sender<(String, String)>);
        impl TranslationProvider for RecordingProvider {
            async fn translate(
                &self,
                text: &str,
                target: &str,
                context: Context,
            ) -> Result<Translated, ProviderError> {
                self.0.send((text.into(), target.into())).unwrap();
                Ok(Translated {
                    text: format!("fake: {text}"),
                    context,
                })
            }
        }
        let (record, recorded) = bounded(1);
        let (completed, completions) = bounded(MAX_JOBS);
        let mut server =
            Server::new_with_provider(MockOptions::default(), completed, RecordingProvider(record))
                .unwrap();
        let (connection, client) = Connection::memory();
        let uri: Uri = "file:///unsaved.rs".parse().unwrap();
        let source = concat!(
            "  //\t`id` 🚀  \r\n",
            "  //  second\r\n",
            "/** \tkeep\r\n * star */ /* left *//*\u{a0}right */",
        );
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), source.into()),
        );
        let params = |position: Position| {
            parse(json!({"textDocument": {"uri": uri}, "position": position})).unwrap()
        };
        let finish = |server: &mut Server<RecordingProvider>, body: &str| {
            let completion = loop {
                if let WorkerEvent::Finished(completion) =
                    completions.recv_timeout(Duration::from_secs(2)).unwrap()
                {
                    break completion;
                }
            };
            assert_eq!(recorded.recv().unwrap(), (body.to_owned(), "ja".into()));
            server.finish(&connection, completion).unwrap();
            let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                panic!("Expected Hover response");
            };
            assert!(
                response.response_result.unwrap()["contents"]["value"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("fake: {body}"))
            );
        };
        // Indentation before a comment token cannot start work.
        for position in [Position::new(0, 1), Position::new(1, 0)] {
            assert_eq!(
                server.hover(RequestId::from(1), params(position)).unwrap(),
                Some(Value::Null)
            );
        }
        assert!(
            server
                .hover(RequestId::from(2), params(Position::new(0, 14)))
                .unwrap()
                .is_none()
        );
        finish(&mut server, "\t`id` 🚀  \r\n second");
        assert!(
            server
                .hover(RequestId::from(3), params(Position::new(2, 0)))
                .unwrap()
                .is_none()
        );
        finish(&mut server, "\tkeep\r\n * star ");
        let left_start = source.find("/* left").unwrap();
        let left_range = server
            .documents
            .get_mut(&uri)
            .unwrap()
            .comment_range(&uri, left_start)
            .unwrap();
        assert!(
            server
                .hover(RequestId::from(4), params(left_range.start))
                .unwrap()
                .is_none()
        );
        finish(&mut server, "left ");
        assert!(
            server
                .hover(RequestId::from(5), params(left_range.end))
                .unwrap()
                .is_none()
        );
        finish(&mut server, "\u{a0}right ");
        assert_eq!(server.documents[&uri].text.as_deref(), Some(source));
        assert!(server.pending.is_empty());
        assert!(server.translations.is_empty());
    }
    #[test]
    fn stale_completion_ticket_cannot_answer_a_reused_hover_request_id() {
        let (connection, client) = Connection::memory();
        let (finished, _completions) = bounded(MAX_JOBS);
        let mut server = Server::new(
            MockOptions {
                delay: Duration::from_secs(30),
                ..Default::default()
            },
            finished,
        )
        .unwrap();
        let uri: Uri = "file:///ticket.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(
                1,
                1,
                "rust".into(),
                include_str!("../../../../samples/comment.rs").into(),
            ),
        );
        let params = || {
            parse(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 10 }
            }))
            .unwrap()
        };
        let id = RequestId::from("reused".to_owned());
        assert!(server.hover(id.clone(), params()).unwrap().is_none());
        let old_ticket = server.pending[&id].ticket;
        server
            .cancel(&connection, &id, ErrorCode::RequestCanceled)
            .unwrap();
        let Message::Response(canceled) = client.receiver.try_recv().unwrap() else {
            panic!("Expected cancellation response");
        };
        assert!(canceled.response_result.is_err());
        assert!(server.hover(id.clone(), params()).unwrap().is_none());
        let new_ticket = server.pending[&id].ticket;
        assert_ne!(old_ticket, new_ticket);
        server
            .finish(
                &connection,
                Finished {
                    ticket: old_ticket,
                    result: Ok(Outcome::Translation(Translated {
                        text: "stale result".into(),
                        context: Context::Mock,
                    })),
                },
            )
            .unwrap();
        assert!(client.receiver.try_recv().is_err());
        assert!(server.documents[&uri].preview.is_none());
        assert_eq!(server.pending[&id].ticket, new_ticket);
        server
            .finish(
                &connection,
                Finished {
                    ticket: new_ticket,
                    result: Ok(Outcome::Translation(Translated {
                        text: "current result".into(),
                        context: Context::Mock,
                    })),
                },
            )
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected retained Hover response");
        };
        assert_eq!(response.id, id);
        let value = response.response_result.unwrap();
        assert!(
            value["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("current result")
        );
        assert!(!value.to_string().contains("stale result"));
        assert!(server.pending.is_empty());
        assert!(server.translations.is_empty());
    }
}

#[cfg(test)]
mod chatgpt_tests {
    use super::*;
    use crate::providers::chatgpt::test_support::Fixture;
    use serde_json::json;

    #[tokio::test(flavor = "current_thread")]
    async fn configured_auto_toggle_keeps_non_ja_manual_and_comment_translation() {
        for automatic in [false, true] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().translation_segments =
                Some(vec![json!({"id":0,"text":"Translated sentence."})]);
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let settings = Settings::from_options(Some(json!({
                "target_language":"en", "markdown_auto_translate":automatic,
                "hover_timeout_ms":1000, "selection_timeout_ms":1500, "selection_debounce_ms":100,
                "provider":{"name":"chatgpt","model":"synthetic-model"}
            })))
            .unwrap();
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions {
                    timeout_ms: Some(1),
                    ..Default::default()
                },
                finished,
                provider,
                settings,
                Ui::new(connection.sender.clone(), UiCapabilities::default()),
            )
            .unwrap();
            let uri: Uri = "file:///configured-en.md".parse().unwrap();
            let range = Range::new(Position::new(0, 0), Position::new(0, 15));
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "markdown".into(), "First sentence.".into()),
            );
            let actions = markdown_actions(&mut server, &uri, range);
            assert_eq!(actions[0].title, "Translate selection to en");
            assert_eq!(actions[1].title, "ChatGPT account…");
            let command = actions[0].command.clone().unwrap();
            assert_eq!(
                command.arguments.as_ref().unwrap()[0]["targetLanguage"],
                "en"
            );
            if automatic {
                let due = server.selection.as_ref().unwrap().start.unwrap();
                assert!(due <= Instant::now() + Duration::from_millis(100));
                markdown_actions(&mut server, &uri, range);
                assert_eq!(server.selection.as_ref().unwrap().start, Some(due));
                server.expire(&connection).unwrap();
                assert!(server.translations.is_empty());
                assert!(fixture.state.lock().unwrap().records.is_empty());
                tokio::time::sleep(Duration::from_millis(110)).await;
                server.expire(&connection).unwrap();
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
                assert!(
                    client.receiver.is_empty(),
                    "Background completion remains silent"
                );
                markdown_actions(&mut server, &uri, range);
                server.expire(&connection).unwrap();
                assert!(
                    server.translations.is_empty(),
                    "Same selection cannot duplicate completed work"
                );
            } else {
                assert!(server.selection.is_none());
                tokio::time::sleep(Duration::from_millis(110)).await;
                markdown_actions(&mut server, &uri, range);
                server.expire(&connection).unwrap();
                assert!(
                    fixture.state.lock().unwrap().records.is_empty(),
                    "Disabled automatic translation sends nothing"
                );
            }
            server
                .request(
                    &connection,
                    Request::new(
                        1.into(),
                        ExecuteCommand::METHOD.into(),
                        json!({
                            "command":command.command,"arguments":command.arguments.unwrap()
                        }),
                    ),
                )
                .unwrap();
            if !automatic {
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
            }
            assert!(command_notice(&client, 1, 3).unwrap().is_null());
            let markdown = server
                .hover(
                    2.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                        .unwrap(),
                )
                .unwrap()
                .unwrap();
            assert!(
                markdown["contents"]["value"]
                    .as_str()
                    .unwrap()
                    .ends_with("```text\nTranslated sentence.\n```")
            );
            assert_eq!(
                server.documents[&uri].text.as_deref(),
                Some("First sentence.")
            );
            let comment_uri: Uri = "file:///configured-en.rs".parse().unwrap();
            server.documents.insert(
                comment_uri.clone(),
                Document::new(1, 2, "rust".into(), "// First sentence.".into()),
            );
            server
                .request(
                    &connection,
                    Request::new(
                        3.into(),
                        HoverRequest::METHOD.into(),
                        json!({
                            "textDocument":{"uri":comment_uri},"position":{"line":0,"character":4}
                        }),
                    ),
                )
                .unwrap();
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            let Message::Response(comment) = client.receiver.try_recv().unwrap() else {
                panic!("Expected comment Hover");
            };
            assert!(
                comment.response_result.unwrap()["contents"]["value"]
                    .as_str()
                    .unwrap()
                    .contains("Translated sentence.")
            );
            assert_eq!(
                server.documents[&comment_uri].text.as_deref(),
                Some("// First sentence.")
            );
            let records = fixture.state.lock().unwrap().records.clone();
            let responses: Vec<_> = records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .collect();
            assert_eq!(responses.len(), 2);
            for response in responses {
                let body: Value = serde_json::from_str(&response.body).unwrap();
                assert!(body["instructions"].as_str().unwrap().starts_with(
                    "Translate the unprotected text segments into en. Source language: auto."
                ));
            }
            assert!(client.receiver.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn activity_problem_survives_busy_store_and_clears_confirmed_external_invalidation() {
        for plan_disabled in [false, true] {
            let fixture = Fixture::new().await;
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions {
                    fail: true,
                    ..Default::default()
                },
                finished,
                provider,
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        server_status: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///problem-account.rs".parse().unwrap();
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "rust".into(), "// Translate this.".into()),
            );
            server
                .hover(
                    1.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                        .unwrap(),
                )
                .unwrap();
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            let Message::Response(failed) = client.receiver.try_recv().unwrap() else {
                panic!("Expected failure");
            };
            assert!(failed.response_result.is_err());
            server.publish_activity();
            let Message::Notification(problem) = client.receiver.try_recv().unwrap() else {
                panic!("Expected warning");
            };
            assert_eq!(problem.params["health"], "warning");
            let mut locked = auth.store.lock(false).await.unwrap().unwrap();
            assert_eq!(auth.ui_state(), AuthUiState::Unavailable);
            server.publish_activity();
            assert!(
                client.receiver.is_empty(),
                "Busy is not a confirmed account change"
            );
            let id = locked.state.active_registration.unwrap();
            let registration = locked.state.registrations.get_mut(&id).unwrap();
            if plan_disabled {
                registration.credentials.as_mut().unwrap().scopes.clear();
            } else {
                registration.credentials = None;
            }
            locked.save().unwrap();
            drop(locked);
            assert_eq!(
                auth.ui_state(),
                if plan_disabled {
                    AuthUiState::PlanDisabled
                } else {
                    AuthUiState::SignedOut
                }
            );
            server.publish_activity();
            let Message::Notification(clear) = client.receiver.try_recv().unwrap() else {
                panic!("Expected clear");
            };
            assert_eq!(clear.params, json!({"health":"ok","quiescent":true}));
            server.publish_activity();
            assert!(client.receiver.is_empty());
            assert!(fixture.state.lock().unwrap().records.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn activity_keeps_background_owner_after_hover_expiry_and_allows_token_refresh() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().stream_tail_delay = Duration::from_millis(300);
        let auth = fixture.auth();
        fixture.connected(&auth, 0).await;
        fixture.state.lock().unwrap().delay = Duration::from_millis(200);
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions {
                delay: Duration::from_millis(100),
                ..Default::default()
            },
            finished,
            provider.clone(),
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    work_done_progress: true,
                    server_status: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///activity-background.md".parse().unwrap();
        let range = Range::new(Position::new(0, 0), Position::new(0, 15));
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "markdown".into(), "First sentence.".into()),
        );
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        server
            .hover(
                1.into(),
                parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                    .unwrap(),
            )
            .unwrap();
        let ticket = server.pending[&1.into()].ticket;
        let original_context = server.translations[&ticket].context.clone();
        server.publish_activity();
        let Message::Request(create) = client.receiver.try_recv().unwrap() else {
            panic!("Expected create");
        };
        assert!(
            server
                .activity
                .response(&Response::new_ok(create.id, Value::Null))
        );
        server.publish_activity();
        let Message::Notification(begin) = client.receiver.try_recv().unwrap() else {
            panic!("Expected begin");
        };
        assert_eq!(begin.params["value"]["message"], "Queued: 1");
        let locked = auth.store.lock(false).await.unwrap().unwrap();
        let events = completions.clone();
        let started = tokio::task::spawn_blocking(move || {
            events.recv_timeout(Duration::from_secs(2)).unwrap()
        })
        .await
        .unwrap();
        let WorkerEvent::Started(started_ticket) = started else {
            panic!("Expected worker Started");
        };
        assert_eq!(started_ticket, ticket);
        server.translations.get_mut(&ticket).unwrap().phase = Phase::Running;
        assert!(matches!(
            auth.store.snapshot(),
            Err(crate::providers::chatgpt::auth_store::StoreError::Busy)
        ));
        assert!(provider.context().is_err());
        tokio::time::sleep(Duration::from_millis(160)).await;
        server.publish_activity();
        let Message::Notification(running) = client.receiver.try_recv().unwrap() else {
            panic!("Expected running report during store lock");
        };
        assert_eq!(running.params["value"]["kind"], "report");
        assert_eq!(
            running.params["value"]["message"],
            "Translating: 1, queued: 0"
        );
        server.pending.get_mut(&1.into()).unwrap().deadline = Instant::now();
        server.expire(&connection).unwrap();
        server.publish_activity();
        let Message::Response(expired) = client.receiver.try_recv().unwrap() else {
            panic!("Expected expired Hover");
        };
        assert_eq!(
            expired.response_result.unwrap_err().code,
            ErrorCode::RequestFailed as i32
        );
        assert!(server.translations[&ticket].background_deadline.is_some());
        assert!(
            client.receiver.is_empty(),
            "Remaining background owner keeps progress alive"
        );
        drop(locked);
        tokio::time::timeout(Duration::from_secs(2), async {
            while !fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .any(|record| record.path == "/token")
            {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            auth.store.snapshot(),
            Err(crate::providers::chatgpt::auth_store::StoreError::Busy)
        ));
        server.publish_activity();
        assert!(
            client.receiver.is_empty(),
            "HTTP refresh holding auth lock keeps progress alive"
        );
        let completion = next(&completions).await;
        assert_ne!(
            provider.context().unwrap(),
            original_context,
            "Synthetic expired credentials must refresh"
        );
        server.publish_activity();
        assert!(
            client.receiver.is_empty(),
            "Credential revision change must not end valid work"
        );
        server.finish(&connection, completion).unwrap();
        server.publish_activity();
        let Message::Notification(end) = client.receiver.try_recv().unwrap() else {
            panic!("Expected end");
        };
        assert_eq!(
            end.params,
            json!({"token":create.params["token"],"value":{"kind":"end"}})
        );
        assert!(server.documents[&uri].preview.is_some());
        assert!(
            client.receiver.is_empty(),
            "Background completion adds no status or INFO"
        );
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .count(),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn activity_cancellation_leaves_account_picker_and_its_deadline_alive() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions {
                delay: Duration::from_secs(1),
                ..Default::default()
            },
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    work_done_progress: true,
                    server_status: true,
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///activity-picker.md".parse().unwrap();
        let range = Range::new(Position::new(0, 0), Position::new(0, 15));
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "markdown".into(), "First sentence.".into()),
        );
        markdown_actions(&mut server, &uri, range);
        server.selection.as_mut().unwrap().start = Some(Instant::now());
        server.expire(&connection).unwrap();
        server.publish_activity();
        let Message::Request(create) = client.receiver.try_recv().unwrap() else {
            panic!("Expected create");
        };
        server
            .activity
            .response(&Response::new_ok(create.id, Value::Null));
        server.publish_activity();
        let _begin = client.receiver.try_recv().unwrap();
        server
            .request(
                &connection,
                Request::new(
                    2.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({"command":SETTINGS_COMMAND}),
                ),
            )
            .unwrap();
        let Message::Request(picker) = protocol_message(&client).await else {
            panic!("Expected picker");
        };
        assert_eq!(picker.method, "window/showMessageRequest");
        let deadline = server.administrations[&2.into()].deadline;
        server
            .notification(
                &connection,
                lsp_server::Notification::new(
                    "window/workDoneProgress/cancel".into(),
                    json!({"token":create.params["token"]}),
                ),
            )
            .unwrap();
        server.publish_activity();
        assert!(server.translations.is_empty());
        assert!(server.selection.is_none());
        assert_eq!(
            *server.administrations[&2.into()].stop.borrow(),
            Some(deadline)
        );
        let Message::Notification(end) = client.receiver.try_recv().unwrap() else {
            panic!("Expected end");
        };
        assert_eq!(end.params["value"]["kind"], "end");
        assert!(client.receiver.is_empty(), "Picker must remain pending");
        server.ui.response(Response::new_ok(picker.id, Value::Null));
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let Message::Response(dismissed) = client.receiver.try_recv().unwrap() else {
            panic!("Expected picker dismissal");
        };
        assert_eq!(dismissed.id, 2.into());
        assert!(dismissed.response_result.unwrap().is_null());
        assert!(client.receiver.is_empty());
        assert!(fixture.state.lock().unwrap().records.is_empty());
    }

    async fn next(receiver: &crossbeam_channel::Receiver<WorkerEvent>) -> Finished {
        let receiver = receiver.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                if let WorkerEvent::Finished(completion) =
                    receiver.recv_timeout(Duration::from_secs(3)).unwrap()
                {
                    break completion;
                }
            }
        })
        .await
        .unwrap()
    }

    fn command_notice(client: &Connection, id: i32, level: u64) -> RequestResult<Value> {
        let Message::Notification(notification) = client.receiver.try_recv().unwrap() else {
            panic!("Expected command notification");
        };
        assert_eq!(notification.method, "window/showMessage");
        assert_eq!(notification.params["type"], level);
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected command response");
        };
        assert_eq!(response.id, RequestId::from(id));
        match &response.response_result {
            Ok(value) => assert!(value.is_null()),
            Err(error) => assert_eq!(notification.params["message"], error.message),
        }
        assert!(
            client.receiver.try_recv().is_err(),
            "Duplicate command notification"
        );
        response.response_result
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_deadlines_keep_one_http_operation_and_start_at_each_admission() {
        let defaults = Settings::default();
        assert_eq!(defaults.hover_timeout_ms, 20_000);
        assert_eq!(defaults.selection_timeout_ms, 30_000);
        for mode in [
            "manual",
            "hover",
            "hover-first",
            "manual-first",
            "late-hover",
            "late-manual",
        ] {
            let fixture = Fixture::new().await;
            {
                let mut state = fixture.state.lock().unwrap();
                state.response_mode = "delta_compact".into();
                state.stream_tail_delay =
                    Duration::from_millis(if mode == "late-manual" { 700 } else { 320 });
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions {
                    timeout_ms: Some(1),
                    ..Default::default()
                },
                finished,
                provider,
                Settings {
                    hover_timeout_ms: 200,
                    selection_timeout_ms: 600,
                    ..Default::default()
                },
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///deadline.rs".parse().unwrap();
            let source = "// Translate this.\n";
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "rust".into(), source.into()),
            );
            let target = || TranslationTarget {
                uri: uri.clone(),
                version: 1,
                session: 1,
                range: Range::new(Position::new(0, 0), Position::new(0, 18)),
                target_language: "ja".into(),
            };
            let first_hover = mode == "hover" || mode == "hover-first";
            server
                .queue(
                    1.into(),
                    target(),
                    if first_hover {
                        PendingReply::Hover(Position::new(0, 4))
                    } else {
                        PendingReply::Command
                    },
                )
                .unwrap();
            let first_deadline = server.pending[&1.into()].deadline;
            // Wait until the HTTP request has actually started, before extending it.
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if fixture
                        .state
                        .lock()
                        .unwrap()
                        .records
                        .iter()
                        .any(|r| r.path == "/v1/responses")
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            if matches!(
                mode,
                "hover-first" | "manual-first" | "late-hover" | "late-manual"
            ) {
                tokio::time::sleep(Duration::from_millis(
                    if mode == "late-hover" || mode == "late-manual" {
                        180
                    } else {
                        70
                    },
                ))
                .await;
                server
                    .queue(
                        2.into(),
                        target(),
                        if first_hover || mode == "late-manual" {
                            PendingReply::Command
                        } else {
                            PendingReply::Hover(Position::new(0, 4))
                        },
                    )
                    .unwrap();
                let second = &server.pending[&2.into()];
                assert_eq!(second.ticket, server.pending[&1.into()].ticket);
                assert_eq!(server.translations.len(), 1);
                if first_hover || mode == "late-manual" {
                    assert!(second.deadline > first_deadline);
                } else {
                    assert!(second.deadline > first_deadline - Duration::from_millis(400));
                }
            }
            let expired_hover = if first_hover {
                Some(RequestId::from(1))
            } else if mode == "manual-first" {
                Some(RequestId::from(2))
            } else {
                None
            };
            if let Some(id) = expired_hover {
                let deadline = server.pending[&id].deadline;
                tokio::time::sleep_until((deadline + Duration::from_millis(20)).into()).await;
                server.expire(&connection).unwrap();
                let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                    panic!("Expected expired Hover");
                };
                assert_eq!(response.id, id);
                assert_eq!(
                    response.response_result.unwrap_err().code,
                    ErrorCode::RequestFailed as i32
                );
                assert!(
                    client.receiver.try_recv().is_err(),
                    "Automatic Hover must stay silent"
                );
                assert!(server.documents[&uri].preview.is_none());
            }
            if mode == "late-manual" {
                // A later command gets its own full window, beyond the first command's.
                tokio::time::sleep_until((first_deadline + Duration::from_millis(20)).into()).await;
                server.expire(&connection).unwrap();
                let error = command_notice(&client, 1, 1).unwrap_err();
                assert_eq!(error.code, ErrorCode::RequestFailed as i32);
                assert!(server.documents[&uri].preview.is_none());
                assert_eq!(server.pending.len(), 1);
                assert!(server.pending.contains_key(&2.into()));
            }
            if mode == "hover" {
                assert!(server.translations.is_empty());
                assert!(server.pending.is_empty());
            } else {
                let completion = next(&completions).await;
                assert!(
                    completion.result.is_ok(),
                    "{mode}: operation was restarted or timed out"
                );
                server.finish(&connection, completion).unwrap();
                assert!(server.documents[&uri].preview.is_some());
                let Message::Notification(notification) = client.receiver.try_recv().unwrap()
                else {
                    panic!("Expected one ready notification");
                };
                assert_eq!(
                    notification.params,
                    json!({"type":3,"message":TRANSLATION_READY})
                );
                let count = if mode == "late-hover" { 2 } else { 1 };
                for _ in 0..count {
                    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                        panic!("Expected successful waiter");
                    };
                    assert!(response.response_result.is_ok(), "{mode}");
                }
                assert!(client.receiver.try_recv().is_err());
                assert!(server.pending.is_empty());
                assert!(server.translations.is_empty());
            }
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|r| r.path == "/v1/responses")
                    .count(),
                1,
                "{mode}: HTTP must not restart on deadline update"
            );
            assert_eq!(server.documents[&uri].text.as_deref(), Some(source));
            drop(completions);
            let before = Instant::now();
            drop(server);
            assert!(before.elapsed() < Duration::from_millis(200));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_preview_reports_admission_refresh_and_stream_failures() {
        let source = "Private source sentence.";
        for mode in ["admission", "refresh", "stream", "wrapped-stream"] {
            let fixture = Fixture::new().await;
            let auth = fixture.auth();
            if mode != "admission" {
                fixture
                    .connected(&auth, if mode == "refresh" { 0 } else { 3600 })
                    .await;
            }
            if mode == "refresh" {
                fixture.state.lock().unwrap().refresh_failure = Some((
                    400,
                    json!({"error":"invalid_grant","error_description":"synthetic refresh invalid"}),
                ));
            } else if mode == "stream" {
                fixture.state.lock().unwrap().stream_error = Some(
                    json!({"type":"error","code":"unsupported_value","param":"reasoning.effort","message":format!("unsupported: {source} synthetic-access-0")}),
                );
            } else if mode == "wrapped-stream" {
                fixture.state.lock().unwrap().stream_error =
                    Some(json!({"type":"error","sequence_number":1,"error":{
                        "code":"unsupported_value","param":"reasoning.effort",
                        "message":format!("unsupported: {source} synthetic-access-0")}}));
            }
            let before = auth.store.snapshot().unwrap();
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///command-failure.md".parse().unwrap();
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "markdown".into(), source.into()),
            );
            let range =
                json!({"start":{"line":0,"character":0},"end":{"line":0,"character":source.len()}});
            let action = json!({"command":COMMAND,"arguments":[{"uri":uri,"version":1,"session":1,"range":range,"targetLanguage":"ja"}]});
            server
                .request(
                    &connection,
                    Request::new(1.into(), ExecuteCommand::METHOD.into(), action),
                )
                .unwrap();
            if mode != "admission" {
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
            }
            let error = command_notice(&client, 1, 1).unwrap_err();
            assert_eq!(error.code, ErrorCode::RequestFailed as i32);
            assert!(!error.message.contains(source));
            assert!(!error.message.contains("synthetic-access-0"));
            assert!(server.documents[&uri].preview.is_none());
            assert_eq!(server.documents[&uri].text.as_deref(), Some(source));
            if mode == "refresh" {
                assert!(error.message.contains("code=invalid_grant"));
                assert!(error.message.contains("request_id=req-synthetic"));
                assert!(error.message.contains("reconnect the saved account"));
                let before = before.unwrap().state;
                let after = auth.store.snapshot().unwrap().unwrap().state;
                let id = before.active_registration.unwrap();
                assert_eq!(after.active_registration, Some(id));
                assert_eq!(after.ext_agent_host_id, before.ext_agent_host_id);
                assert_eq!(
                    after.registrations[&id].client_id,
                    before.registrations[&id].client_id
                );
                assert!(after.registrations[&id].credentials.is_none());
            } else if matches!(mode, "stream" | "wrapped-stream") {
                assert!(error.message.contains("event=error"));
                assert!(error.message.contains("code=unsupported_value"));
                assert!(error.message.contains("param=reasoning.effort"));
                assert!(error.message.contains("request_id=req-synthetic"));
                assert!(!error.message.contains("event_keys="));
                assert!(error.message.contains("[redacted]"));
            }
            let records = fixture.state.lock().unwrap().records.len();
            server.request(&connection, Request::new(2.into(), CodeActionRequest::METHOD.into(), json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":[]}}))).unwrap();
            let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                panic!("Expected local candidates");
            };
            assert_eq!(
                response.response_result.unwrap().as_array().unwrap().len(),
                if mode == "admission" || mode == "refresh" {
                    1
                } else {
                    2
                }
            );
            assert!(client.receiver.try_recv().is_err());
            assert_eq!(fixture.state.lock().unwrap().records.len(), records);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn administration_reports_admission_worker_and_deadline_failures_but_not_cancel() {
        let fixture = Fixture::new().await;
        let provider = ChatGptProvider::synthetic(
            fixture.auth(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(connection.sender.clone(), UiCapabilities::default()),
        )
        .unwrap();
        server
            .request(
                &connection,
                Request::new(
                    1.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({"command":"zed-code-translation.connect","arguments":["unexpected"]}),
                ),
            )
            .unwrap();
        assert_eq!(
            command_notice(&client, 1, 1).unwrap_err().code,
            ErrorCode::InvalidParams as i32
        );
        let usage = json!({"command":"zed-code-translation.usage"});
        server
            .request(
                &connection,
                Request::new(2.into(), ExecuteCommand::METHOD.into(), usage.clone()),
            )
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert!(
            command_notice(&client, 2, 1)
                .unwrap_err()
                .message
                .contains("window/showDocument")
        );
        server
            .request(
                &connection,
                Request::new(3.into(), ExecuteCommand::METHOD.into(), usage.clone()),
            )
            .unwrap();
        let work = server.administrations.get_mut(&3.into()).unwrap();
        let ticket = work.ticket;
        work.deadline = Instant::now();
        server.expire(&connection).unwrap();
        let error = command_notice(&client, 3, 1).unwrap_err();
        assert_eq!(error.code, ErrorCode::RequestFailed as i32);
        assert!(error.message.contains("timed out"));
        server
            .finish(
                &connection,
                Finished {
                    ticket,
                    result: Err(ProviderError::failed("late completion")),
                },
            )
            .unwrap();
        assert!(client.receiver.try_recv().is_err());
        // Cancellation retires a queued job asynchronously. Wait for that slot so
        // this case tests request cancellation, rather than queue-busy admission.
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.admin_jobs.as_ref().unwrap().capacity() == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        server
            .request(
                &connection,
                Request::new(4.into(), ExecuteCommand::METHOD.into(), usage),
            )
            .unwrap();
        server
            .cancel(&connection, &4.into(), ErrorCode::RequestCanceled)
            .unwrap();
        let Message::Response(response) = client.receiver.try_recv().unwrap() else {
            panic!("Expected cancellation response");
        };
        assert_eq!(
            response.response_result.unwrap_err().code,
            ErrorCode::RequestCanceled as i32
        );
        assert!(client.receiver.try_recv().is_err());
        assert!(fixture.state.lock().unwrap().records.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chatgpt_worker_coalesces_and_invalidates_completed_cache_on_account_changes() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().response_mode = "delta_compact".into();
        let auth = fixture.auth();
        fixture.connected(&auth, 0).await;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///synthetic.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(
                1,
                1,
                "rust".into(),
                "// Translate this.\nlet value = 1;\n".into(),
            ),
        );
        let hover = || {
            parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}})).unwrap()
        };
        assert!(server.hover(RequestId::from(1), hover()).unwrap().is_none());
        assert!(server.hover(RequestId::from(2), hover()).unwrap().is_none());
        assert_eq!(server.translations.len(), 1);
        server
            .cancel(&connection, &RequestId::from(1), ErrorCode::RequestCanceled)
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert!(server.documents[&uri].preview.is_some());
        assert!(server.hover(RequestId::from(3), hover()).unwrap().is_some());
        let records = fixture.state.lock().unwrap().records.clone();
        assert_eq!(
            records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .count(),
            1
        );
        assert!(
            records
                .iter()
                .filter(|record| record.path.starts_with("/v1/"))
                .all(|record| record.authorization == "Bearer synthetic-access-1")
        );
        let _ = client.receiver.try_recv().unwrap();
        let _ = client.receiver.try_recv().unwrap();
        // A second registration changes the key and starts a fresh inference.
        fixture.connected(&auth, 3600).await;
        assert!(server.hover(RequestId::from(4), hover()).unwrap().is_none());
        assert!(server.documents[&uri].preview.is_none());
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.path == "/v1/models")
                .count(),
            0
        );
        assert_eq!(
            server.documents[&uri].text.as_deref(),
            Some("// Translate this.\nlet value = 1;\n")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chatgpt_worker_never_caches_failed_compact_streams() {
        for mode in [
            "compact_partial_json",
            "item_role",
            "item_id_mismatch",
            "text_done_mismatch",
            "compact_truncated",
            "compact_incomplete",
            "compact_late_failed",
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().response_mode = mode.into();
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider.clone(),
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///failed-stream.rs".parse().unwrap();
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
            );
            let hover = || {
                parse(json!({
                    "textDocument":{"uri":uri}, "position":{"line":0,"character":4}
                }))
                .unwrap()
            };
            assert!(server.hover(RequestId::from(1), hover()).unwrap().is_none());
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            assert!(server.documents[&uri].preview.is_none(), "SSE mode {mode}");
            assert!(server.pending.is_empty());
            assert!(server.translations.is_empty());
            let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                panic!("Expected failed Hover response");
            };
            assert!(response.response_result.is_err());
            if mode == "compact_late_failed" {
                assert!(provider.context().is_err());
                assert_eq!(
                    fixture
                        .state
                        .lock()
                        .unwrap()
                        .records
                        .iter()
                        .filter(|record| record.path == "/v1/responses")
                        .count(),
                    1
                );
            } else {
                fixture.state.lock().unwrap().response_mode = "delta_compact".into();
                assert!(server.hover(RequestId::from(2), hover()).unwrap().is_none());
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
                assert!(server.documents[&uri].preview.is_some());
                assert!(server.hover(RequestId::from(3), hover()).unwrap().is_some());
                assert_eq!(
                    fixture
                        .state
                        .lock()
                        .unwrap()
                        .records
                        .iter()
                        .filter(|record| record.path == "/v1/responses")
                        .count(),
                    2
                );
            }
            assert_eq!(
                server.documents[&uri].text.as_deref(),
                Some("// Translate this.\n")
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chatgpt_worker_drops_outbound_on_cancel_deadline_edit_and_shutdown() {
        for operation in ["cancel", "deadline", "edit", "shutdown"] {
            let fixture = Fixture::new().await;
            {
                let mut state = fixture.state.lock().unwrap();
                state.response_mode = "delta_compact".into();
                state.stream_tail_delay = Duration::from_secs(5);
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings {
                    hover_timeout_ms: 250,
                    ..Default::default()
                },
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///cancel-synthetic.rs".parse().unwrap();
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
            );
            server
                .hover(
                    RequestId::from(1),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                        .unwrap(),
                )
                .unwrap();
            tokio::time::sleep(Duration::from_millis(120)).await;
            let target = TranslationTarget {
                uri: uri.clone(),
                version: 1,
                session: 1,
                range: Range::new(Position::new(0, 0), Position::new(0, 18)),
                target_language: "ja".into(),
            };
            server
                .queue(2.into(), target, PendingReply::Command)
                .unwrap();
            let ticket = server.pending[&1.into()].ticket;
            assert_eq!(server.pending[&2.into()].ticket, ticket);
            assert_eq!(
                *server.translations[&ticket].deadline.borrow(),
                Some(server.pending[&2.into()].deadline)
            );
            assert!(
                matches!(completions.try_recv().unwrap(), WorkerEvent::Started(started) if started == ticket)
            );
            assert!(
                completions.is_empty(),
                "Stream EOF has not completed the worker"
            );
            assert!(server.documents[&uri].preview.is_none());
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                1
            );
            match operation {
                "deadline" => {
                    // Removing the extending waiter restores the remaining Hover's deadline.
                    server
                        .cancel(&connection, &2.into(), ErrorCode::RequestCanceled)
                        .unwrap();
                    assert_eq!(
                        *server.translations[&ticket].deadline.borrow(),
                        Some(server.pending[&1.into()].deadline)
                    );
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    server.expire(&connection).unwrap();
                }
                "edit" => {
                    server.notification(&connection,lsp_server::Notification::new("textDocument/didChange".into(),json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"// Changed source.\n"}]}))).unwrap();
                }
                _ => {
                    server.cancel_all(&connection).unwrap();
                }
            }
            for _ in 0..2 {
                let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                    panic!("Expected failed waiter without notification");
                };
                let expected = match operation {
                    "edit" => ErrorCode::ContentModified,
                    "deadline" if response.id == RequestId::from(1) => ErrorCode::RequestFailed,
                    _ => ErrorCode::RequestCanceled,
                };
                assert_eq!(response.response_result.unwrap_err().code, expected as i32);
            }
            assert!(client.receiver.try_recv().is_err());
            assert!(server.pending.is_empty());
            assert!(server.translations.is_empty());
            assert!(server.documents[&uri].preview.is_none());
            let start = Instant::now();
            drop(completions);
            drop(server);
            assert!(start.elapsed() < Duration::from_millis(500));
        }
    }

    fn markdown_actions(
        server: &mut Server<ChatGptProvider>,
        uri: &Uri,
        range: Range,
    ) -> Vec<CodeAction> {
        server
            .code_actions(
                parse(json!({
                    "textDocument": {"uri": uri}, "range": range, "context": {"diagnostics": []}
                }))
                .unwrap(),
            )
            .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn background_owners_share_one_http_and_keep_waiter_deadlines_independent() {
        for mode in [
            "deselect",
            "auto-disabled",
            "cancel-clients",
            "background-expiry",
        ] {
            let fixture = Fixture::new().await;
            fixture.state.lock().unwrap().stream_tail_delay =
                Duration::from_millis(if mode == "background-expiry" {
                    900
                } else {
                    550
                });
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth,
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings {
                    hover_timeout_ms: 400,
                    selection_timeout_ms: 800,
                    ..Default::default()
                },
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///background.md".parse().unwrap();
            let range = Range::new(Position::new(0, 0), Position::new(0, 15));
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "markdown".into(), "First sentence.".into()),
            );
            assert_eq!(markdown_actions(&mut server, &uri, range).len(), 2);
            assert!(server.translations.is_empty());
            let due = server.selection.as_ref().unwrap().start.unwrap();
            assert!(due > Instant::now() + Duration::from_millis(400));
            markdown_actions(&mut server, &uri, range);
            assert_eq!(
                server.selection.as_ref().unwrap().start,
                Some(due),
                "Same range must not reset stability"
            );
            server.selection.as_mut().unwrap().start = Some(Instant::now());
            server.expire(&connection).unwrap();
            let ticket = *server.translations.keys().next().unwrap();
            let target = server.selection.as_ref().unwrap().target.clone();
            if mode == "background-expiry" {
                tokio::time::sleep(Duration::from_millis(650)).await;
            }
            server
                .hover(
                    1.into(),
                    parse(json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}))
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(server.pending[&1.into()].ticket, ticket);
            if mode == "background-expiry" {
                tokio::time::sleep(Duration::from_millis(170)).await;
                server.expire(&connection).unwrap();
                assert!(server.translations[&ticket].background_deadline.is_none());
                assert!(client.receiver.try_recv().is_err());
            } else {
                server
                    .queue(2.into(), target, PendingReply::Command)
                    .unwrap();
                assert_eq!(server.pending[&2.into()].ticket, ticket);
                if matches!(mode, "deselect" | "auto-disabled") {
                    if mode == "auto-disabled" {
                        // Exercise retirement with an existing owner; public settings apply on restart.
                        server.settings.markdown_auto_translate = false;
                        markdown_actions(&mut server, &uri, range);
                    } else {
                        markdown_actions(&mut server, &uri, Range::new(range.start, range.start));
                    }
                    assert!(server.selection.is_none());
                    assert!(server.translations[&ticket].background_deadline.is_none());
                    tokio::time::sleep(Duration::from_millis(420)).await;
                    server.expire(&connection).unwrap();
                    let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                        panic!("Expected expired Hover");
                    };
                    assert_eq!(response.id, 1.into());
                    assert_eq!(
                        response.response_result.unwrap_err().code,
                        ErrorCode::RequestFailed as i32
                    );
                } else {
                    for id in [1, 2] {
                        server
                            .cancel(&connection, &id.into(), ErrorCode::RequestCanceled)
                            .unwrap();
                        let Message::Response(response) = client.receiver.try_recv().unwrap()
                        else {
                            panic!("Expected cancellation");
                        };
                        assert_eq!(
                            response.response_result.unwrap_err().code,
                            ErrorCode::RequestCanceled as i32
                        );
                    }
                    assert!(server.translations[&ticket].background_deadline.is_some());
                }
                assert!(client.receiver.try_recv().is_err());
            }
            let completion = next(&completions).await;
            assert!(
                completion.result.is_ok(),
                "{mode}: {:?}",
                completion.result.as_ref().err()
            );
            server.finish(&connection, completion).unwrap();
            assert!(server.documents[&uri].preview.is_some(), "{mode}");
            if matches!(mode, "deselect" | "auto-disabled") {
                command_notice(&client, 2, 3).unwrap();
            } else if mode == "background-expiry" {
                let Message::Response(response) = client.receiver.try_recv().unwrap() else {
                    panic!("Expected Hover result");
                };
                assert!(response.response_result.is_ok());
            }
            assert!(
                client.receiver.try_recv().is_err(),
                "Background success must be silent"
            );
            assert_eq!(
                fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .filter(|record| record.path == "/v1/responses")
                    .count(),
                1
            );
            assert_eq!(
                server.documents[&uri].text.as_deref(),
                Some("First sentence.")
            );
            drop(completions);
            drop(server);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn background_selection_changes_cancel_silently_and_failures_do_not_retry() {
        for mode in [
            "new-range",
            "empty",
            "edit",
            "close",
            "account",
            "shutdown",
            "failure",
        ] {
            let fixture = Fixture::new().await;
            if mode == "failure" {
                fixture.state.lock().unwrap().failure = Some((
                    400,
                    json!({"error":{"code":"synthetic_failure","message":"synthetic failure"}}),
                ));
            } else if mode != "new-range" {
                fixture.state.lock().unwrap().stream_tail_delay = Duration::from_secs(5);
            }
            let auth = fixture.auth();
            fixture.connected(&auth, 3600).await;
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///changes.md".parse().unwrap();
            let range = Range::new(Position::new(0, 0), Position::new(0, 15));
            server.documents.insert(
                uri.clone(),
                Document::new(
                    1,
                    1,
                    "Markdown".into(),
                    "First sentence.\nSecond sentence.".into(),
                ),
            );
            markdown_actions(&mut server, &uri, range);
            if mode == "new-range" {
                let second = Range::new(Position::new(1, 0), Position::new(1, 16));
                markdown_actions(&mut server, &uri, second);
                server.selection.as_mut().unwrap().start = Some(Instant::now());
                server.expire(&connection).unwrap();
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
                assert_eq!(
                    server.documents[&uri].preview.as_ref().unwrap().range,
                    second
                );
                let records = fixture.state.lock().unwrap().records.clone();
                let body = &records
                    .iter()
                    .find(|record| record.path == "/v1/responses")
                    .unwrap()
                    .body;
                assert!(body.contains("Second sentence."));
                assert!(!body.contains("First sentence."));
            } else {
                server.selection.as_mut().unwrap().start = Some(Instant::now());
                server.expire(&connection).unwrap();
                if mode == "failure" {
                    server
                        .finish(&connection, next(&completions).await)
                        .unwrap();
                    fixture.state.lock().unwrap().failure = None;
                    for _ in 0..2 {
                        markdown_actions(&mut server, &uri, range);
                        server.expire(&connection).unwrap();
                        assert!(server.selection.as_ref().unwrap().start.is_none());
                    }
                    assert!(server.documents[&uri].preview.is_none());
                    let target = server.selection.as_ref().unwrap().target.clone();
                    server
                        .queue(1.into(), target, PendingReply::Command)
                        .unwrap();
                    server
                        .finish(&connection, next(&completions).await)
                        .unwrap();
                    command_notice(&client, 1, 3).unwrap();
                    assert!(server.documents[&uri].preview.is_some());
                    assert_eq!(
                        fixture
                            .state
                            .lock()
                            .unwrap()
                            .records
                            .iter()
                            .filter(|record| record.path == "/v1/responses")
                            .count(),
                        2
                    );
                } else {
                    tokio::time::timeout(Duration::from_secs(1), async {
                        loop {
                            if fixture
                                .state
                                .lock()
                                .unwrap()
                                .records
                                .iter()
                                .any(|record| record.path == "/v1/responses")
                            {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    })
                    .await
                    .unwrap();
                    match mode {
                        "empty" => {
                            markdown_actions(
                                &mut server,
                                &uri,
                                Range::new(range.start, range.start),
                            );
                        }
                        "edit" => {
                            server
                                .notification(
                                    &connection,
                                    lsp_server::Notification::new(
                                        "textDocument/didChange".into(),
                                        json!({
                                            "textDocument": {"uri": uri, "version": 2},
                                            "contentChanges": [{"text": "Changed sentence."}]
                                        }),
                                    ),
                                )
                                .unwrap();
                        }
                        "close" => {
                            server
                                .notification(
                                    &connection,
                                    lsp_server::Notification::new(
                                        "textDocument/didClose".into(),
                                        json!({"textDocument":{"uri":uri}}),
                                    ),
                                )
                                .unwrap();
                        }
                        "account" => {
                            fixture.connected(&auth, 3600).await;
                            let completion = next(&completions).await;
                            assert!(
                                matches!(&completion.result,Err(error) if error.kind == ErrorKind::Modified)
                            );
                            server.finish(&connection, completion).unwrap();
                        }
                        _ => {
                            server.cancel_all(&connection).unwrap();
                        }
                    }
                    assert!(server.translations.is_empty());
                    assert!(server.pending.is_empty());
                    assert!(
                        server
                            .documents
                            .get(&uri)
                            .is_none_or(|doc| doc.preview.is_none())
                    );
                }
            }
            assert!(
                client.receiver.try_recv().is_err(),
                "Background outcome/cancellation must be silent"
            );
            drop(completions);
            let before = Instant::now();
            drop(server);
            assert!(before.elapsed() < Duration::from_millis(500));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn automatic_selection_excludes_code_whitespace_non_markdown_and_ineligible_auth() {
        for (mode, language, text) in [
            ("code", "markdown", "`code`"),
            ("space", "markdown", " "),
            ("large", "markdown", "sentence"),
            ("rust", "rust", "sentence"),
            ("signed-out", "markdown", "sentence"),
            ("disabled", "markdown", "sentence"),
        ] {
            let fixture = Fixture::new().await;
            let auth = fixture.auth();
            if mode != "signed-out" {
                let id = fixture.connected(&auth, 3600).await;
                if mode == "disabled" {
                    let mut locked = auth.store.lock(false).await.unwrap().unwrap();
                    locked
                        .state
                        .registrations
                        .get_mut(&id)
                        .unwrap()
                        .credentials
                        .as_mut()
                        .unwrap()
                        .scopes
                        .clear();
                    locked.save().unwrap();
                }
            }
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///excluded.md".parse().unwrap();
            let text = if mode == "large" {
                "a".repeat(MAX_SELECTION_BYTES + 1)
            } else {
                text.into()
            };
            let range = Range::new(Position::new(0, 0), Position::new(0, text.len() as u32));
            server
                .documents
                .insert(uri.clone(), Document::new(1, 1, language.into(), text));
            markdown_actions(&mut server, &uri, range);
            server.expire(&connection).unwrap();
            assert!(server.selection.is_none());
            assert!(server.translations.is_empty());
            assert!(fixture.state.lock().unwrap().records.is_empty());
            assert!(client.receiver.try_recv().is_err());
            if mode == "signed-out" {
                assert!(auth.store.snapshot().unwrap().is_none());
            }
            drop(completions);
            drop(server);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn automatic_selection_uses_document_code_context_and_sends_only_selected_bytes() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth,
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///context.md".parse().unwrap();
        for (index, (text, selected, prose)) in [
            (
                "```rust\nlet retry_count = 3;\n```\n",
                "let retry_count = 3;",
                false,
            ),
            ("```rust\nlet retry_count = 3;\n```\n", "retry_count", false),
            (
                "```rust\nlet retry_count = 3;\n```\n",
                "```rust\nlet retry_count = 3;\n```",
                false,
            ),
            ("Keep `retry_count` unchanged.", "`retry_count`", false),
            ("Keep `retry_count` unchanged.", "retry_count", false),
            (
                "Read [the guide](https://example.test/path).",
                "https://example.test/path",
                false,
            ),
            (
                "Read [the guide](https://example.test/path).",
                "example.test",
                false,
            ),
            (
                "Read <https://example.test/path>.",
                "https://example.test/path",
                false,
            ),
            (
                "    let retry_count = 3;\n",
                "    let retry_count = 3;",
                false,
            ),
            ("    let retry_count = 3;\n", "retry_count", false),
            ("<div>\nhidden text\n</div>\n", "hidden text", false),
            (
                "Do not send the entire document. First sentence. Keep this too.",
                "First sentence.",
                true,
            ),
            (
                "Read [the guide](https://example.test/path).",
                "the guide",
                true,
            ),
            (
                "Outside. Keep `retry_count` unchanged. Outside.",
                "Keep `retry_count` unchanged.",
                true,
            ),
            (
                "Outside. Read [the guide](https://example.test/path). Outside.",
                "Read [the guide](https://example.test/path).",
                true,
            ),
            (
                "Outside.\n\n```rust\nlet retry_count = 3;\n```\n\nFirst sentence.\nOutside.",
                "let retry_count = 3;\n```\n\nFirst sentence.",
                false,
            ),
            (
                "Outside.\n\n```rust\nlet retry_count = 3;\n```\n\nFirst sentence.\nOutside.",
                "```rust\nlet retry_count = 3;\n```\n\nFirst sentence.",
                true,
            ),
            (
                "Keep `retry_count` unchanged.",
                "retry_count` unchanged.",
                false,
            ),
            (
                "Read [the guide](https://example.test/path). First sentence.",
                "example.test/path). First sentence.",
                false,
            ),
            (
                "First sentence. Keep `retry_count` unchanged.",
                "First sentence. Keep `retry",
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let start = text.find(selected).unwrap();
            let position = |offset: usize| {
                let prefix = &text[..offset];
                Position::new(
                    prefix.bytes().filter(|&byte| byte == b'\n').count() as u32,
                    prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32,
                )
            };
            let range = Range::new(position(start), position(start + selected.len()));
            server.documents.insert(
                uri.clone(),
                Document::new(1, index as u64, "markdown".into(), text.into()),
            );
            let before = fixture.state.lock().unwrap().records.len();
            markdown_actions(&mut server, &uri, range);
            if prose {
                assert!(server.selection.is_some(), "{selected:?}");
                server.selection.as_mut().unwrap().start = Some(Instant::now());
                server.expire(&connection).unwrap();
                server
                    .finish(&connection, next(&completions).await)
                    .unwrap();
                let state = fixture.state.lock().unwrap();
                assert_eq!(state.records.len(), before + 1, "{selected:?}");
                let body: Value =
                    serde_json::from_str(&state.records.last().unwrap().body).unwrap();
                let input: Value =
                    serde_json::from_str(body["input"][0]["content"].as_str().unwrap()).unwrap();
                let sent: String = input["segments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|segment| segment["text"].as_str().unwrap())
                    .collect();
                assert_eq!(sent, selected);
                for literal in ["retry_count", "https://example.test/path"] {
                    if selected.contains(literal) {
                        assert!(input["segments"].as_array().unwrap().iter().any(|segment| {
                            segment["protected"] == true
                                && segment["text"].as_str().unwrap().contains(literal)
                        }));
                        assert!(
                            server.documents[&uri]
                                .preview
                                .as_ref()
                                .unwrap()
                                .translation
                                .contains(literal)
                        );
                    }
                }
            } else {
                assert!(server.selection.is_none(), "{selected:?}");
                server.expire(&connection).unwrap();
                assert!(server.translations.is_empty());
                assert_eq!(fixture.state.lock().unwrap().records.len(), before);
            }
            assert_eq!(server.documents[&uri].text.as_deref(), Some(text));
            assert!(client.receiver.try_recv().is_err());
        }
        drop(completions);
        drop(server);
    }

    async fn protocol_message(client: &Connection) -> Message {
        let receiver = client.receiver.clone();
        tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(3)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn live_loop_starts_after_stability_and_settings_dismissal_preserves_background() {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().stream_tail_delay = Duration::from_millis(800);
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let before = auth.store.snapshot().unwrap().unwrap().revision;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let server = thread::spawn(move || {
            let ui = Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            );
            serve_initialized(
                &connection,
                MockOptions::default(),
                Settings::from_options(Some(json!({"selection_debounce_ms":700}))).unwrap(),
                ui,
                provider,
            )
            .unwrap();
        });
        let uri: Uri = "file:///loop.md".parse().unwrap();
        client
            .sender
            .send(
                lsp_server::Notification::new(
                    "textDocument/didOpen".into(),
                    json!({
                        "textDocument": {
                            "uri": uri, "languageId": "markdown", "version": 1,
                            "text": "First sentence."
                        }
                    }),
                )
                .into(),
            )
            .unwrap();
        let params = json!({
            "textDocument": {"uri": uri},
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 15}},
            "context": {"diagnostics": []}
        });
        let started = Instant::now();
        client
            .sender
            .send(Request::new(1.into(), CodeActionRequest::METHOD.into(), params.clone()).into())
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected candidates");
        };
        assert_eq!(
            response.response_result.unwrap().as_array().unwrap().len(),
            2
        );
        assert!(started.elapsed() < Duration::from_millis(200));
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(fixture.state.lock().unwrap().records.is_empty());
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if fixture
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .iter()
                    .any(|record| record.path == "/v1/responses")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(690));
        let picker = Instant::now();
        client
            .sender
            .send(
                Request::new(
                    2.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({"command":SETTINGS_COMMAND}),
                )
                .into(),
            )
            .unwrap();
        let Message::Request(request) = protocol_message(&client).await else {
            panic!("Expected picker");
        };
        assert_eq!(request.method, "window/showMessageRequest");
        assert!(
            picker.elapsed() < Duration::from_millis(400),
            "Picker must not wait for translation EOF"
        );
        assert_eq!(
            request.params["actions"],
            json!([
                {"title": "Change ChatGPT account"},
                {"title": "Sign out of ChatGPT"},
                {"title": "ChatGPT Usage"}
            ])
        );
        assert_eq!(request.params["message"], "");
        client
            .sender
            .send(
                Response {
                    id: request.id,
                    response_result: Ok(Value::Null),
                }
                .into(),
            )
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected dismissal");
        };
        assert_eq!(response.id, 2.into());
        assert!(response.response_result.unwrap().is_null());
        client
            .sender
            .send(
                Request::new(
                    3.into(),
                    HoverRequest::METHOD.into(),
                    json!({"textDocument":{"uri":uri},"position":{"line":0,"character":4}}),
                )
                .into(),
            )
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected completed Hover");
        };
        assert_eq!(response.id, 3.into());
        assert!(
            response.response_result.unwrap()["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("訳First sentence.")
        );
        assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
        client
            .sender
            .send(Request::new(4.into(), CodeActionRequest::METHOD.into(), params).into())
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected cached candidates");
        };
        assert_eq!(response.id, 4.into());
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|record| record.path == "/v1/responses")
                .count(),
            1
        );
        assert!(client.receiver.try_recv().is_err());
        client
            .sender
            .send(Request::new(99.into(), Shutdown::METHOD.into(), Value::Null).into())
            .unwrap();
        client
            .sender
            .send(lsp_server::Notification::new(Exit::METHOD.into(), Value::Null).into())
            .unwrap();
        let Message::Response(response) = protocol_message(&client).await else {
            panic!("Expected shutdown");
        };
        assert_eq!(response.id, 99.into());
        tokio::task::spawn_blocking(move || server.join().unwrap())
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_picker_filters_states_and_closing_it_has_no_effects() {
        for mode in ["signed-out", "disabled", "ready", "unavailable"] {
            let fixture = Fixture::new().await;
            let auth = fixture.auth();
            if mode != "signed-out" {
                let id = fixture.connected(&auth, 3600).await;
                if mode == "disabled" {
                    let mut locked = auth.store.lock(false).await.unwrap().unwrap();
                    locked
                        .state
                        .registrations
                        .get_mut(&id)
                        .unwrap()
                        .credentials
                        .as_mut()
                        .unwrap()
                        .scopes
                        .clear();
                    locked.save().unwrap();
                }
            }
            let before = auth
                .store
                .snapshot()
                .unwrap()
                .map(|snapshot| snapshot.revision);
            let locked = if mode == "unavailable" {
                auth.store.lock(false).await.unwrap()
            } else {
                None
            };
            let provider = ChatGptProvider::synthetic(
                auth.clone(),
                format!("{}/v1", fixture.base),
                "synthetic-model".into(),
            );
            let (connection, client) = Connection::memory();
            let (finished, completions) = bounded(MAX_JOBS);
            let mut server = Server::configured(
                MockOptions::default(),
                finished,
                provider,
                Settings::default(),
                Ui::new(
                    connection.sender.clone(),
                    UiCapabilities {
                        external_documents: true,
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
            let uri: Uri = "file:///picker.rs".parse().unwrap();
            server.documents.insert(
                uri.clone(),
                Document::new(1, 1, "rust".into(), "// sentence".into()),
            );
            server.documents.get_mut(&uri).unwrap().preview = Some(Preview {
                context: Context::Mock,
                range: Range::new(Position::new(0, 0), Position::new(0, 11)),
                translation: "cached".into(),
                is_comment: true,
            });
            let state = server.provider.auth_ui_state();
            server
                .execute_command(
                    &connection,
                    7.into(),
                    parse(json!({"command":SETTINGS_COMMAND})).unwrap(),
                )
                .unwrap();
            let Message::Request(request) = protocol_message(&client).await else {
                panic!("Expected management picker");
            };
            assert_eq!(request.method, "window/showMessageRequest");
            let expected: Vec<_> = management_commands(state)
                .iter()
                .map(|command| json!({"title":command.title}))
                .collect();
            assert_eq!(request.params["actions"], json!(expected));
            server.ui.response(Response {
                id: request.id,
                response_result: Ok(Value::Null),
            });
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            let Message::Response(response) = protocol_message(&client).await else {
                panic!("Expected silent dismissal");
            };
            assert_eq!(response.id, 7.into());
            assert!(response.response_result.unwrap().is_null());
            assert!(client.receiver.try_recv().is_err());
            assert_eq!(
                server.documents[&uri].preview.as_ref().unwrap().translation,
                "cached"
            );
            drop(locked);
            assert_eq!(
                auth.store
                    .snapshot()
                    .unwrap()
                    .map(|snapshot| snapshot.revision),
                before
            );
            assert!(fixture.state.lock().unwrap().records.is_empty());
            drop(completions);
            drop(server);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn account_picker_dispatches_only_the_selected_command() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        fixture.connected(&auth, 3600).await;
        let before = auth.store.snapshot().unwrap().unwrap().revision;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let commands = serde_json::to_value(capabilities(true)).unwrap()["capabilities"]
            ["executeCommandProvider"]["commands"]
            .clone();
        for command in std::iter::once(COMMAND)
            .chain(ADMIN_COMMANDS.iter().map(|(command, _)| *command))
            .chain(std::iter::once(SETTINGS_COMMAND))
        {
            assert!(commands.as_array().unwrap().contains(&json!(command)));
        }
        server
            .execute_command(
                &connection,
                1.into(),
                parse(json!({"command":SETTINGS_COMMAND})).unwrap(),
            )
            .unwrap();
        let Message::Request(request) = protocol_message(&client).await else {
            panic!("Expected picker");
        };
        assert_eq!(request.params["message"], "");
        assert_eq!(
            request.params["actions"],
            json!([
                {"title": "Change ChatGPT account"},
                {"title": "Sign out of ChatGPT"},
                {"title": "ChatGPT Usage"}
            ])
        );
        server.ui.response(Response {
            id: request.id,
            response_result: Ok(json!({"title":"ChatGPT Usage"})),
        });
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert_eq!(
            server.administrations[&1.into()].command,
            "zed-code-translation.usage"
        );
        let Message::Request(request) = protocol_message(&client).await else {
            panic!("Expected chosen usage UI");
        };
        assert_eq!(request.method, "window/showDocument");
        assert_eq!(request.params["uri"], "https://chatgpt.com/settings/usage");
        server.ui.response(Response {
            id: request.id,
            response_result: Ok(json!({"success":true})),
        });
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        command_notice(&client, 1, 3).unwrap();
        assert!(fixture.state.lock().unwrap().records.is_empty());
        assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
        drop(completions);
        drop(server);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn candidates_follow_auth_transitions_without_http_or_store_writes() {
        use crate::providers::chatgpt::{
            auth::Auth,
            auth_store::{AuthStore, DIRECT_SCOPE},
            http::Http,
        };
        let fixture = Fixture::new().await;
        let directory =
            std::env::temp_dir().join(format!("zed-actions-synthetic-{}", uuid::Uuid::new_v4()));
        let auth = Auth::synthetic(
            AuthStore::synthetic(directory.clone()),
            Http::synthetic_revocation(&fixture.base),
            fixture.base.clone(),
        );
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///state-actions.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
        );
        let actions = |server: &Server<ChatGptProvider>| -> Vec<(String, String)> {
            management_commands(server.provider.auth_ui_state())
                .into_iter()
                .map(|command| (command.command, command.title))
                .collect()
        };
        let connect = (
            "zed-code-translation.connect".into(),
            "Continue with ChatGPT / Choose account".into(),
        );
        let usage = ("zed-code-translation.usage".into(), "ChatGPT Usage".into());
        let signed_out = vec![connect.clone(), usage.clone()];
        let unknown = vec![
            (
                "zed-code-translation.connect".into(),
                "Check ChatGPT connection…".into(),
            ),
            usage.clone(),
        ];
        assert_eq!(actions(&server), signed_out);
        assert!(
            !directory.exists(),
            "Candidate discovery must not create auth/lock files"
        );
        let id = fixture.connected(&auth, 3600).await;
        let ready = vec![
            (
                "zed-code-translation.connect".into(),
                "Change ChatGPT account".into(),
            ),
            (
                "zed-code-translation.sign-out".into(),
                "Sign out of ChatGPT".into(),
            ),
            usage.clone(),
        ];
        let before = auth.store.snapshot().unwrap().unwrap().revision;
        assert_eq!(actions(&server), ready);
        assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
        // No token expiry probing or refresh happens in discovery.
        {
            let mut locked = auth.store.lock(false).await.unwrap().unwrap();
            let credentials = locked
                .state
                .registrations
                .get_mut(&id)
                .unwrap()
                .credentials
                .as_mut()
                .unwrap();
            credentials.expires_in = 0;
            locked.save().unwrap();
        }
        assert_eq!(actions(&server), ready);
        {
            let mut locked = auth.store.lock(false).await.unwrap().unwrap();
            locked
                .state
                .registrations
                .get_mut(&id)
                .unwrap()
                .credentials
                .as_mut()
                .unwrap()
                .scopes
                .retain(|scope| scope != DIRECT_SCOPE);
            locked.save().unwrap();
        }
        assert_eq!(
            actions(&server),
            vec![
                connect.clone(),
                (
                    "zed-code-translation.enable".into(),
                    "Enable ChatGPT plan use".into()
                ),
                (
                    "zed-code-translation.sign-out".into(),
                    "Sign out of ChatGPT".into()
                ),
                usage.clone()
            ]
        );
        assert!(
            auth.key()
                .unwrap_err()
                .message
                .contains("Enable ChatGPT plan use")
        );
        {
            let mut locked = auth.store.lock(false).await.unwrap().unwrap();
            // While another process owns the store, use a non-assertive recovery title.
            assert_eq!(actions(&server), unknown);
            let before = locked.revision().unwrap();
            server
                .request(
                    &connection,
                    Request::new(
                        9.into(),
                        ExecuteCommand::METHOD.into(),
                        json!({"command":"zed-code-translation.connect"}),
                    ),
                )
                .unwrap();
            assert!(
                server
                    .code_actions(
                        parse(json!({
                            "textDocument": {"uri": uri},
                            "range": {
                                "start": {"line": 0, "character": 0},
                                "end": {"line": 0, "character": 18}
                            },
                            "context": {"diagnostics": []}
                        }))
                        .unwrap()
                    )
                    .unwrap()
                    .is_empty()
            );
            server
                .finish(&connection, next(&completions).await)
                .unwrap();
            assert_eq!(
                command_notice(&client, 9, 1).unwrap_err().code,
                ErrorCode::RequestFailed as i32
            );
            assert_eq!(locked.revision().unwrap(), before);
            assert!(fixture.state.lock().unwrap().records.is_empty());
            locked.state.registrations.get_mut(&id).unwrap().credentials = None;
            locked.save().unwrap();
        }
        assert_eq!(actions(&server), signed_out);
        let error = auth.key().unwrap_err();
        assert!(
            error
                .message
                .contains("Continue with ChatGPT / Choose account")
        );
        assert!(!error.message.contains("Enable"));
        fixture.connected(&auth, 3600).await;
        assert_eq!(actions(&server), ready);
        // An invalid local snapshot also retains the explicit recovery entry.
        let path = directory.join("auth.json");
        let saved = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(actions(&server), unknown);
        server
            .request(
                &connection,
                Request::new(
                    10.into(),
                    ExecuteCommand::METHOD.into(),
                    json!({"command":"zed-code-translation.connect"}),
                ),
            )
            .unwrap();
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        let error = command_notice(&client, 10, 1).unwrap_err();
        assert_eq!(error.code, ErrorCode::RequestFailed as i32);
        assert!(!error.message.contains("synthetic-access"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{",
            "Explicit failed recovery must not rewrite the store"
        );
        std::fs::write(&path, saved).unwrap();
        assert_eq!(actions(&server), ready);
        assert!(fixture.state.lock().unwrap().records.is_empty());
        let mut files: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(
            files,
            vec![
                std::ffi::OsString::from("auth.json"),
                std::ffi::OsString::from("auth.lock")
            ]
        );
        drop(completions);
        drop(server);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn candidates_are_local_and_explicit_commands_use_standard_lsp_ui() {
        let fixture = Fixture::new().await;
        let auth = fixture.auth();
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let (connection, client) = Connection::memory();
        let (finished, completions) = bounded(MAX_JOBS);
        let mut server = Server::configured(
            MockOptions::default(),
            finished,
            provider,
            Settings::default(),
            Ui::new(
                connection.sender.clone(),
                UiCapabilities {
                    external_documents: true,
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        let uri: Uri = "file:///actions.rs".parse().unwrap();
        server.documents.insert(
            uri.clone(),
            Document::new(1, 1, "rust".into(), "// Translate this.\n".into()),
        );
        let actions = server.code_actions(parse(json!({"textDocument":{"uri":uri},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"context":{"diagnostics":[]}})).unwrap()).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "ChatGPT account…");
        assert_eq!(
            actions[0].command.as_ref().unwrap().command,
            SETTINGS_COMMAND
        );
        assert!(fixture.state.lock().unwrap().records.is_empty());
        assert!(auth.store.snapshot().unwrap().is_none());
        server
            .execute_command(
                &connection,
                RequestId::from(1),
                parse(json!({"command":"zed-code-translation.connect"})).unwrap(),
            )
            .unwrap();
        let receiver = client.receiver.clone();
        let Message::Request(request) = tokio::task::spawn_blocking(move || {
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        })
        .await
        .unwrap() else {
            panic!("Expected account picker");
        };
        assert_eq!(request.method, "window/showMessageRequest");
        server
            .ui
            .response(Response::new_ok(request.id, Value::Null));
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        assert!(auth.store.snapshot().unwrap().is_none());
        command_notice(&client, 1, 3).unwrap();
        fixture.connected(&auth, 3600).await;
        let before = auth.store.snapshot().unwrap().unwrap().revision;
        let commands = capabilities(true)
            .capabilities
            .execute_command_provider
            .unwrap()
            .commands;
        assert!(commands.iter().any(|command| command == SETTINGS_COMMAND));
        assert!(
            !commands
                .iter()
                .any(|command| command == "zed-code-translation.models")
        );
        let error = server
            .execute_command(
                &connection,
                RequestId::from(2),
                parse(json!({"command":"zed-code-translation.models"})).unwrap(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams as i32);
        assert!(client.receiver.try_recv().is_err());
        assert!(server.administrations.is_empty());
        assert!(fixture.state.lock().unwrap().records.is_empty());
        assert_eq!(auth.store.snapshot().unwrap().unwrap().revision, before);
        server
            .execute_command(
                &connection,
                RequestId::from(3),
                parse(json!({"command":"zed-code-translation.usage"})).unwrap(),
            )
            .unwrap();
        let receiver = client.receiver.clone();
        let Message::Request(request) = tokio::task::spawn_blocking(move || {
            receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        })
        .await
        .unwrap() else {
            panic!("Expected usage URL");
        };
        assert_eq!(request.method, "window/showDocument");
        assert_eq!(request.params["uri"], "https://chatgpt.com/settings/usage");
        server
            .ui
            .response(Response::new_ok(request.id, json!({"success":true})));
        server
            .finish(&connection, next(&completions).await)
            .unwrap();
        command_notice(&client, 3, 3).unwrap();
    }
}
