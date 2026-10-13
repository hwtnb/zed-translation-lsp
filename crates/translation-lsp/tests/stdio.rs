use std::{
    collections::HashMap,
    io::{BufReader, Read},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use lsp_server::{Message, Notification, Request, RequestId, Response, ResponseError};
use serde_json::{Value, json};

// A checkout may give the sample files CRLF line endings. The tests find text
// in them by LF, so each sample is read with LF whatever the checkout wrote.
macro_rules! sample {
    ($name:literal) => {{
        const SOURCE: &str = include_str!(concat!("../../../samples/", $name));
        const COPY: ([u8; SOURCE.len()], usize) = with_line_feeds(SOURCE);
        const BYTES: &[u8] = COPY.0.split_at(COPY.1).0;
        match std::str::from_utf8(BYTES) {
            Ok(text) => text,
            Err(_) => panic!("A sample is not UTF-8 after its carriage returns are removed"),
        }
    }};
}

const fn with_line_feeds<const N: usize>(text: &str) -> ([u8; N], usize) {
    let source = text.as_bytes();
    let mut copy = [0; N];
    let (mut read, mut written) = (0, 0);
    while read < source.len() {
        let before_line_feed = read + 1 < source.len() && source[read + 1] == b'\n';
        if !(source[read] == b'\r' && before_line_feed) {
            copy[written] = source[read];
            written += 1;
        }
        read += 1;
    }
    (copy, written)
}

const COMMENT_DOCUMENT: &str = sample!("comment.rs");
const MARKDOWN_DOCUMENT: &str = sample!("paragraph.md");
const COMMENT: &str = "// Retry only if the remote service\n    // reports a temporary failure.";
const PARAGRAPH: &str =
    "The `retry_count` value is preserved.\nRetry only after a temporary failure. 🚀";
const COMMENT_JA: &str = "リモートサービスが一時的な障害を報告した場合に限り、再試行する。";
const PARAGRAPH_JA: &str =
    "`retry_count` の値を維持する。\n一時的な障害が発生した場合に限り再試行する。🚀";
const TIMEOUT: Duration = Duration::from_secs(8);
const READY: &str = "Translation ready. Hover over the selected text to view it.";

type Reply = Result<Value, ResponseError>;

// Framing is read and written by the same published SDK used by the server.
// Server requests are held until each test supplies the client's response.
struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Message, String>>,
    reader: Option<thread::JoinHandle<()>>,
    stderr: Option<thread::JoinHandle<String>>,
    replies: HashMap<RequestId, Reply>,
    notifications: Vec<Value>,
    next_id: i32,
    activity_capabilities: (bool, bool),
    creates: Vec<Request>,
    documents: Vec<Request>,
    progress: Vec<Value>,
    statuses: Vec<Value>,
}

impl Client {
    fn new(args: &[&str]) -> Self {
        Self::with_activity(args, false, false)
    }

    fn with_activity(args: &[&str], progress: bool, status: bool) -> Self {
        Self::configured(args, progress, status, Value::Null)
    }

    fn spawn(args: &[&str], progress: bool, status: bool) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_translation-lsp"))
            .args(args)
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut stderr = child.stderr.take().unwrap();
        let (sender, messages) = mpsc::channel();
        let reader = thread::spawn(move || {
            loop {
                match Message::read(&mut stdout) {
                    Ok(Some(message)) => {
                        if sender.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).unwrap();
            text
        });
        Self {
            child,
            stdin,
            messages,
            reader: Some(reader),
            stderr: Some(stderr),
            replies: HashMap::new(),
            notifications: Vec::new(),
            next_id: 0,
            activity_capabilities: (progress, status),
            creates: Vec::new(),
            documents: Vec::new(),
            progress: Vec::new(),
            statuses: Vec::new(),
        }
    }

    fn configured(args: &[&str], progress: bool, status: bool, options: Value) -> Self {
        Self::configured_documents(args, progress, status, options, false)
    }

    fn configured_documents(
        args: &[&str],
        progress: bool,
        status: bool,
        options: Value,
        documents: bool,
    ) -> Self {
        let mut client = Self::spawn(args, progress, status);
        let initialized = client
            .call(
                "initialize",
                json!({
                    "initializationOptions": options,
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {
                        "general": { "positionEncodings": ["utf-16"] },
                        "window": {"workDoneProgress": progress, "showDocument": {"support": documents}},
                        "experimental": {"serverStatusNotification": status},
                        "textDocument": {
                            "hover": { "contentFormat": ["markdown"] },
                            "codeAction": { "codeActionLiteralSupport": {
                                "codeActionKind": { "valueSet": ["translation.preview"] }
                            }}
                        }
                    }
                }),
            )
            .unwrap();
        assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-16");
        assert_eq!(initialized["capabilities"]["textDocumentSync"]["change"], 2);
        assert_eq!(
            initialized["capabilities"]["executeCommandProvider"]["commands"],
            json!([
                "translation-lsp.preview",
                "translation-lsp.retry",
                "translation-lsp.open-translation",
                "translation-lsp.cancel-range",
                "translation-lsp.cancel-all"
            ])
        );
        client.notify("initialized", json!({}));
        client
    }

    fn send(&mut self, message: Message) {
        message.write(&mut self.stdin).unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(Notification::new(method.into(), params).into());
    }

    fn request(&mut self, method: &str, params: Value) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.request_with_id(id.clone(), method, params);
        id
    }

    fn request_with_id(&mut self, id: RequestId, method: &str, params: Value) {
        self.send(Request::new(id, method.into(), params).into());
    }

    fn receive(&mut self, id: RequestId, timeout: Duration) -> Reply {
        let start = Instant::now();
        loop {
            if let Some(reply) = self.replies.remove(&id) {
                return reply;
            }
            let remaining = timeout
                .checked_sub(start.elapsed())
                .expect("LSP response timed out");
            let message = self
                .messages
                .recv_timeout(remaining)
                .expect("LSP response timed out or process exited")
                .expect("Invalid stdout framing");
            self.record(message);
        }
    }

    fn record(&mut self, message: Message) {
        match message {
            Message::Response(response) => {
                // A server started here may run only the mock provider. The
                // others read the credentials of whoever runs the tests, and
                // send what a test translates to the real API.
                if let Ok(result) = &response.response_result
                    && let Some(name) = result.pointer("/serverInfo/name")
                {
                    assert_eq!(
                        name, "Translation LSP (Mock)",
                        "A test started a real provider"
                    );
                }
                assert!(
                    self.replies
                        .insert(response.id, response.response_result)
                        .is_none(),
                    "Duplicate response"
                );
            }
            Message::Request(request) if request.method == "window/showDocument" => {
                self.documents.push(request);
            }
            Message::Request(request)
                if self.activity_capabilities.0
                    && request.method == "window/workDoneProgress/create" =>
            {
                self.creates.push(request);
            }
            Message::Notification(notification)
                if self.activity_capabilities.0 && notification.method == "$/progress" =>
            {
                self.progress.push(notification.params);
            }
            Message::Notification(notification)
                if self.activity_capabilities.1
                    && notification.method == "experimental/serverStatus" =>
            {
                self.statuses.push(notification.params);
            }
            Message::Notification(notification) if notification.method == "window/showMessage" => {
                assert!(matches!(notification.params["type"].as_u64(), Some(1..=3)));
                assert!(notification.params["message"].is_string());
                assert_eq!(notification.params.as_object().unwrap().len(), 2);
                self.notifications.push(notification.params);
            }
            other => panic!("Unexpected server message: {other:?}"),
        }
    }

    fn pump(&mut self) {
        let message = self.messages.recv_timeout(TIMEOUT).unwrap().unwrap();
        self.record(message);
    }

    fn notice(&mut self) -> Value {
        while self.notifications.is_empty() {
            self.pump();
        }
        self.notifications.remove(0)
    }

    fn create_progress(&mut self) -> Request {
        while self.creates.is_empty() {
            self.pump();
        }
        self.creates.remove(0)
    }

    fn progress_kind(&mut self, kind: &str) -> Value {
        loop {
            if let Some(index) = self
                .progress
                .iter()
                .position(|params| params["value"]["kind"] == kind)
            {
                return self.progress.remove(index);
            }
            self.pump();
        }
    }

    fn status(&mut self) -> Value {
        while self.statuses.is_empty() {
            self.pump();
        }
        self.statuses.remove(0)
    }

    fn call(&mut self, method: &str, params: Value) -> Reply {
        let id = self.request(method, params);
        self.receive(id, TIMEOUT)
    }

    fn open(&mut self, uri: &str, language: &str, text: &str, version: i32) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri, "languageId": language, "version": version, "text": text
            }}),
        );
    }

    fn change(&mut self, uri: &str, version: i32, changes: Value) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri, "version": version }, "contentChanges": changes
            }),
        );
    }

    fn close(&mut self, uri: &str) {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri }}),
        );
    }

    fn actions(&mut self, uri: &str, range: Value) -> Value {
        self.call(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": [] }
            }),
        )
        .unwrap()
    }

    fn action(&mut self, uri: &str, range: Value) -> Value {
        let actions = self.actions(uri, range);
        let action = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["command"]["command"] == "translation-lsp.preview")
            .unwrap();
        assert!(action.get("edit").is_none());
        assert_eq!(action["title"], "Translate selection to ja");
        assert_eq!(action["command"]["title"], action["title"]);
        action["command"].clone()
    }

    fn hover(&mut self, uri: &str, position: Value) -> Value {
        self.call(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri }, "position": position
            }),
        )
        .unwrap()
    }

    fn request_hover(&mut self, uri: &str, position: Value) -> RequestId {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri }, "position": position
            }),
        )
    }

    fn execute(&mut self, action: Value) -> RequestId {
        self.request("workspace/executeCommand", action)
    }

    fn finish(&mut self, expect_clean_stderr: bool) -> String {
        assert!(self.call("shutdown", Value::Null).unwrap().is_null());
        self.finish_exit(expect_clean_stderr)
    }

    fn finish_exit(&mut self, expect_clean_stderr: bool) -> String {
        self.notify("exit", Value::Null);
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "Shutdown failed to exit promptly"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert!(status.success());
        self.reader.take().unwrap().join().unwrap();
        assert!(
            self.messages.try_iter().next().is_none(),
            "Unconsumed or invalid protocol output"
        );
        assert!(self.replies.is_empty(), "Unconsumed responses");
        assert!(self.documents.is_empty(), "Unexpected document opening");
        let stderr = self.stderr.take().unwrap().join().unwrap();
        if expect_clean_stderr {
            assert!(stderr.is_empty(), "Unexpected stderr: {stderr}");
        }
        stderr
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(stderr) = self.stderr.take() {
            let _ = stderr.join();
        }
    }
}

fn position(text: &str) -> Value {
    json!({
        "line": text.bytes().filter(|&byte| byte == b'\n').count(),
        "character": text.rsplit('\n').next().unwrap().encode_utf16().count()
    })
}

fn range_for(document: &str, selected: &str) -> Value {
    let start = document.find(selected).unwrap();
    json!({ "start": position(&document[..start]), "end": position(&document[..start + selected.len()]) })
}

fn assert_preview(hover: &Value, original: &str, translation: &str, range: &Value) {
    assert_eq!(hover["range"], *range);
    assert_eq!(hover["contents"]["kind"], "markdown");
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(
        text.starts_with("> **Translation LSP**\n>\n> Mock translation (fixed sample)\n\n---\n\n")
    );
    assert!(!text.contains("**Original**"));
    assert!(
        !text.contains(original),
        "Original must stay in the editor: {text}"
    );
    assert!(text.contains(&format!("```text\n{translation}\n```")));
}

fn assert_error(reply: Reply, code: i32) {
    assert_eq!(reply.unwrap_err().code, code);
}

// The editor starts the server with `--stdio` or with no argument at all.
#[test]
fn server_starts_with_or_without_the_stdio_argument() {
    for arguments in [&[][..], &["--stdio"], &["--stdio", "--mock-delay-ms", "1"]] {
        let mut client = Client::new(arguments);
        client.open("file:///started.md", "markdown", MARKDOWN_DOCUMENT, 1);
        let hover = client.call(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": "file:///started.md"},
                "position": {"line": 0, "character": 0}
            }),
        );
        assert!(hover.unwrap().is_null());
        client.finish(true);
    }
}

#[test]
fn initialization_rejects_invalid_public_settings_before_mock_override() {
    for options in [
        json!({"hover_timeout_ms":999}),
        json!({"selection_timeout_ms":120001}),
        json!({"document_timeout_ms":600001}),
        json!({"selection_debounce_ms":99}),
        json!({"features":{"automatic_selection":"false"}}),
        json!({"markdown_auto_translate":false}),
        json!({"hover_timeout_ms":1.5}),
        json!({"unknown_setting":true}),
        json!({"translation_limits":{"input_kib":{"document":16385}}}),
        json!({"translation_limits":{"work":{"admitted":65}}}),
        json!({"translation_limits":{"output_kib":16384,"work":{"concurrent":8}}}),
    ] {
        let mut client = Client::spawn(&["--mock-timeout-ms", "80"], false, false);
        let error = client
            .call(
                "initialize",
                json!({
                    "capabilities":{}, "initializationOptions":options
                }),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert!(error.message.is_ascii());
        client.notify("exit", Value::Null);
        assert!(!client.child.wait().unwrap().success());
        client.reader.take().unwrap().join().unwrap();
        assert!(client.messages.try_iter().next().is_none());
        assert!(
            client
                .stderr
                .take()
                .unwrap()
                .join()
                .unwrap()
                .contains("translation")
        );
    }
}

#[test]
fn anthropic_settings_errors_name_the_setting_at_initialization() {
    for (provider, message) in [
        (
            json!({"name": "anthropic", "effort": "minimal"}),
            "provider.effort must be one of low, medium, high, xhigh, max",
        ),
        (
            json!({"name": "anthropic", "effort": "max", "thinking": false}),
            "provider.thinking must not be false when provider.effort is max",
        ),
        (
            json!({"name": "anthropic", "model": " "}),
            "provider.model must be nonempty",
        ),
        (
            json!({"name": "anthropic", "document": {"effort": "high"}}),
            "provider.document.model is required",
        ),
    ] {
        let mut client = Client::spawn(&[], false, false);
        let error = client
            .call(
                "initialize",
                json!({"capabilities": {}, "initializationOptions": {"provider": provider}}),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert!(error.message.starts_with(message), "{}", error.message);
        client.notify("exit", Value::Null);
        assert!(!client.child.wait().unwrap().success());
    }
}

#[test]
fn settings_written_as_arrays_are_rejected_at_initialization() {
    for options in [
        json!([]),
        json!({"translation_limits": {"work": []}}),
        json!({"provider": ["anthropic"]}),
        json!({"provider": {"name": "chatgpt", "model": "m", "document": ["synthetic-tab", "low"]}}),
    ] {
        let mut client = Client::spawn(&[], false, false);
        let error = client
            .call(
                "initialize",
                json!({"capabilities": {}, "initializationOptions": options}),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(
            error.message,
            "Invalid translation settings: check field names and value types"
        );
        client.notify("exit", Value::Null);
        assert!(!client.child.wait().unwrap().success());
    }
}

#[test]
fn public_deadlines_separate_hover_waiting_from_retained_execution() {
    let mut client = Client::configured(
        &["--mock-delay-ms", "1400"],
        false,
        false,
        json!({"hover_timeout_ms":1000,"selection_timeout_ms":1800,"features":{"automatic_selection":false}}),
    );
    let uri = "file:///configured-deadline.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let action = client.action(uri, range.clone());
    let hover = client.request_hover(uri, range["start"].clone());
    let command = client.execute(action);
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    assert_error(client.receive(hover, TIMEOUT), -32803);
    let notice = client.notice();
    assert_eq!(notice["type"], 3);
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    client.finish(true);
    let mut client = Client::configured(
        &["--mock-delay-ms", "400", "--mock-timeout-ms", "80"],
        false,
        false,
        json!({"hover_timeout_ms":1000,"selection_timeout_ms":1800}),
    );
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let action = client.action(uri, range.clone());
    let command = client.execute(action);
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    assert_eq!(client.notice()["type"], 1);
    let failed = client.hover(uri, range["start"].clone());
    assert!(
        failed["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("translation failed")
    );
    client.finish(true);
}

#[test]
fn progress_registration_does_not_block_hover_or_coalesced_waiter_cancellation() {
    let mut client = Client::with_activity(&["--mock-delay-ms", "700"], true, true);
    let uri = "file:///progress.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let position = range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone();
    let first = client.request_hover(uri, position.clone());
    let create = client.create_progress();
    assert!(client.progress.is_empty());
    let sync = Instant::now();
    assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
    assert!(sync.elapsed() < Duration::from_millis(400));
    let second = client.request_hover(uri, position);
    client.notify("$/cancelRequest", json!({"id":first}));
    assert_error(client.receive(first, TIMEOUT), -32800);
    client.send(Response::new_ok(create.id, Value::Null).into());
    let begin = client.progress_kind("begin");
    assert_eq!(begin["token"], create.params["token"]);
    assert_eq!(
        begin["value"],
        json!({
            "kind":"begin", "title":"Translation LSP", "cancellable":true,
            "message":"Translating: 1, queued: 0"
        })
    );
    assert_preview(
        &client.receive(second, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range_for(COMMENT_DOCUMENT, COMMENT),
    );
    assert_eq!(client.progress_kind("end")["token"], create.params["token"]);
    assert!(
        client.creates.is_empty(),
        "Coalesced owners count as one job"
    );
    assert!(client.statuses.is_empty());
    assert!(
        client.notifications.is_empty(),
        "Automatic Hover does not add INFO"
    );
    client.finish(true);
}

#[test]
fn delayed_progress_ack_and_old_cancel_do_not_revive_edited_or_closed_work() {
    let mut client = Client::with_activity(&["--mock-delay-ms", "1500"], true, true);
    let uri = "file:///progress-edited.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let position = range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone();
    let old_hover = client.request_hover(uri, position.clone());
    let old_create = client.create_progress();
    client.change(uri, 2, json!([{"text":COMMENT_DOCUMENT}]));
    assert_error(client.receive(old_hover, TIMEOUT), -32801);
    let current_hover = client.request_hover(uri, position);
    assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
    assert!(
        client.creates.is_empty(),
        "One outstanding registration only"
    );
    client.send(Response::new_ok(old_create.id.clone(), Value::Null).into());
    let old_begin = client.progress_kind("begin");
    assert_eq!(old_begin["token"], old_create.params["token"]);
    assert!(old_begin["value"].get("message").is_none());
    assert_eq!(
        client.progress_kind("end")["token"],
        old_create.params["token"]
    );
    let current_create = client.create_progress();
    assert_ne!(current_create.params["token"], old_create.params["token"]);
    client.notify(
        "window/workDoneProgress/cancel",
        json!({"token":old_create.params["token"]}),
    );
    client.send(Response::new_ok(old_create.id, Value::Null).into());
    client.send(Response::new_ok(current_create.id, Value::Null).into());
    assert_eq!(
        client.progress_kind("begin")["token"],
        current_create.params["token"]
    );
    client.close(uri);
    assert_error(client.receive(current_hover, TIMEOUT), -32801);
    assert_eq!(
        client.progress_kind("end")["token"],
        current_create.params["token"]
    );
    assert!(client.statuses.is_empty());
    assert!(client.notifications.is_empty());
    client.finish(true);
}

#[test]
fn progress_cancel_retires_running_and_queued_jobs_and_shutdown_flushes_end() {
    let mut client = Client::with_activity(&["--mock-delay-ms", "1500"], true, false);
    let position = range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone();
    let mut owners = Vec::new();
    for index in 0..3 {
        let uri = format!("file:///progress-queue-{index}.rs");
        client.open(&uri, "rust", COMMENT_DOCUMENT, 1);
        owners.push(client.request_hover(&uri, position.clone()));
    }
    let create = client.create_progress();
    assert!(
        client
            .hover(
                "file:///progress-queue-0.rs",
                json!({"line":0,"character":0})
            )
            .is_null()
    );
    client.send(Response::new_ok(create.id, Value::Null).into());
    let mut progress = client.progress_kind("begin");
    while progress["value"]["message"] != "Translating: 2, queued: 1" {
        assert!(matches!(
            progress["value"]["message"].as_str(),
            Some("Queued: 3" | "Translating: 1, queued: 2")
        ));
        progress = client.progress_kind("report");
    }
    client.notify(
        "window/workDoneProgress/cancel",
        json!({"token":create.params["token"]}),
    );
    for id in owners {
        assert_error(client.receive(id, TIMEOUT), -32800);
    }
    assert_eq!(client.progress_kind("end")["token"], create.params["token"]);
    let pending = client.request_hover("file:///progress-queue-0.rs", position);
    let create = client.create_progress();
    client.send(Response::new_ok(create.id, Value::Null).into());
    client.progress_kind("begin");
    let shutdown = client.request("shutdown", Value::Null);
    assert_error(client.receive(pending, TIMEOUT), -32800);
    assert!(client.receive(shutdown, TIMEOUT).unwrap().is_null());
    assert_eq!(client.progress_kind("end")["token"], create.params["token"]);
    client.notify("exit", Value::Null);
    assert!(client.child.wait().unwrap().success());
    client.reader.take().unwrap().join().unwrap();
    assert!(client.messages.try_iter().next().is_none());
    assert!(client.stderr.take().unwrap().join().unwrap().is_empty());
}

#[test]
fn translation_problems_clear_on_edit_and_fallback_channels_remain_independent() {
    for (args, expected) in [
        (vec!["--mock-fail"], "Simulated mock translation failure"),
        (
            vec!["--mock-delay-ms", "1000", "--mock-timeout-ms", "80"],
            "Translation timed out. Try a shorter selection or adjust the timeout in Zed settings.",
        ),
    ] {
        let mut client = Client::with_activity(&args, false, true);
        let uri = "file:///problem.rs";
        client.open(uri, "rust", COMMENT_DOCUMENT, 1);
        let position = range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone();
        let failed = client.request_hover(uri, position);
        assert_error(client.receive(failed, TIMEOUT), -32803);
        assert_eq!(
            client.status(),
            json!({
                "health":"warning", "quiescent":true, "message":expected
            })
        );
        client.change(uri, 2, json!([{"text":COMMENT_DOCUMENT}]));
        assert_eq!(client.status(), json!({"health":"ok","quiescent":true}));
        assert!(client.creates.is_empty());
        assert!(client.progress.is_empty());
        assert!(client.notifications.is_empty());
        client.finish(true);
    }
    let mut client = Client::with_activity(&["--mock-delay-ms", "60"], true, false);
    let uri = "file:///declined.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let hover = client.request_hover(uri, range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone());
    let create = client.create_progress();
    client.send(Response::new_err(create.id, -32601, "unsupported".into()).into());
    assert!(
        client.receive(hover, TIMEOUT).unwrap()["contents"]["value"]
            .as_str()
            .unwrap()
            .contains(COMMENT_JA)
    );
    assert!(client.progress.is_empty());
    client.finish(true);
}

#[test]
fn a_failed_ticket_reports_its_problem_while_another_ticket_remains_busy() {
    let mut client = Client::with_activity(
        &["--mock-delay-ms", "10000", "--mock-timeout-ms", "2000"],
        false,
        true,
    );
    let position = range_for(COMMENT_DOCUMENT, COMMENT)["start"].clone();
    client.open("file:///problem-first.rs", "rust", COMMENT_DOCUMENT, 1);
    let first = client.request_hover("file:///problem-first.rs", position.clone());
    assert!(
        client
            .hover("file:///problem-first.rs", json!({"line":0,"character":0}))
            .is_null()
    );
    thread::sleep(Duration::from_millis(50));
    client.open("file:///problem-second.rs", "rust", COMMENT_DOCUMENT, 1);
    let second = client.request_hover("file:///problem-second.rs", position);
    assert_error(client.receive(first, TIMEOUT), -32803);
    assert_eq!(
        client.status(),
        json!({
            "health":"warning", "quiescent":false, "message":"Translation timed out. Try a shorter selection or adjust the timeout in Zed settings."
        })
    );
    assert_error(client.receive(second, TIMEOUT), -32803);
    assert_eq!(
        client.status(),
        json!({
            "health":"warning", "quiescent":true, "message":"Translation timed out. Try a shorter selection or adjust the timeout in Zed settings."
        })
    );
    assert!(client.notifications.is_empty());
    client.finish(true);
}

#[test]
fn comment_and_markdown_selection_preview_leave_source_unchanged() {
    let mut client = Client::new(&["--mock-delay-ms", "40"]);
    for (uri, language, document, selected, translated) in [
        (
            "file:///comment.rs",
            "rust",
            COMMENT_DOCUMENT,
            COMMENT,
            COMMENT_JA,
        ),
        (
            "file:///paragraph.md",
            "markdown",
            MARKDOWN_DOCUMENT,
            PARAGRAPH,
            PARAGRAPH_JA,
        ),
    ] {
        client.open(uri, language, document, 1);
        let range = range_for(document, selected);
        let action = client.action(uri, range.clone());
        // Allow enough time for an incorrectly started mock job to finish.
        thread::sleep(Duration::from_millis(100));
        if language == "markdown" {
            assert!(client.hover(uri, range["start"].clone()).is_null());
        }
        let started = Instant::now();
        let execution = client.execute(action.clone());
        assert!(client.receive(execution, TIMEOUT).unwrap().is_null());
        let notice = client.notice();
        assert_eq!(notice["type"], 3);
        assert!(notice["message"].as_str().unwrap().ends_with(READY));
        client.notifications.clear();
        assert!(
            started.elapsed() >= Duration::from_millis(35),
            "Listing candidates must not start or cache a translation"
        );
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            selected,
            translated,
            &range,
        );
        // Hover has no selection payload: the preview survives clearing the selection.
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            selected,
            translated,
            &range,
        );
        assert!(
            client
                .hover(uri, json!({"line": 0, "character": 0}))
                .is_null()
        );
        assert_eq!(client.action(uri, range.clone()), action);
        let cached = client.execute(action);
        assert!(client.receive(cached, TIMEOUT).unwrap().is_null());
        let notice = client.notice();
        assert_eq!(notice["type"], 3);
        assert!(notice["message"].as_str().unwrap().ends_with(READY));
        client.notifications.clear();
    }
    client.finish(true);
}

#[test]
fn a_notice_names_the_file_as_it_is_written_and_not_as_its_uri_encodes_it() {
    let mut client = Client::new(&[]);
    let uri = "file:///work/%E7%BF%BB%E8%A8%B3%20notes.md";
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let action = client.action(uri, range_for(MARKDOWN_DOCUMENT, PARAGRAPH));
    let execution = client.execute(action);
    assert!(client.receive(execution, TIMEOUT).unwrap().is_null());
    assert_eq!(
        client.notice(),
        json!({"type": 3, "message": format!("翻訳 notes.md:3-4: {READY}")})
    );
    client.finish(true);
}

#[test]
fn whole_line_whitespace_is_preserved_and_multiple_ranges_remain_readable() {
    let mut client = Client::new(&[]);
    let uri = "file:///two-selections.md";
    let comment = format!("    {}\r\n", COMMENT.replace('\n', "\r\n"));
    let paragraph = format!("{}\r\n", PARAGRAPH.replace('\n', "\r\n"));
    let document = format!("前🚀\r\n{comment}separator\r\n{paragraph}Tail");
    client.open(uri, "markdown", &document, 1);
    let first_range = range_for(&document, &comment);
    let action = client.action(uri, first_range.clone());
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, first_range["end"].clone()),
        &comment,
        COMMENT_JA,
        &first_range,
    );

    let last_range = range_for(&document, &paragraph);
    let action = client.action(uri, last_range.clone());
    assert_preview(
        &client.hover(uri, first_range["start"].clone()),
        &comment,
        COMMENT_JA,
        &first_range,
    );
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, first_range["end"].clone()),
        &comment,
        COMMENT_JA,
        &first_range,
    );
    assert_preview(
        &client.hover(uri, last_range["end"].clone()),
        &paragraph,
        PARAGRAPH_JA,
        &last_range,
    );
    let mut beyond = last_range["end"].clone();
    beyond["character"] = json!(beyond["character"].as_u64().unwrap() + 1);
    assert!(client.hover(uri, beyond).is_null());
    client.finish(true);
}

#[test]
fn unsaved_utf16_crlf_and_incremental_updates_select_the_current_source() {
    let mut client = Client::new(&[]);
    let uri = "file:///unsaved.md";
    client.open(uri, "markdown", "Disk text", 1);
    let original = PARAGRAPH.replace('\n', "\r\n");
    let final_document = format!("前🚀 {original}\r\nTail");
    let initial_document = final_document.replace("temporary", "permanent");
    client.change(uri, 2, json!([{ "text": initial_document }]));
    client.change(
        uri,
        3,
        json!([{
            "range": { "start": {"line": 1, "character": 19}, "end": {"line": 1, "character": 28} },
            "text": "temporary"
        }]),
    );
    let range = range_for(&final_document, &original);
    assert_eq!(range["start"], json!({"line": 0, "character": 4}));
    let action = client.action(uri, range.clone());
    assert_eq!(action["arguments"][0]["version"], 3);
    let execution = client.execute(action.clone());
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, range["end"].clone()),
        &original,
        PARAGRAPH_JA,
        &range,
    );

    client.change(
        uri,
        4,
        json!([{
            "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0} },
            "text": "extra "
        }]),
    );
    assert!(client.hover(uri, range["start"].clone()).is_null());
    let stale = client.execute(action);
    assert_error(client.receive(stale, TIMEOUT), -32801);
    let new_document = format!("extra {final_document}");
    let range = range_for(&new_document, &original);
    let action = client.action(uri, range.clone());
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        &original,
        PARAGRAPH_JA,
        &range,
    );
    client.finish(true);
}

#[test]
fn five_second_first_hover_returns_translation_without_blocking_sync() {
    let mut client = Client::new(&["--mock-delay-ms", "5000"]);
    let slow_uri = "file:///slow.rs";
    let other_uri = "file:///other.md";
    client.open(slow_uri, "rust", COMMENT_DOCUMENT, 1);
    client.open(other_uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let started = Instant::now();
    let hover = client.request(
        "textDocument/hover",
        json!({
            "textDocument": { "uri": slow_uri }, "position": range["start"]
        }),
    );
    let outside_started = Instant::now();
    assert!(
        client
            .hover(slow_uri, json!({"line": 0, "character": 0}))
            .is_null()
    );
    let outside_hover = outside_started.elapsed();
    let other_started = Instant::now();
    assert!(
        client
            .hover(other_uri, json!({"line": 0, "character": 0}))
            .is_null()
    );
    let other_hover = other_started.elapsed();
    let change_started = Instant::now();
    let updated = format!("heading\n{MARKDOWN_DOCUMENT}");
    client.change(other_uri, 2, json!([{ "text": updated }]));
    let action = client.action(other_uri, range_for(&updated, PARAGRAPH));
    assert_eq!(action["arguments"][0]["version"], 2);
    let sync_round_trip = change_started.elapsed();
    assert!(outside_hover < Duration::from_millis(500));
    assert!(other_hover < Duration::from_millis(500));
    assert!(sync_round_trip < Duration::from_millis(500));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        !client.replies.contains_key(&hover),
        "First Hover replied before its work completed"
    );
    assert_preview(
        &client.receive(hover, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    let translation_time = started.elapsed();
    assert!(translation_time >= Duration::from_millis(4900));
    let mut durations = Vec::new();
    for _ in 0..100 {
        let started = Instant::now();
        assert_preview(
            &client.hover(slow_uri, range["end"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        durations.push(started.elapsed());
    }
    durations.sort();
    let p95 = durations[94];
    assert!(
        p95 < Duration::from_millis(250),
        "Cached stdio hover P95: {p95:?}"
    );
    eprintln!(
        "stdio (5000ms first Hover): outside={outside_hover:?}, other={other_hover:?}, sync={sync_round_trip:?}, first={translation_time:?}, cached P95={p95:?} (n=100)"
    );
    client.finish(true);
}

#[test]
fn edits_and_close_cancel_retained_work_and_reopen_rejects_old_actions() {
    let mut client = Client::new(&["--mock-delay-ms", "300"]);
    let uri = "file:///changing.md";
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let range = range_for(MARKDOWN_DOCUMENT, PARAGRAPH);
    let old = client.action(uri, range.clone());
    let command = client.execute(old.clone());
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    client.change(uri, 2, json!([{"text":MARKDOWN_DOCUMENT}]));
    assert!(client.hover(uri, range["start"].clone()).is_null());
    let command = client.execute(old);
    assert_error(client.receive(command, TIMEOUT), -32801);
    let action = client.action(uri, range.clone());
    let command = client.execute(action.clone());
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    client.close(uri);
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 2);
    let command = client.execute(action);
    assert_error(client.receive(command, TIMEOUT), -32801);
    thread::sleep(Duration::from_millis(350));
    assert!(client.hover(uri, range["start"].clone()).is_null());
    assert!(client.notifications.is_empty());
    let action = client.action(uri, range.clone());
    let command = client.execute(action);
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        PARAGRAPH,
        PARAGRAPH_JA,
        &range,
    );
    client.close(uri);
    assert!(client.hover(uri, range["start"].clone()).is_null());
    client.finish(true);
}

#[test]
fn command_request_cancellation_preserves_work_and_range_action_cancels_it() {
    let mut client = Client::new(&["--mock-delay-ms", "300"]);
    let uri = "file:///cancel.md";
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let range = range_for(MARKDOWN_DOCUMENT, PARAGRAPH);
    let action = client.action(uri, range.clone());
    let id = RequestId::from("retained-command".to_owned());
    client.request_with_id(id.clone(), "workspace/executeCommand", action.clone());
    assert!(client.receive(id.clone(), TIMEOUT).unwrap().is_null());
    client.notify("$/cancelRequest", json!({"id":"retained-command"}));
    let actions = client.actions(uri, range.clone());
    let cancel = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["command"]["command"] == "translation-lsp.cancel-range")
        .unwrap()["command"]
        .clone();
    let request = client.execute(cancel.clone());
    assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    thread::sleep(Duration::from_millis(350));
    assert!(client.hover(uri, range["start"].clone()).is_null());
    client.request_with_id(id.clone(), "workspace/executeCommand", action);
    assert!(client.receive(id, TIMEOUT).unwrap().is_null());
    // Reusing an old cancellation action cannot cancel the new ticket.
    let stale = client.execute(cancel);
    assert!(client.receive(stale, TIMEOUT).unwrap().is_null());
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        PARAGRAPH,
        PARAGRAPH_JA,
        &range,
    );
    client.finish(true);
}

#[test]
fn simulated_failures_are_visible_per_range_and_edits_clear_them() {
    for (args, text, selected) in [
        (vec!["--mock-fail"], MARKDOWN_DOCUMENT, PARAGRAPH),
        (vec![], "Arbitrary text", "Arbitrary text"),
    ] {
        let mut client = Client::new(&args);
        let uri = "file:///failure.md";
        client.open(uri, "markdown", text, 1);
        let range = range_for(text, selected);
        let action = client.action(uri, range.clone());
        let command = client.execute(action);
        assert!(client.receive(command, TIMEOUT).unwrap().is_null());
        let notice = client.notice();
        assert_eq!(notice["type"], 1);
        let hover = client.hover(uri, range["start"].clone());
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("translation failed")
        );
        assert!(
            hover["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("retry")
        );
        client.change(uri, 2, json!([{"text":MARKDOWN_DOCUMENT}]));
        assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
        client.finish(true);
    }
}

#[test]
fn invalid_ranges_empty_selections_and_command_arguments_are_rejected() {
    let mut client = Client::new(&[]);
    let uri = "file:///invalid.md";
    client.open(uri, "markdown", &format!("🚀 {PARAGRAPH}"), 1);
    for range in [
        json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}),
        json!({"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 3}}),
        json!({"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}}),
        json!({"start": {"line": 0, "character": 9}, "end": {"line": 0, "character": 5}}),
        json!({"start": {"line": 0, "character": 0}, "end": {"line": 99, "character": 0}}),
        json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 999}}),
    ] {
        assert_eq!(client.actions(uri, range), json!([]));
    }
    let document = format!("🚀 {PARAGRAPH}");
    let mut action = client.action(uri, range_for(&document, PARAGRAPH));
    action["arguments"][0]["range"]["start"]["character"] = json!(1);
    let execution = client.execute(action);
    let error = client.receive(execution, TIMEOUT).unwrap_err();
    assert_eq!(error.code, -32602);
    assert_eq!(
        client.notifications,
        vec![json!({"type":1,"message":error.message})]
    );
    client.notifications.clear();
    let mut action = client.action(uri, range_for(&document, PARAGRAPH));
    action["arguments"][0]["targetLanguage"] = json!("en");
    let execution = client.execute(action);
    assert_error(client.receive(execution, TIMEOUT), -32602);
    let oversized = "a".repeat(16 * 1024 + 1);
    client.change(uri, 2, json!([{ "text": oversized }]));
    assert_eq!(
        client.actions(uri, range_for(&oversized, &oversized)),
        json!([])
    );
    assert_error(
        client.call(
            "workspace/executeCommand",
            json!({ "command": "other", "arguments": [] }),
        ),
        -32602,
    );
    assert_error(
        client.call("textDocument/hover", json!({ "bad": "parameters" })),
        -32602,
    );
    assert_error(client.call("unknown/request", json!({})), -32601);
    assert_eq!(
        client.notifications.len(),
        2,
        "Only explicit command failures notify"
    );
    client.finish(true);
}

#[test]
fn configurable_queue_accepts_multiple_ranges_and_shutdown_retires_retained_work() {
    let mut client = Client::configured(
        &["--mock-delay-ms", "5000"],
        false,
        false,
        json!({"translation_limits":{"work":{"admitted":4,"automatic":2,"concurrent":1}}}),
    );
    let uri = "file:///queue.md";
    let text = format!("{PARAGRAPH}\n\n{COMMENT}\n\n{PARAGRAPH}");
    client.open(uri, "markdown", &text, 1);
    let first = range_for(&text, PARAGRAPH);
    let second = range_for(&text, COMMENT);
    for range in [first.clone(), second] {
        let action = client.action(uri, range);
        let request = client.execute(action);
        assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    }
    // An exact duplicate shares a job, rather than consuming another admission.
    let action = client.action(uri, first);
    let request = client.execute(action);
    assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    for i in 0..2 {
        let uri = format!("file:///queued-{i}.md");
        client.open(&uri, "markdown", MARKDOWN_DOCUMENT, 1);
        let action = client.action(&uri, range_for(MARKDOWN_DOCUMENT, PARAGRAPH));
        let request = client.execute(action);
        assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    }
    let uri = "file:///nested/overflow.md";
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let action = client.action(uri, range_for(MARKDOWN_DOCUMENT, PARAGRAPH));
    let request = client.execute(action);
    let error = client.receive(request, TIMEOUT).unwrap_err();
    assert_eq!(error.code, -32803);
    assert_eq!(
        error.message,
        "overflow.md:3-4: Translation queue is full; this range was not accepted"
    );
    assert_eq!(
        client.notifications.last().unwrap(),
        &json!({"type":1,"message":error.message})
    );
    client.finish(true);
}

#[test]
fn newline_join_split_and_sequential_edits_keep_current_translation_ranges() {
    let mut client = Client::new(&[]);
    for language in ["rust", "markdown"] {
        let (sample, translation, start_character) = if language == "rust" {
            (format!("    {COMMENT}"), COMMENT_JA, 4)
        } else {
            (PARAGRAPH.into(), PARAGRAPH_JA, 0)
        };
        let uri = if language == "rust" {
            "file:///joined.rs"
        } else {
            "file:///joined.md"
        };
        let end_character = sample.lines().last().unwrap().encode_utf16().count();
        let range = |line| {
            json!({
                "start": {"line":line,"character":start_character},
                "end": {"line":line + 1,"character":end_character}
            })
        };
        let mut version = 1;
        client.open(uri, language, &format!("前🚀\rX\n{sample}"), version);
        let show = |client: &mut Client, line| {
            let range = range(line);
            let action = client.action(uri, range.clone());
            let request = client.execute(action);
            client.receive(request, TIMEOUT).unwrap();
            assert_preview(
                &client.hover(uri, range["start"].clone()),
                &sample,
                translation,
                &range,
            );
        };
        show(&mut client, 2);
        version += 1;
        client.change(
            uri,
            version,
            json!([{
                "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}},
                "text":""
            }]),
        );
        show(&mut client, 1);
        version += 1;
        client.change(
            uri,
            version,
            json!([{
                "range":{"start":{"line":0,"character":3},"end":{"line":1,"character":0}},
                "text":"\rX\n"
            }, {
                "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                "text":"🚀前"
            }]),
        );
        show(&mut client, 2);
        version += 1;
        client.change(
            uri,
            version,
            json!([{
                "range":{"start":{"line":0,"character":3},"end":{"line":2,"character":0}},
                "text":"\n"
            }]),
        );
        show(&mut client, 1);
        client.close(uri);
    }
    client.finish(true);
}

#[test]
fn invalid_incremental_sync_discards_cache_and_recovers_with_full_text() {
    let mut client = Client::new(&[]);
    let uri = "file:///bad-update.md";
    client.open(uri, "markdown", MARKDOWN_DOCUMENT, 1);
    let range = range_for(MARKDOWN_DOCUMENT, PARAGRAPH);
    let action = client.action(uri, range.clone());
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    client.change(
        uri,
        2,
        json!([{
            "range": { "start": {"line": 3, "character": 38}, "end": {"line": 3, "character": 39} },
            "text": "bad surrogate edit"
        }]),
    );
    assert!(client.hover(uri, range["start"].clone()).is_null());
    assert_eq!(client.actions(uri, range.clone()), json!([]));
    client.change(uri, 3, json!([{ "text": MARKDOWN_DOCUMENT }]));
    let action = client.action(uri, range.clone());
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        PARAGRAPH,
        PARAGRAPH_JA,
        &range,
    );
    client.finish(false);
}

#[test]
fn automatic_hover_recognizes_comments_and_returns_only_known_translations() {
    let mut client = Client::new(&[]);
    let uri = "file:///fixture.rs";
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    for ending in ["\n", "\r\n", "\r"] {
        client.open(uri, "rust", &COMMENT_DOCUMENT.replace('\n', ending), 1);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        assert!(
            client
                .hover(uri, json!({"line": 3, "character": 10}))
                .is_null()
        );
        client.close(uri);
    }
    client.open(uri, "markdown", COMMENT_DOCUMENT, 1);
    assert!(client.hover(uri, range["start"].clone()).is_null());
    client.open(uri, "rust", &format!("let text = r#\"{COMMENT}\"#;"), 1);
    assert!(
        client
            .hover(uri, json!({"line": 1, "character": 10}))
            .is_null()
    );
    client.open(uri, "rust", "// Arbitrary comment", 1);
    assert_error(
        client.call(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri}, "position": {"line": 0, "character": 5}
            }),
        ),
        -32803,
    );
    assert!(
        client.notifications.is_empty(),
        "Automatic Hover should be silent"
    );
    client.finish(true);
}

#[test]
fn general_comment_hovers_use_zed_language_ids_uri_dialects_and_new_versions() {
    let mut client = Client::new(&[]);
    for (uri, language, lead) in [
        ("file:///general.rs", "rust", "let s = r#\"// string\"#;"),
        (
            "file:///general.js",
            "javascript",
            "const t = `// template`; const view = <div>// jsx text</div>;",
        ),
        (
            "file:///general.jsx",
            "javascript",
            "const view = <div>// jsx text</div>;",
        ),
        ("file:///general.ts", "typescript", "const n: number = 1;"),
        ("file:///general.cs", "csharp", "var s = @\"// string\";"),
        (
            "untitled:general.tsx",
            "typescriptreact",
            "const view: Element = <div>// jsx text</div>;",
        ),
        ("file:///general.json", "json", "\"// string\""),
        ("file:///general.jsonc", "jsonc", "{\"s\": \"// string\"}"),
        ("file:///general.yaml", "yaml", "s: \"# string\""),
        ("file:///general.toml", "toml", "s = \"# string\""),
        ("file:///general.sh", "shellscript", "s=\"# string\""),
        ("file:///general.ps1", "powershell", "$s = \"# string\""),
        ("file:///Dockerfile", "dockerfile", "ENV s=\"# string\""),
        ("file:///compose.yaml", "dockercompose", "s: \"# string\""),
    ] {
        let hash = matches!(
            language,
            "yaml" | "toml" | "shellscript" | "powershell" | "dockerfile" | "dockercompose"
        );
        let comment = if hash {
            COMMENT.replace("//", "#")
        } else {
            COMMENT.to_owned()
        };
        let source = format!("{lead}\n    {comment}\n");
        let range = range_for(&source, &comment);
        client.open(uri, language, &source, 1);
        for point in [
            json!({"line": 1, "character": 0}),
            json!({"line": 2, "character": 2}),
        ] {
            assert!(client.hover(uri, point).is_null());
        }
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            &comment,
            COMMENT_JA,
            &range,
        );
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            &comment,
            COMMENT_JA,
            &range,
        );
        let literal = match language {
            "rust" => format!("let t = r#\"{comment}\"#;"),
            "csharp" => format!("var t = @\"{comment}\";"),
            // JSON strings cannot span lines.
            "json" | "jsonc" => format!("[\n\"{}\"]", comment.replace('\n', " ")),
            "yaml" | "dockercompose" => format!("t: \"{comment}\""),
            "toml" => format!("t = \"\"\"{comment}\"\"\""),
            "shellscript" => format!("t=\"{comment}\""),
            "powershell" => format!("$t = \"{comment}\""),
            // The lines of a here-document belong to its instruction.
            "dockerfile" => format!("RUN <<EOF\n{comment}\nEOF"),
            _ => format!("const t = `{comment}`;"),
        };
        client.change(uri, 2, json!([{"text": literal}]));
        assert!(
            client
                .hover(uri, json!({"line": 1, "character": 8}))
                .is_null()
        );
        client.change(uri, 3, json!([{"text": source}]));
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            &comment,
            COMMENT_JA,
            &range,
        );
        // The auxiliary selection path remains available in every registered language.
        let action = client.action(uri, range.clone());
        let execution = client.execute(action);
        client.receive(execution, TIMEOUT).unwrap();
        client.close(uri);
    }
    client.finish(true);
}

#[test]
fn adjacent_block_start_and_unclosed_blocks_do_not_reuse_previous_preview() {
    let mut client = Client::new(&[]);
    let first = "/* Retry only if the remote service\nreports a temporary failure. */";
    for (uri, language) in [
        ("file:///blocks.rs", "rust"),
        ("file:///blocks.js", "javascript"),
    ] {
        let source = format!("{first}/* unknown */");
        let range = range_for(&source, first);
        client.open(uri, language, &source, 1);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            first,
            COMMENT_JA,
            &range,
        );
        assert_error(
            client.call(
                "textDocument/hover",
                json!({"textDocument": {"uri": uri}, "position": range["end"]}),
            ),
            -32803,
        );
        client.change(
            uri,
            2,
            json!([{"text": "/* Retry only if the remote service\nreports a temporary failure."}]),
        );
        assert!(
            client
                .hover(uri, json!({"line": 0, "character": 3}))
                .is_null()
        );
        client.change(uri, 3, json!([{"text": first}]));
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            first,
            COMMENT_JA,
            &range,
        );
        // With no next token, the existing preview's keyboard end remains readable.
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            first,
            COMMENT_JA,
            &range,
        );
        client.close(uri);
    }
    client.finish(true);
}

#[test]
fn cold_line_end_caret_translates_at_eof_and_original_line_endings() {
    let mut client = Client::new(&["--mock-delay-ms", "20"]);
    let range = json!({"start": {"line": 0, "character": 4}, "end": {"line": 1, "character": 35}});
    for (uri, language) in [
        ("file:///cold.rs", "rust"),
        ("file:///cold.js", "javascript"),
        ("file:///cold.ts", "typescript"),
    ] {
        for ending in ["\n", "\r\n", "\r"] {
            for suffix in ["", ending] {
                let source = format!("    {}{suffix}", COMMENT.replace('\n', ending));
                client.open(uri, language, &source, 1);
                let retained = client.request_hover(uri, range["end"].clone());
                assert!(
                    client
                        .hover(uri, json!({"line": 0, "character": 1}))
                        .is_null()
                );
                assert_preview(
                    &client.receive(retained, TIMEOUT).unwrap(),
                    COMMENT,
                    COMMENT_JA,
                    &range,
                );
                client.close(uri);
            }
        }
    }
    client.finish(true);
}

#[test]
fn cached_comment_end_does_not_cover_an_unclosed_block_and_raw_selection_end_is_kept() {
    let mut client = Client::new(&[]);
    let left = "/* Retry only if the remote service\nreports a temporary failure. */";
    for (uri, language) in [
        ("file:///boundary.rs", "rust"),
        ("file:///boundary.js", "javascript"),
    ] {
        for explicit in [false, true] {
            let source = format!("{left}/* unfinished");
            let range = range_for(&source, left);
            client.open(uri, language, &source, 1);
            assert!(client.hover(uri, range["end"].clone()).is_null());
            if explicit {
                let action = client.action(uri, range.clone());
                let execution = client.execute(action);
                client.receive(execution, TIMEOUT).unwrap();
            }
            assert_preview(
                &client.hover(uri, range["start"].clone()),
                left,
                COMMENT_JA,
                &range,
            );
            assert!(client.hover(uri, range["end"].clone()).is_null());
            assert_preview(
                &client.hover(uri, range["start"].clone()),
                left,
                COMMENT_JA,
                &range,
            );
            client.close(uri);
        }
        let literal = if language == "rust" {
            format!("let text = r#\"{COMMENT}/* unfinished\"#;")
        } else {
            format!("const text = `{COMMENT}/* unfinished`;")
        };
        let range = range_for(&literal, COMMENT);
        client.open(uri, language, &literal, 1);
        let action = client.action(uri, range.clone());
        let execution = client.execute(action);
        client.receive(execution, TIMEOUT).unwrap();
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        client.close(uri);
    }
    client.finish(true);
}

#[test]
fn oxc_fatal_tail_keeps_selection_preview_available() {
    let mut client = Client::new(&[]);
    let uri = "file:///unfinished.ts";
    let source = format!("const s = 'unclosed\n{COMMENT}");
    let range = range_for(&source, COMMENT);
    client.open(uri, "typescript", &source, 1);
    assert!(client.hover(uri, range["start"].clone()).is_null());
    let action = client.action(uri, range.clone());
    assert!(client.hover(uri, range["start"].clone()).is_null());
    let execution = client.execute(action);
    client.receive(execution, TIMEOUT).unwrap();
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    client.finish(true);
}

#[test]
fn coalesced_hovers_complete_once_and_one_cancellation_keeps_the_other_requests() {
    let mut client = Client::new(&["--mock-delay-ms", "1000"]);
    let uri = "file:///coalesced.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let started = Instant::now();
    let canceled = client.request_hover(uri, range["start"].clone());
    let remaining = client.request_hover(uri, range["end"].clone());
    let action = client.action(uri, range.clone());
    let command = client.execute(action.clone());
    let second_command = client.execute(action);
    client.notify(
        "$/cancelRequest",
        json!({"id": serde_json::to_value(canceled.clone()).unwrap()}),
    );
    assert_error(client.receive(canceled, TIMEOUT), -32800);
    assert!(client.notifications.is_empty());
    assert!(!client.replies.contains_key(&remaining));
    assert_preview(
        &client.receive(remaining, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    assert!(client.receive(command, TIMEOUT).unwrap().is_null());
    assert!(client.receive(second_command, TIMEOUT).unwrap().is_null());
    let notice = client.notice();
    assert_eq!(notice["type"], 3);
    assert!(notice["message"].as_str().unwrap().ends_with(READY));
    assert!(started.elapsed() >= Duration::from_millis(990));
    assert!(
        started.elapsed() < Duration::from_millis(1900),
        "Shared work ran multiple translations"
    );
    client.finish(true);
}

#[test]
fn coalesced_commands_acknowledge_once_and_report_one_failure_for_the_range() {
    for args in [
        vec!["--mock-delay-ms", "400", "--mock-fail"],
        vec!["--mock-delay-ms", "400", "--mock-timeout-ms", "100"],
    ] {
        let mut client = Client::new(&args);
        let uri = "file:///shared-failure.rs";
        client.open(uri, "rust", COMMENT_DOCUMENT, 1);
        let range = range_for(COMMENT_DOCUMENT, COMMENT);
        let action = client.action(uri, range.clone());
        let hover = client.request_hover(uri, range["start"].clone());
        let first = client.execute(action.clone());
        let second = client.execute(action);
        assert!(client.receive(first, TIMEOUT).unwrap().is_null());
        assert!(client.receive(second, TIMEOUT).unwrap().is_null());
        assert_error(client.receive(hover, TIMEOUT), -32803);
        assert_eq!(client.notifications.len(), 1);
        assert_eq!(client.notifications[0]["type"], 1);
        assert!(!client.notifications[0].to_string().contains(COMMENT));
        let failed = client.hover(uri, range["start"].clone());
        assert!(
            failed["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("translation failed")
        );
        client.change(uri, 2, json!([{"text":"fn main() {}"}]));
        assert!(client.hover(uri, range["start"].clone()).is_null());
        client.finish(true);
    }
}

#[test]
fn canceled_hover_id_can_be_reused_to_join_retained_work() {
    let mut client = Client::new(&["--mock-delay-ms", "500"]);
    let uri = "file:///reuse.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let id = RequestId::from("hover-reuse".to_owned());
    let params = json!({"textDocument":{"uri":uri}, "position":range["start"]});
    client.request_with_id(id.clone(), "textDocument/hover", params.clone());
    assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
    client.notify("$/cancelRequest", json!({"id":"hover-reuse"}));
    assert_error(client.receive(id.clone(), TIMEOUT), -32800);
    client.request_with_id(id.clone(), "textDocument/hover", params);
    assert_preview(
        &client.receive(id, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    client.finish(true);
}

#[test]
fn pending_hovers_are_invalidated_by_edit_close_reopen_and_broken_sync() {
    for operation in ["edit", "reopen", "broken-sync"] {
        let mut client = Client::new(&["--mock-delay-ms", "500"]);
        let uri = "file:///invalidate.rs";
        client.open(uri, "rust", COMMENT_DOCUMENT, 1);
        let range = range_for(COMMENT_DOCUMENT, COMMENT);
        let first = client.request_hover(uri, range["start"].clone());
        let second = client.request_hover(uri, range["end"].clone());
        assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
        match operation {
            "edit" => client.change(uri, 2, json!([{"text": "// changed"}])),
            "reopen" => {
                client.close(uri);
                client.open(uri, "rust", COMMENT_DOCUMENT, 1);
            }
            _ => client.change(
                uri,
                2,
                json!([{
                    "range":{"start":{"line":99,"character":0},"end":{"line":99,"character":1}},
                    "text":"bad"
                }]),
            ),
        }
        assert_error(client.receive(first, TIMEOUT), -32801);
        assert_error(client.receive(second, TIMEOUT), -32801);
        if operation != "reopen" {
            assert!(client.hover(uri, range["start"].clone()).is_null());
            client.change(uri, 3, json!([{"text":COMMENT_DOCUMENT}]));
        }
        let restarted = Instant::now();
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        assert!(
            restarted.elapsed() >= Duration::from_millis(490),
            "Old work populated the reopened/edited cache"
        );
        client.finish(operation != "broken-sync");
    }
}

#[test]
fn automatic_hover_failure_and_deadline_preserve_failure_without_blocking_other_requests() {
    for args in [
        vec!["--mock-fail", "--mock-delay-ms", "100"],
        vec!["--mock-delay-ms", "5000", "--mock-timeout-ms", "100"],
    ] {
        let mut client = Client::new(&args);
        let uri = "file:///failure.rs";
        client.open(uri, "rust", COMMENT_DOCUMENT, 1);
        let range = range_for(COMMENT_DOCUMENT, COMMENT);
        let first = client.request_hover(uri, range["start"].clone());
        let second = client.request_hover(uri, range["end"].clone());
        let outside = Instant::now();
        assert!(client.hover(uri, json!({"line":0,"character":0})).is_null());
        assert!(outside.elapsed() < Duration::from_millis(500));
        for id in [first, second] {
            let error = client.receive(id, Duration::from_secs(1)).unwrap_err();
            assert_eq!(error.code, -32803);
            assert!(error.message.contains(if args.contains(&"--mock-fail") {
                "failure"
            } else {
                "timed out"
            }));
        }
        let cached = client.hover(uri, range["start"].clone());
        assert!(
            cached["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("translation failed")
        );
        assert!(
            client.notifications.is_empty(),
            "Automatic Hover failures should be silent"
        );
        client.finish(true);
    }
}

#[test]
fn queued_hover_deadline_includes_time_waiting_for_the_worker() {
    let mut client = Client::new(&["--mock-delay-ms", "1000", "--mock-timeout-ms", "1500"]);
    let first_uri = "file:///deadline-first.rs";
    let second_uri = "file:///deadline-second.rs";
    let queued_uri = "file:///deadline-queued.rs";
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    client.open(first_uri, "rust", COMMENT_DOCUMENT, 1);
    client.open(second_uri, "rust", COMMENT_DOCUMENT, 1);
    client.open(queued_uri, "rust", COMMENT_DOCUMENT, 1);
    let started = Instant::now();
    let first = client.request_hover(first_uri, range["start"].clone());
    let second = client.request_hover(second_uri, range["start"].clone());
    let queued = client.request_hover(queued_uri, range["end"].clone());
    assert_preview(
        &client.receive(first, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    assert_preview(
        &client.receive(second, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    let error = client.receive(queued, TIMEOUT).unwrap_err();
    assert_eq!(error.code, -32803);
    assert!(error.message.contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(3));
    client.finish(true);
}

#[test]
fn coalesced_hover_waiters_are_bounded_and_shutdown_cancels_them() {
    let mut client = Client::new(&["--mock-delay-ms", "5000"]);
    let uri = "file:///bounded.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let mut pending = Vec::new();
    for _ in 0..16 {
        pending.push(client.request_hover(uri, range["start"].clone()));
    }
    let rejected = client.request_hover(uri, range["end"].clone());
    assert_error(client.receive(rejected, TIMEOUT), -32803);
    let shutdown = client.request("shutdown", Value::Null);
    assert!(client.receive(shutdown, TIMEOUT).unwrap().is_null());
    for id in pending {
        assert_error(client.receive(id, TIMEOUT), -32800);
    }
    client.notify("exit", Value::Null);
    let started = Instant::now();
    while client.child.try_wait().unwrap().is_none() {
        assert!(started.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(5));
    }
    client.reader.take().unwrap().join().unwrap();
    assert!(client.messages.try_iter().next().is_none());
    assert!(client.replies.is_empty());
    assert!(client.stderr.take().unwrap().join().unwrap().is_empty());
}

#[test]
fn duplicate_live_hover_id_is_retired_once_and_can_later_be_reused() {
    let mut client = Client::new(&["--mock-delay-ms", "500"]);
    let uri = "file:///duplicate-id.rs";
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    let id = RequestId::from("duplicate-live".to_owned());
    let params = json!({"textDocument":{"uri":uri},"position":range["start"]});
    client.request_with_id(id.clone(), "textDocument/hover", params.clone());
    client.request_with_id(id.clone(), "textDocument/hover", params.clone());
    assert_error(client.receive(id.clone(), TIMEOUT), -32600);
    client.request_with_id(id.clone(), "textDocument/hover", params);
    assert_preview(
        &client.receive(id, TIMEOUT).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    client.finish(true);
}

fn document_action(client: &mut Client, uri: &str, range: Value) -> Value {
    client
        .actions(uri, range)
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["command"]["command"] == "translation-lsp.open-translation")
        .unwrap()["command"]
        .clone()
}

#[test]
fn document_translation_acknowledges_admission_before_slow_work_finishes() {
    let mut client = Client::configured_documents(
        &["--mock-delay-ms", "1200"],
        false,
        false,
        Value::Null,
        true,
    );
    let uri = "file:///slow.md";
    client.open(uri, "markdown", PARAGRAPH, 1);
    let action = document_action(
        &mut client,
        uri,
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}),
    );
    let id = client.execute(action);
    assert!(
        client
            .receive(id, Duration::from_millis(500))
            .unwrap()
            .is_null()
    );
    assert!(client.documents.is_empty());
    assert!(client.notifications.is_empty());
    while client.documents.is_empty() {
        client.pump();
    }
    let open = client.documents.remove(0);
    client.send(Response::new_ok(open.id, json!({"success": true})).into());
    client.finish(true);
}

#[test]
fn document_preview_opens_complete_translation_and_keeps_saved_copy() {
    for (language, extension) in [("markdown", "md"), ("plaintext", "txt")] {
        let mut client = Client::configured_documents(&[], false, false, Value::Null, true);
        let uri = format!("file:///source.{extension}");
        client.open(&uri, language, PARAGRAPH, 1);
        let action = document_action(
            &mut client,
            &uri,
            json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}),
        );
        assert_eq!(action["title"], "Translate document to ja and open in tab");
        let request = client.execute(action.clone());
        while client.documents.is_empty() {
            client.pump();
        }
        let open = client.documents.remove(0);
        assert_eq!(open.params["external"], false);
        assert_eq!(open.params["takeFocus"], true);
        let path = reqwest::Url::parse(open.params["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap();
        assert_eq!(path.extension().unwrap(), extension);
        assert_eq!(path.file_stem().unwrap(), "Translation");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PARAGRAPH_JA);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        client.send(Response::new_ok(open.id, json!({"success":true})).into());
        assert!(client.receive(request, TIMEOUT).unwrap().is_null());
        assert!(client.notifications.is_empty());
        let saved = path.with_extension("saved");
        std::fs::copy(&path, &saved).unwrap();
        let cached = client.execute(action);
        while client.documents.is_empty() {
            client.pump();
        }
        let open = client.documents.remove(0);
        let cached_path = reqwest::Url::parse(open.params["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap();
        assert_eq!(
            cached_path, path,
            "Cached tab must reuse the same preview file"
        );
        assert_eq!(std::fs::read_to_string(&cached_path).unwrap(), PARAGRAPH_JA);
        client.send(Response::new_ok(open.id, json!({"success":true})).into());
        assert!(client.receive(cached, TIMEOUT).unwrap().is_null());
        assert!(client.call("shutdown", Value::Null).unwrap().is_null());
        assert!(!path.exists());
        assert!(!cached_path.exists());
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), PARAGRAPH_JA);
        client.finish_exit(true);
        std::fs::remove_file(saved).unwrap();
    }
}

#[test]
fn document_preview_reports_editor_refusal_and_does_not_claim_hover_success() {
    let mut client = Client::configured_documents(&[], false, false, Value::Null, true);
    client.open("file:///source.md", "markdown", PARAGRAPH, 1);
    let action = document_action(
        &mut client,
        "file:///source.md",
        json!({"start":{"line":0,"character":0},"end":{"line":1,"character":40}}),
    );
    let request = client.execute(action);
    while client.documents.is_empty() {
        client.pump();
    }
    let open = client.documents.remove(0);
    client.send(Response::new_ok(open.id, json!({"success":false})).into());
    assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    // A following request is a barrier after the editor's refusal.
    client.hover("file:///source.md", json!({"line":0,"character":4}));
    assert_eq!(client.notifications.len(), 1);
    assert_eq!(client.notifications[0]["type"], 1);
    client.finish(true);
}

#[test]
fn document_preview_never_opens_canceled_stale_expired_or_failed_translation() {
    for mode in ["cancel", "change", "close", "timeout", "fail", "shutdown"] {
        let args: &[&str] = match mode {
            "timeout" => &["--mock-delay-ms", "500", "--mock-timeout-ms", "25"],
            "fail" => &["--mock-fail"],
            _ => &["--mock-delay-ms", "500"],
        };
        let mut client =
            Client::configured_documents(args, mode == "cancel", false, Value::Null, true);
        let uri = "file:///source.md";
        client.open(uri, "markdown", PARAGRAPH, 1);
        let action = document_action(
            &mut client,
            uri,
            json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}),
        );
        let id = client.execute(action);
        assert!(client.receive(id, TIMEOUT).unwrap().is_null());
        match mode {
            "cancel" => {
                let create = client.create_progress();
                client.send(Response::new_ok(create.id, Value::Null).into());
                let begin = client.progress_kind("begin");
                client.notify(
                    "window/workDoneProgress/cancel",
                    json!({"token":begin["token"]}),
                );
            }
            "change" => client.change(uri, 2, json!([{"text": "Changed text"}])),
            "close" => client.close(uri),
            "shutdown" => {
                client.finish(true);
                continue;
            }
            _ => (),
        }
        thread::sleep(Duration::from_millis(530));
        assert!(client.hover(uri, json!({"line":0,"character":4})).is_null());
        assert!(client.documents.is_empty(), "{mode}");
        assert_eq!(
            client.notifications.len(),
            usize::from(matches!(mode, "timeout" | "fail"))
        );
        client.finish(true);
    }
}

#[test]
fn document_preview_preserves_selection_scope_and_rejects_oversized_input() {
    let mut client = Client::configured_documents(&[], false, false, Value::Null, true);
    let uri = "file:///selected.md";
    client.open(
        uri,
        "markdown",
        &format!("Not selected\n{PARAGRAPH}\nAlso not selected"),
        1,
    );
    let range = json!({"start":{"line":1,"character":0},"end":{"line":2,"character":40}});
    let action = document_action(&mut client, uri, range);
    assert_eq!(action["title"], "Translate selection to ja and open in tab");
    let id = client.execute(action);
    while client.documents.is_empty() {
        client.pump();
    }
    let open = client.documents.remove(0);
    let path = reqwest::Url::parse(open.params["uri"].as_str().unwrap())
        .unwrap()
        .to_file_path()
        .unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), PARAGRAPH_JA);
    client.send(Response::new_ok(open.id, json!({"success":true})).into());
    client.receive(id, TIMEOUT).unwrap();
    client.change(uri, 2, json!([{"text": "a".repeat(1024 * 1024 + 1)}]));
    let action = document_action(
        &mut client,
        uri,
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}),
    );
    let id = client.execute(action);
    assert_eq!(
        client.receive(id, TIMEOUT).unwrap_err().message,
        "Translation size 1048577 bytes exceeds 1024 KiB; adjust translation_limits.input_kib.document"
    );
    assert!(client.documents.is_empty());
    client.finish(true);
}

#[test]
fn document_limits_accept_large_input_and_partial_settings_override_the_default() {
    for (options, size) in [
        (Value::Null, 1024 * 1024),
        (
            json!({"translation_limits":{"input_kib":{"document":2}}}),
            2 * 1024,
        ),
        (
            json!({"translation_limits":{"input_kib":{"document":2048}}}),
            2 * 1024 * 1024,
        ),
    ] {
        let mut client = Client::configured_documents(&[], false, false, options, true);
        let uri = "file:///large.txt";
        let source = format!("{}{}", " ".repeat(size - PARAGRAPH.len()), PARAGRAPH);
        client.open(uri, "plaintext", &source, 1);
        let range = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}});
        let action = document_action(&mut client, uri, range.clone());
        let id = client.execute(action);
        client.receive(id, TIMEOUT).unwrap();
        while client.documents.is_empty() {
            client.pump();
        }
        let open = client.documents.remove(0);
        let path = reqwest::Url::parse(open.params["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PARAGRAPH_JA);
        client.send(Response::new_ok(open.id, json!({"success":true})).into());
        client.change(uri, 2, json!([{"text":format!(" {source}")}]));
        let action = document_action(&mut client, uri, range);
        let id = client.execute(action);
        let message = client.receive(id, TIMEOUT).unwrap_err().message;
        assert!(message.contains("translation_limits.input_kib.document"));
        assert!(message.contains(&(size + 1).to_string()));
        assert!(client.documents.is_empty());
        client.finish(true);
        assert!(!path.exists());
    }
}

#[test]
fn document_execution_deadline_starts_after_queue_waiting() {
    let mut client = Client::configured_documents(
        &["--mock-delay-ms", "1500"],
        false,
        false,
        json!({"selection_timeout_ms":3000,"document_timeout_ms":1000,"features":{"automatic_selection":false}}),
        true,
    );
    let mut blockers = Vec::new();
    for uri in ["file:///first.md", "file:///second.md"] {
        client.open(uri, "markdown", PARAGRAPH, 1);
        let actions = client.actions(
            uri,
            json!({"start":{"line":0,"character":0},"end":{"line":1,"character":40}}),
        );
        let command = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["command"]["command"] == "translation-lsp.preview")
            .unwrap()["command"]
            .clone();
        blockers.push(client.execute(command));
    }
    let uri = "file:///queued.md";
    client.open(uri, "markdown", PARAGRAPH, 1);
    let action = document_action(
        &mut client,
        uri,
        json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}),
    );
    let admitted = Instant::now();
    let request = client.execute(action);
    assert!(client.receive(request, TIMEOUT).unwrap().is_null());
    for id in blockers {
        assert!(client.receive(id, TIMEOUT).unwrap().is_null());
    }
    let failure = loop {
        let notice = client.notice();
        if notice["type"] == 1 {
            break notice;
        }
    };
    assert!(
        failure["message"]
            .as_str()
            .unwrap()
            .contains("Translation timed out")
    );
    assert!(
        admitted.elapsed() >= Duration::from_millis(2300),
        "Queue time must not consume the execution budget"
    );
    assert!(client.hover(uri, json!({"line":0,"character":4})).is_null());
    assert!(client.documents.is_empty());
    client.finish(true);
}

#[test]
fn csharp_xml_hover_preserves_tags_utf16_ranges_and_manual_selection() {
    let mut client = Client::new(&[]);
    let xml = "/// <summary>\n    /// Retry only if the remote service\n    /// reports a temporary failure.\n    /// </summary>";
    let translated = format!("<summary>\n{COMMENT_JA}\n</summary>");
    for ending in ["\n", "\r\n"] {
        let source = sample!("comment.cs").replace('\n', ending);
        let original = xml.replace('\n', ending);
        let range = range_for(&source, &original);
        let uri = "file:///Example.cs";
        client.open(uri, "csharp", &source, 1);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            &original,
            &translated,
            &range,
        );
        let action = client.action(uri, range.clone());
        let id = client.execute(action);
        assert!(client.receive(id, TIMEOUT).unwrap().is_null());
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            &original,
            &translated,
            &range,
        );
        client.close(uri);
    }
    let source = format!("var emoji = \"🚀\"; /* {PARAGRAPH} */");
    let selected = format!("/* {PARAGRAPH} */");
    let range = range_for(&source, &selected);
    client.open("untitled:Example", "csharp", &source, 1);
    assert_preview(
        &client.hover("untitled:Example", range["start"].clone()),
        &selected,
        PARAGRAPH_JA,
        &range,
    );
    client.finish(true);
}

#[test]
fn csharp_pending_hover_obeys_cancel_edit_and_close() {
    for mode in ["cancel", "edit", "close"] {
        let mut client = Client::new(&["--mock-delay-ms", "150"]);
        let uri = "file:///Pending.cs";
        let source = format!("class A {{\n    {COMMENT}\n}}");
        let range = range_for(&source, COMMENT);
        client.open(uri, "csharp", &source, 1);
        let id = client.request_hover(uri, range["start"].clone());
        match mode {
            "cancel" => client.notify("$/cancelRequest", json!({"id":id})),
            "edit" => client.change(uri, 2, json!([{"text":"class A {}"}])),
            "close" => client.close(uri),
            _ => unreachable!(),
        }
        assert_error(
            client.receive(id, TIMEOUT),
            if mode == "cancel" { -32800 } else { -32801 },
        );
        client.close(uri);
        client.open(uri, "csharp", "var s = @\"// Not a comment.\";", 1);
        thread::sleep(Duration::from_millis(180));
        assert!(
            client
                .hover(uri, json!({"line":0,"character":12}))
                .is_null()
        );
        assert!(client.notifications.is_empty());
        client.finish(true);
    }
}

#[test]
fn c_family_doxygen_hover_and_selection_share_the_comment_body() {
    let mut client = Client::new(&[]);
    for (language, extension) in [("c", "c"), ("cpp", "cpp")] {
        let uri = format!("file:///retry.{extension}");
        for ending in ["\n", "\r\n", "\r"] {
            let source = sample!("comment.c").replace('\n', ending);
            let original =
                "/// @brief Retry only if the remote service\n/// reports a temporary failure."
                    .replace('\n', ending);
            let range = json!({"start":{"line":0,"character":0},"end":{"line":1,"character":32}});
            client.open(&uri, language, &source, 1);
            assert_preview(
                &client.hover(&uri, range["start"].clone()),
                &original,
                &format!("@brief {COMMENT_JA}"),
                &range,
            );
            let action = client.action(&uri, range.clone());
            let id = client.execute(action);
            assert!(client.receive(id, TIMEOUT).unwrap().is_null());
            client.change(
                &uri,
                2,
                json!([{"text":"const char *s = \"// Not a comment\";"}]),
            );
            assert!(
                client
                    .hover(&uri, json!({"line":0,"character":20}))
                    .is_null()
            );
            client.close(&uri);
        }
    }
    client.finish(true);
}

#[test]
fn python_docstring_and_comment_hover_use_canonical_bodies() {
    let mut client = Client::new(&[]);
    let uri = "file:///retry.py";
    for ending in ["\n", "\r\n", "\r"] {
        let source = sample!("comment.py").replace('\n', ending);
        let original =
            "\"\"\"Retry only if the remote service\n    reports a temporary failure.\n    \"\"\""
                .replace('\n', ending);
        let range = json!({"start":{"line":1,"character":4},"end":{"line":3,"character":7}});
        client.open(uri, "python", &source, 1);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            &original,
            COMMENT_JA,
            &range,
        );
        let action = client.action(uri, range.clone());
        let id = client.execute(action);
        assert!(client.receive(id, TIMEOUT).unwrap().is_null());
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            &original,
            COMMENT_JA,
            &range,
        );
        let original = "# Retry only if the remote service\n    # reports a temporary failure."
            .replace('\n', ending);
        let range = json!({"start":{"line":4,"character":4},"end":{"line":5,"character":34}});
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            &original,
            COMMENT_JA,
            &range,
        );
        client.close(uri);
    }
    client.finish(true);
}

#[test]
fn json_yaml_and_toml_samples_translate_their_comment_groups() {
    let mut client = Client::new(&[]);
    for (uri, language, source) in [
        ("file:///comment.json", "json", sample!("comment.jsonc")),
        ("file:///comment.jsonc", "jsonc", sample!("comment.jsonc")),
        ("file:///comment.yaml", "yaml", sample!("comment.yaml")),
        ("file:///comment.toml", "toml", sample!("comment.toml")),
        ("file:///comment.sh", "shellscript", sample!("comment.sh")),
        ("file:///comment.ps1", "powershell", sample!("comment.ps1")),
        ("file:///Dockerfile", "dockerfile", sample!("Dockerfile")),
        (
            "file:///compose.yaml",
            "dockercompose",
            sample!("compose.yaml"),
        ),
    ] {
        for ending in ["\n", "\r\n", "\r"] {
            let source = source.replace('\n', ending);
            let start = source.find(['/', '#']).unwrap();
            let end = source.find("failure.").unwrap() + "failure.".len();
            let original = &source[start..end];
            let range = range_for(
                &source.replace(ending, "\n"),
                &original.replace(ending, "\n"),
            );
            client.open(uri, language, &source, 1);
            for point in ["start", "end"] {
                assert_preview(
                    &client.hover(uri, range[point].clone()),
                    original,
                    COMMENT_JA,
                    &range,
                );
            }
            let key = source.find("retry_count").unwrap();
            assert!(
                client
                    .hover(uri, position(&source[..key].replace(ending, "\n")))
                    .is_null()
            );
            client.close(uri);
        }
    }
    client.finish(true);
}

#[test]
fn shell_hover_ranges_count_utf16_units_after_japanese_and_emoji() {
    let mut client = Client::new(&[]);
    let uri = "file:///utf16.sh";
    let inline = "# 日本語のコメント😀";
    let comment = COMMENT.replace("//", "#");
    let source = format!(
        "greeting=\"日本語😀\"; answer=$(echo \"é😀\") {inline}\n    {comment}\necho \"$greeting\"\n"
    );
    client.open(uri, "shellscript", &source, 1);
    let range = range_for(&source, &comment);
    for point in ["start", "end"] {
        assert_preview(
            &client.hover(uri, range[point].clone()),
            &comment,
            COMMENT_JA,
            &range,
        );
    }
    // The mock translates the sample text only. The range of the comment it
    // refuses comes back as the retry target.
    let range = range_for(&source, inline);
    // 50 bytes and 37 characters come before the comment.
    assert_eq!(range["start"]["character"], 39);
    assert_eq!(range["end"]["character"], 51);
    let hover = client.request_hover(uri, range["end"].clone());
    assert_error(client.receive(hover, TIMEOUT), -32803);
    let actions = client.actions(uri, json!({"start": range["start"], "end": range["start"]}));
    let retry = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["command"]["command"] == "translation-lsp.retry")
        .unwrap();
    assert_eq!(retry["command"]["arguments"][0]["range"], range);
    client.finish(true);
}

#[test]
fn python_pending_docstring_obeys_cancel_edit_and_close() {
    for mode in ["cancel", "edit", "close"] {
        let mut client = Client::new(&["--mock-delay-ms", "150"]);
        let uri = "file:///Pending.py";
        client.open(uri, "python", sample!("comment.py"), 1);
        let id = client.request_hover(uri, json!({"line":1,"character":10}));
        match mode {
            "cancel" => client.notify("$/cancelRequest", json!({"id":id})),
            "edit" => client.change(uri, 2, json!([{"text":"pass"}])),
            "close" => client.close(uri),
            _ => unreachable!(),
        }
        assert_error(
            client.receive(id, TIMEOUT),
            if mode == "cancel" { -32800 } else { -32801 },
        );
        client.close(uri);
        client.open(uri, "python", "value = '''Not a docstring.'''", 1);
        thread::sleep(Duration::from_millis(180));
        assert!(
            client
                .hover(uri, json!({"line":0,"character":15}))
                .is_null()
        );
        assert!(client.notifications.is_empty());
        client.finish(true);
    }
}

#[test]
fn comment_hover_response_cancellation_preserves_work_until_explicit_cancel_or_completion() {
    let mut client = Client::new(&["--mock-delay-ms", "300"]);
    let uri = "file:///retained-comments.rs";
    let range = range_for(COMMENT_DOCUMENT, COMMENT);
    client.open(uri, "rust", COMMENT_DOCUMENT, 1);
    let hover = client.request_hover(uri, range["start"].clone());
    client.notify("$/cancelRequest", json!({"id":hover}));
    assert_error(client.receive(hover, TIMEOUT), -32800);
    let actions = client.actions(uri, range.clone());
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["command"]["command"] == "translation-lsp.cancel-range")
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while client
        .actions(uri, range.clone())
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["command"]["command"] == "translation-lsp.cancel-range")
    {
        assert!(Instant::now() < deadline, "Retained comment did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    let revisit = client.request_hover(uri, range["start"].clone());
    assert_preview(
        &client.receive(revisit, Duration::from_millis(200)).unwrap(),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    assert!(client.notifications.is_empty());
    for (version, command) in [
        (2, "translation-lsp.cancel-range"),
        (3, "translation-lsp.cancel-all"),
    ] {
        client.change(uri, version, json!([{"text": COMMENT_DOCUMENT}]));
        let hover = client.request_hover(uri, range["start"].clone());
        client.notify("$/cancelRequest", json!({"id":hover}));
        assert_error(client.receive(hover, TIMEOUT), -32800);
        let actions = client.actions(uri, range.clone());
        let cancel = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["command"]["command"] == command)
            .unwrap()["command"]
            .clone();
        let cancel = client.execute(cancel);
        assert!(client.receive(cancel, TIMEOUT).unwrap().is_null());
        assert!(
            !client
                .actions(uri, range.clone())
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["command"]["command"] == "translation-lsp.cancel-range")
        );
    }
    client.finish(true);
}

#[test]
fn failed_ranges_offer_cursor_retry_and_reject_stale_retry_actions() {
    for (language, source, original) in [
        ("rust", COMMENT_DOCUMENT, COMMENT),
        ("markdown", MARKDOWN_DOCUMENT, PARAGRAPH),
    ] {
        let mut client = Client::new(&["--mock-fail"]);
        let uri = "file:///retry.txt";
        client.open(uri, language, source, 1);
        let range = range_for(source, original);
        if language == "rust" {
            let id = client.request_hover(uri, range["start"].clone());
            assert_error(client.receive(id, TIMEOUT), -32803);
        } else {
            let action = client.action(uri, range.clone());
            let id = client.execute(action);
            assert!(client.receive(id, TIMEOUT).unwrap().is_null());
            assert_eq!(client.notice()["type"], 1);
        }
        let cursor = json!({"start":range["start"],"end":range["start"]});
        let actions = client.actions(uri, cursor.clone());
        let retry = actions
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["command"]["command"] == "translation-lsp.retry")
            .unwrap()["command"]
            .clone();
        assert_eq!(retry["arguments"][0]["range"], range);
        let id = client.execute(retry.clone());
        assert!(client.receive(id, TIMEOUT).unwrap().is_null());
        assert_eq!(client.notice()["type"], 1);
        assert!(
            client.hover(uri, range["start"].clone())["contents"]["value"]
                .as_str()
                .unwrap()
                .contains("Retry translation of lines")
        );
        client.change(uri, 2, json!([{"text":source}]));
        let id = client.execute(retry);
        assert_error(client.receive(id, TIMEOUT), -32801);
        assert!(
            !client
                .actions(uri, cursor)
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["command"]["command"] == "translation-lsp.retry")
        );
        client.finish(true);
    }
}

#[test]
fn features_errors_name_the_language_or_the_feature_at_initialization() {
    let generic = "Invalid translation settings: check field names and value types";
    for (options, message) in [
        (json!({"features": {"unknown": true}}), generic),
        (json!({"features": {"comment_hover": "false"}}), generic),
        (json!({"features": {"languages": []}}), generic),
        (json!({"markdown_auto_translate": false}), generic),
        (
            json!({"features": {"languages": {"javascriptreact": {"comment_hover": false}}}}),
            "features.languages keys must be one of rust, javascript, typescript, \
             typescriptreact, csharp, c, cpp, python, json, jsonc, yaml, toml, shellscript, \
             powershell, dockerfile, dockercompose, markdown, plaintext",
        ),
        (
            json!({"features": {"languages": {"rust": {"document_action": false}}}}),
            "features.languages.rust.document_action is not available; document_action applies \
             to markdown, plaintext",
        ),
    ] {
        let mut client = Client::spawn(&[], false, false);
        let error = client
            .call(
                "initialize",
                json!({"capabilities": {}, "initializationOptions": options}),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, message);
        client.notify("exit", Value::Null);
        assert!(!client.child.wait().unwrap().success());
    }
}

fn commands(actions: &Value) -> Vec<&str> {
    actions
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["command"]["command"].as_str().unwrap())
        .collect()
}

fn refuse(client: &mut Client, command: Value, feature: &str) {
    let refused = client.execute(command);
    let error = client.receive(refused, TIMEOUT).unwrap_err();
    assert_eq!(error.code, -32602);
    assert_eq!(
        error.message,
        format!("The features setting turns off {feature} for this language")
    );
    assert_eq!(
        client.notifications,
        vec![json!({"type": 1, "message": error.message})]
    );
    client.notifications.clear();
}

fn ready(client: &mut Client, command: Value) {
    let accepted = client.execute(command);
    assert!(client.receive(accepted, TIMEOUT).unwrap().is_null());
    let notice = client.notice();
    assert_eq!(notice["type"], 3);
    assert!(notice["message"].as_str().unwrap().ends_with(READY));
}

fn opened(client: &mut Client, command: Value) {
    let accepted = client.execute(command);
    while client.documents.is_empty() {
        client.pump();
    }
    let open = client.documents.remove(0);
    client.send(Response::new_ok(open.id, json!({"success": true})).into());
    assert!(client.receive(accepted, TIMEOUT).unwrap().is_null());
}

#[test]
fn comment_hover_switch_stops_new_hover_work_and_keeps_manual_results() {
    for features in [
        json!({"languages": {"yaml": {"comment_hover": false}}}),
        json!({"comment_hover": false, "languages": {"toml": {"comment_hover": true}}}),
    ] {
        let mut client = Client::configured(
            &["--mock-delay-ms", "150"],
            false,
            false,
            json!({"features": features}),
        );
        let comment = |source: &'static str| {
            let comment = &source[..source.find("failure.").unwrap() + "failure.".len()];
            (comment, range_for(source, comment))
        };
        let uri = "file:///switch.yaml";
        let source = sample!("comment.yaml");
        let (original, range) = comment(source);
        client.open(uri, "yaml", source, 1);
        assert!(client.hover(uri, range["start"].clone()).is_null());
        // Work started by that Hover would still be running, and offer its cancellation.
        let actions = client.actions(uri, range.clone());
        assert_eq!(commands(&actions), ["translation-lsp.preview"]);
        // A Hover during the manual translation joins it, and later ones read its result.
        let command = client.execute(actions[0]["command"].clone());
        let joined = client.request_hover(uri, range["start"].clone());
        assert!(client.receive(command, TIMEOUT).unwrap().is_null());
        assert_preview(
            &client.receive(joined, TIMEOUT).unwrap(),
            original,
            COMMENT_JA,
            &range,
        );
        assert!(
            client.notice()["message"]
                .as_str()
                .unwrap()
                .ends_with(READY)
        );
        assert_preview(
            &client.hover(uri, range["end"].clone()),
            original,
            COMMENT_JA,
            &range,
        );
        client.change(uri, 2, json!([{ "text": source }]));
        assert!(client.hover(uri, range["start"].clone()).is_null());

        let uri = "file:///switch.toml";
        let source = sample!("comment.toml");
        let (original, range) = comment(source);
        client.open(uri, "toml", source, 1);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            original,
            COMMENT_JA,
            &range,
        );
        client.finish(true);
    }
}

#[test]
fn selection_action_switch_hides_the_action_and_refuses_its_command() {
    for features in [
        json!({"languages": {"markdown": {"selection_action": false}}}),
        json!({
            "selection_action": false,
            "languages": {
                "plaintext": {"selection_action": true},
                "rust": {"selection_action": true}
            }
        }),
    ] {
        let mut client =
            Client::configured_documents(&[], false, false, json!({"features": features}), true);
        let range = range_for(PARAGRAPH, PARAGRAPH);
        let uri = "file:///switch.md";
        client.open(uri, "markdown", PARAGRAPH, 1);
        let actions = client.actions(uri, range.clone());
        assert_eq!(commands(&actions), ["translation-lsp.open-translation"]);
        // The tab action carries the target that the hidden action would.
        let mut command = actions[0]["command"].clone();
        command["command"] = json!("translation-lsp.preview");
        refuse(&mut client, command.clone(), "selection_action");
        assert!(client.hover(uri, range["start"].clone()).is_null());
        // The tab action and the retry command of the same language are unchanged.
        opened(&mut client, actions[0]["command"].clone());
        assert!(client.hover(uri, range["start"].clone()).is_null());
        command["command"] = json!("translation-lsp.retry");
        ready(&mut client, command);
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            PARAGRAPH,
            PARAGRAPH_JA,
            &range,
        );

        let uri = "file:///switch.txt";
        client.open(uri, "plaintext", PARAGRAPH, 1);
        let actions = client.actions(uri, range.clone());
        assert_eq!(
            commands(&actions),
            [
                "translation-lsp.preview",
                "translation-lsp.open-translation"
            ]
        );
        ready(&mut client, actions[0]["command"].clone());
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            PARAGRAPH,
            PARAGRAPH_JA,
            &range,
        );

        let uri = "file:///switch.rs";
        let range = range_for(COMMENT_DOCUMENT, COMMENT);
        client.open(uri, "rust", COMMENT_DOCUMENT, 1);
        client.action(uri, range.clone());
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            COMMENT,
            COMMENT_JA,
            &range,
        );
        client.finish(true);
    }
}

#[test]
fn document_action_switch_hides_both_tab_actions_and_refuses_their_command() {
    for features in [
        json!({"languages": {"markdown": {"document_action": false}}}),
        json!({"document_action": false, "languages": {"plaintext": {"document_action": true}}}),
    ] {
        let mut client =
            Client::configured_documents(&[], false, false, json!({"features": features}), true);
        let cursor = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}});
        let range = range_for(PARAGRAPH, PARAGRAPH);
        let uri = "file:///switch.md";
        client.open(uri, "markdown", PARAGRAPH, 1);
        assert_eq!(client.actions(uri, cursor.clone()), json!([]));
        let actions = client.actions(uri, range.clone());
        assert_eq!(commands(&actions), ["translation-lsp.preview"]);
        // The selection action carries the target that the hidden action would.
        let mut command = actions[0]["command"].clone();
        command["command"] = json!("translation-lsp.open-translation");
        refuse(&mut client, command, "document_action");
        assert!(client.documents.is_empty());
        // The selection action of the same language is unchanged.
        ready(&mut client, actions[0]["command"].clone());
        assert_preview(
            &client.hover(uri, range["start"].clone()),
            PARAGRAPH,
            PARAGRAPH_JA,
            &range,
        );

        let uri = "file:///switch.txt";
        client.open(uri, "plaintext", PARAGRAPH, 1);
        let titles = |actions: Value| -> Vec<String> {
            actions
                .as_array()
                .unwrap()
                .iter()
                .map(|action| action["title"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            titles(client.actions(uri, range.clone())),
            [
                "Translate selection to ja",
                "Translate selection to ja and open in tab"
            ]
        );
        let actions = client.actions(uri, cursor);
        assert_eq!(
            titles(actions.clone()),
            ["Translate document to ja and open in tab"]
        );
        opened(&mut client, actions[0]["command"].clone());
        client.finish(true);
    }
}

const NOT_RECOGNIZED: &str = "Comments in this file are not recognized until you reopen it.";

fn timed_hover(client: &mut Client, uri: &str, position: Value) -> (Value, Duration) {
    let started = Instant::now();
    let hover = client.hover(uri, position);
    (hover, started.elapsed())
}

#[test]
fn comment_parse_timeout_errors_take_the_form_of_the_other_timing_settings() {
    for (value, message) in [
        (
            json!(99),
            "comment_parse_timeout_ms must be between 100 and 60000; received 99",
        ),
        (
            json!(60001),
            "comment_parse_timeout_ms must be between 100 and 60000; received 60001",
        ),
        (
            json!("1000"),
            "Invalid translation settings: check field names and value types",
        ),
        (
            json!(1000.5),
            "Invalid translation settings: check field names and value types",
        ),
    ] {
        let mut client = Client::spawn(&[], false, false);
        let error = client
            .call(
                "initialize",
                json!({"capabilities": {}, "initializationOptions": {"comment_parse_timeout_ms": value}}),
            )
            .unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, message);
        client.notify("exit", Value::Null);
        assert!(!client.child.wait().unwrap().success());
    }
    for value in [100, 60000] {
        let mut client = Client::configured(
            &[],
            false,
            false,
            json!({"comment_parse_timeout_ms": value}),
        );
        client.finish(true);
    }
}

#[test]
fn a_reading_past_its_limit_leaves_the_document_without_comments_until_it_is_reopened() {
    let limit = Duration::from_millis(100);
    let comment = "# Retry only if the remote service\n# reports a temporary failure.";
    let range = range_for(comment, comment);
    // Reading this much text takes seconds in a debug build.
    let slow = format!("{comment}\n{}", "value = 1\n".repeat(400_000));
    // The text is read, and the failure reported, with comment Hover off too.
    for comment_hover in [true, false] {
        let mut client = Client::configured(
            &[],
            false,
            false,
            json!({"comment_parse_timeout_ms": 100, "features": {"comment_hover": comment_hover}}),
        );
        let uri = "file:///work/slow%20%E8%AA%AD.py";
        client.open(uri, "python", &slow, 1);
        let (hover, elapsed) = timed_hover(&mut client, uri, range["start"].clone());
        assert!(hover.is_null());
        assert!(elapsed >= limit, "{elapsed:?}");
        assert_eq!(
            client.notice(),
            json!({"type": 2, "message": format!(
                "Reading comments in slow 読.py timed out. {NOT_RECOGNIZED} To allow more time, adjust comment_parse_timeout_ms in Zed settings."
            )})
        );

        // Neither a later request nor an edit reads the text again. Reading it
        // would take the whole limit each time, so one quick answer is enough.
        let quick = |client: &mut Client| {
            (0..3).any(|_| {
                let (hover, elapsed) = timed_hover(client, uri, range["start"].clone());
                assert!(hover.is_null());
                elapsed < limit
            })
        };
        assert!(quick(&mut client));
        let start = json!({"line": 2, "character": 0});
        client.change(
            uri,
            2,
            json!([{"range": {"start": start, "end": start}, "text": "value = 1\n"}]),
        );
        assert!(quick(&mut client));

        // Other documents are read as before, and so is this one once reopened.
        let other = "file:///work/other.py";
        client.open(other, "python", comment, 1);
        client.close(uri);
        client.open(uri, "python", &format!("{comment}\nvalue = 1\n"), 1);
        for uri in [other, uri] {
            let hover = client.hover(uri, range["start"].clone());
            if comment_hover {
                assert_preview(&hover, comment, COMMENT_JA, &range);
            } else {
                assert!(hover.is_null());
            }
        }
        assert!(client.notifications.is_empty());
        assert_eq!(
            client.finish(false),
            "translation preview: reading comments timed out (language \"python\")\n"
        );
    }
}

#[test]
fn a_panic_while_reading_comments_keeps_the_server_running_and_the_text_out_of_the_log() {
    let mut client = Client::new(&[]);
    // ra-ap-rustc_lexer 0.177 panics on an opening of more than 65,535 dashes.
    let block = "/* Retry only if the remote service\nreports a temporary failure. */";
    let sound = format!("{COMMENT_DOCUMENT}{block}\n");
    let broken = format!("{}\n{sound}", "-".repeat(65_536));
    let uri = "file:///work/dashes.rs";
    client.open(uri, "rust", &broken, 1);

    // The first translation reads the comments. Its selection is translated
    // as it is written, which the mock knows for this comment.
    let range = range_for(&broken, COMMENT);
    let action = client.action(uri, range.clone());
    let execution = client.execute(action);
    assert!(client.receive(execution, TIMEOUT).unwrap().is_null());
    assert_eq!(
        client.notice(),
        json!({"type": 2, "message": format!("Reading comments in dashes.rs failed. {NOT_RECOGNIZED}")})
    );
    assert!(
        client.notice()["message"]
            .as_str()
            .unwrap()
            .ends_with(READY)
    );
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        COMMENT,
        COMMENT_JA,
        &range,
    );
    let block_start = range_for(&broken, block)["start"].clone();
    assert!(client.hover(uri, block_start).is_null());

    // A document that can be read is read as before.
    let other = "file:///work/other.rs";
    client.open(other, "rust", &sound, 1);
    let range = range_for(&sound, COMMENT);
    assert_preview(
        &client.hover(other, range["start"].clone()),
        COMMENT,
        COMMENT_JA,
        &range,
    );

    // The mock knows the block comment without its marks only: its body is
    // translated where the comment is recognized, and nothing where it is not.
    for (uri, text, recognized) in [(other, &sound, true), (uri, &broken, false)] {
        let action = client.action(uri, range_for(text, block));
        let execution = client.execute(action);
        assert!(client.receive(execution, TIMEOUT).unwrap().is_null());
        assert_eq!(client.notice()["type"], if recognized { 3 } else { 1 });
    }
    assert!(client.notifications.is_empty());

    let stderr = client.finish(false);
    let place = stderr
        .strip_prefix("translation preview: reading comments panicked (language \"rust\", at ")
        .and_then(|rest| rest.strip_suffix(")\n"))
        .unwrap_or_else(|| panic!("Unexpected stderr: {stderr}"));
    assert!(!place.contains('\n') && !place.contains("--"), "{place}");
    assert!(
        place.rsplit_once(':').unwrap().1.parse::<u32>().is_ok(),
        "{place}"
    );
}

#[test]
fn a_large_c_file_is_read_within_the_default_limit() {
    let mut client = Client::new(&[]);
    let unit = format!(
        "/**\n{} */\nint add_values(int left, int right);\n\n",
        " * Adds two values and returns their sum to the caller.\n".repeat(10)
    );
    let comment = "// Retry only if the remote service\n// reports a temporary failure.";
    // A debug build on a slow machine reads this in a fraction of the limit.
    let text = format!("{}{comment}\n", unit.repeat(256 * 1024 / unit.len()));
    assert!(text.len() > 250_000);
    let uri = "file:///work/large.c";
    client.open(uri, "c", &text, 1);
    let range = range_for(&text, comment);
    assert_preview(
        &client.hover(uri, range["start"].clone()),
        comment,
        COMMENT_JA,
        &range,
    );
    client.finish(true);
}
