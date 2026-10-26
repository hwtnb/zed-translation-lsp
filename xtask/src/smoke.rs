use std::{
    fs,
    io::{BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use lsp_server::{Message, Notification, Request, RequestId};
use serde_json::{Value, json};

use crate::archive::unpack;
use tempfile::NamedTempFile;

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn archive(path: &Path) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let (binary, source) = unpack(path, temporary.path())?;
    run(&binary, &source.version)?;
    println!("Packaged server passed initialize, mock Hover, and shutdown");
    Ok(())
}

fn run(binary: &Path, version: &str) -> Result<()> {
    let stderr = NamedTempFile::new()?;
    let mut command = Command::new(binary);
    for name in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
        command.env_remove(name);
    }
    let mut process = Process(
        command
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr.reopen()?)
            .spawn()
            .context("Starting packaged executable")?,
    );
    let mut input = process.0.stdin.take().context("Missing server stdin")?;
    let output = process.0.stdout.take().context("Missing server stdout")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(output);
        loop {
            match Message::read(&mut reader) {
                Ok(Some(message)) => {
                    if tx.send(Ok(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    });
    let mut send = |method: &str, params: Value, id: Option<i32>| -> Result<()> {
        let message = match id {
            Some(id) => Message::Request(Request::new(id.into(), method.into(), params)),
            None => Message::Notification(Notification::new(method.into(), params)),
        };
        message.write(&mut input)?;
        input.flush()?;
        Ok(())
    };
    let response = |id: i32| -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let message = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))??;
            if let Message::Response(response) = message
                && response.id == RequestId::from(id)
            {
                return response
                    .response_result
                    .map_err(|_| anyhow::anyhow!("Packaged server returned an LSP error"));
            }
        }
    };
    // The mock provider reads no credentials, so the packaged server opens
    // neither the key file nor the ChatGPT store of whoever runs this.
    send(
        "initialize",
        json!({
            "capabilities": {},
            "initializationOptions": {"provider": {"name": "mock"}}
        }),
        Some(1),
    )?;
    ensure!(
        response(1)?["serverInfo"]["version"] == version,
        "Packaged server reports another version"
    );
    send("initialized", json!({}), None)?;
    let uri = "untitled:release-smoke.rs";
    send(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "rust",
                "version": 1,
                "text": "// Retry only if the remote service\n// reports a temporary failure."
            }
        }),
        None,
    )?;
    send(
        "textDocument/hover",
        json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 4}}),
        Some(2),
    )?;
    ensure!(
        response(2)?
            .to_string()
            .contains("リモートサービスが一時的な障害を報告した場合に限り、再試行する。"),
        "Packaged server did not translate the mock comment"
    );
    send("shutdown", Value::Null, Some(3))?;
    response(3)?;
    send("exit", Value::Null, None)?;
    drop(input);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = process.0.try_wait()? {
            ensure!(
                status.success() && fs::metadata(stderr.path())?.len() == 0,
                "Packaged server did not exit cleanly"
            );
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "Packaged server did not exit before deadline"
        );
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
