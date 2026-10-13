// The server from `initialize` on, as `run_stdio` runs it. Only the directory
// of the key file is the test's own. It never holds a key here: this server
// sends its requests to the real API.
use std::{fs, path::Path};

use serde_json::{Value, json};

use crate::{lsp::server::*, providers::anthropic::test_support::KeyDirectory};

const TIMEOUT: Duration = Duration::from_secs(8);

struct Client {
    connection: Connection,
    server: thread::JoinHandle<ServerResult<()>>,
    notifications: Vec<lsp_server::Notification>,
    next_id: i32,
}

impl Client {
    fn start(directory: &Path) -> Self {
        let (connection, client) = Connection::memory();
        let directory = directory.to_owned();
        let server =
            thread::spawn(move || serve(&connection, MockOptions::default(), Ok(directory)));
        Self {
            connection: client,
            server,
            notifications: Vec::new(),
            next_id: 0,
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.connection
            .sender
            .send(lsp_server::Notification::new(method.into(), params).into())
            .unwrap();
    }

    fn receive(&mut self) -> Option<Response> {
        match self.connection.receiver.recv_timeout(TIMEOUT).unwrap() {
            Message::Response(response) => Some(response),
            Message::Notification(notification) => {
                self.notifications.push(notification);
                None
            }
            Message::Request(request) => panic!("Unexpected request: {}", request.method),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> RequestResult<Value> {
        self.next_id += 1;
        self.connection
            .sender
            .send(Request::new(self.next_id.into(), method.into(), params).into())
            .unwrap();
        loop {
            if let Some(response) = self.receive() {
                assert_eq!(response.id, self.next_id.into());
                return response.response_result;
            }
        }
    }

    fn initialize(&mut self, provider: Value) -> Value {
        let initialized = self
            .call(
                "initialize",
                json!({"capabilities": {}, "initializationOptions": {"provider": provider}}),
            )
            .unwrap();
        self.notify("initialized", json!({}));
        initialized
    }

    fn notice(&mut self) -> lsp_server::Notification {
        while self.notifications.is_empty() {
            assert!(self.receive().is_none(), "Unexpected response");
        }
        self.notifications.remove(0)
    }

    fn finish(mut self) {
        assert!(self.call("shutdown", Value::Null).unwrap().is_null());
        self.notify("exit", Value::Null);
        self.server.join().unwrap().unwrap();
        assert!(self.notifications.is_empty(), "Unconsumed notifications");
    }
}

#[test]
fn anthropic_without_a_key_file_starts_and_reports_the_missing_file() {
    let directory = KeyDirectory::new();
    let mut client = Client::start(&directory.path);
    let initialized = client.initialize(json!({"name": "anthropic"}));
    assert_eq!(
        initialized["serverInfo"]["name"],
        "Translation LSP (Anthropic)"
    );
    assert_eq!(
        initialized["capabilities"]["executeCommandProvider"]["commands"],
        json!([
            COMMAND,
            RETRY_COMMAND,
            DOCUMENT_COMMAND,
            CANCEL_COMMAND,
            CANCEL_ALL_COMMAND,
            "translation-lsp.save-anthropic-key",
            "translation-lsp.remove-anthropic-key",
            SETTINGS_COMMAND
        ])
    );
    let uri = "file:///missing-key.md";
    let text = "Retry only after a temporary failure.";
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {
            "uri": uri, "languageId": "markdown", "version": 1, "text": text
        }}),
    );
    let start = json!({"line": 0, "character": 0});
    let range = json!({"start": start, "end": {"line": 0, "character": text.len()}});
    let actions = client
        .call(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri}, "range": range, "context": {"diagnostics": []}
            }),
        )
        .unwrap();
    // Without a key, the menu that saves one is the only action.
    assert_eq!(
        actions,
        json!([{
            "title": "Anthropic API key…",
            "kind": ACTION_KIND,
            "command": {"title": "Anthropic API key…", "command": SETTINGS_COMMAND}
        }])
    );
    // A translation command that the editor still holds is refused.
    let error = client
        .call(
            "workspace/executeCommand",
            json!({"command": COMMAND, "arguments": [{
                "uri": uri, "version": 1, "session": 1, "range": range, "targetLanguage": "ja"
            }]}),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RequestFailed as i32);
    assert_eq!(
        error.message,
        "Anthropic API key is not saved. Run \"Anthropic API key…\" from the code actions to save it"
    );
    let notice = client.notice();
    assert_eq!(notice.method, "window/showMessage");
    assert_eq!(notice.params["type"], 1);
    assert_eq!(notice.params["message"], error.message);
    let hover = client.call(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": start}),
    );
    assert!(hover.unwrap().is_null());
    client.finish();
    // The server created neither the file nor anything beside it.
    assert!(fs::read_dir(&directory.path).unwrap().next().is_none());
}

#[test]
fn anthropic_starts_when_thinking_is_omitted_at_every_effort() {
    let directory = KeyDirectory::new();
    for provider in [
        json!({"name": "anthropic", "effort": "xhigh"}),
        json!({"name": "anthropic", "effort": "max"}),
        json!({"name": "anthropic", "model": "synthetic-model"}),
        json!({"name": "anthropic", "model": "synthetic-model", "effort": "max"}),
    ] {
        let mut client = Client::start(&directory.path);
        let initialized = client.initialize(provider);
        assert_eq!(
            initialized["serverInfo"]["name"],
            "Translation LSP (Anthropic)"
        );
        client.finish();
    }
}
