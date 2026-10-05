use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

use lsp_server::{ErrorCode, Request, RequestId, Response, ResponseError};
use serde_json::{Value, json};

use crate::lsp::ui::Ui;

#[derive(Default)]
pub(in crate::lsp) struct PreviewFiles {
    files: Vec<PathBuf>,
    opening: HashMap<RequestId, Opening>,
}

struct Opening {
    command: RequestId,
    deadline: Instant,
}

impl PreviewFiles {
    pub fn open(
        &mut self,
        ui: &Ui,
        command: RequestId,
        text: &str,
        markdown: bool,
    ) -> Result<(), ResponseError> {
        let extension = if markdown { "md" } else { "txt" };
        let directory =
            std::env::temp_dir().join(format!("zed-translation-{}", uuid::Uuid::new_v4()));
        let mut directory_options = fs::DirBuilder::new();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            directory_options.mode(0o700);
            options.mode(0o600);
        }
        directory_options
            .create(&directory)
            .map_err(|_| failed("Could not create the translation preview directory"))?;
        let path = directory.join(format!("Translation.{extension}"));
        self.files.push(path.clone());
        let mut file = options
            .open(&path)
            .map_err(|_| failed("Could not create the translation preview file"))?;
        file.write_all(text.as_bytes())
            .map_err(|_| failed("Could not write the translation preview file"))?;
        let uri = reqwest::Url::from_file_path(&path)
            .map_err(|_| failed("Could not open the translation preview path"))?;
        let id = RequestId::from(format!("translation-document-{}", uuid::Uuid::new_v4()));
        ui.sender
            .send(
                Request::new(
                    id.clone(),
                    "window/showDocument".into(),
                    json!({"uri": uri.as_str(), "external": false, "takeFocus": true}),
                )
                .into(),
            )
            .map_err(|_| failed("The editor disconnected"))?;
        self.opening.insert(
            id,
            Opening {
                command,
                deadline: Instant::now() + Duration::from_secs(10),
            },
        );
        Ok(())
    }

    pub fn contains(&self, command: &RequestId) -> bool {
        self.opening.values().any(|open| &open.command == command)
    }

    pub fn clear(&mut self) {
        for path in self.files.drain(..) {
            let _ = fs::remove_file(&path);
            if let Some(directory) = path.parent() {
                let _ = fs::remove_dir(directory);
            }
        }
    }

    pub fn response(&mut self, ui: &Ui, response: &Response) -> bool {
        let Some(open) = self.opening.remove(&response.id) else {
            return false;
        };
        let result = match &response.response_result {
            Ok(value) if value["success"] == true => Ok(Value::Null),
            _ => Err(failed("The editor could not open the translation preview")),
        };
        reply(ui, open.command, result);
        true
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.opening.values().map(|open| open.deadline).min()
    }

    pub fn expire(&mut self, ui: &Ui) {
        let expired: Vec<_> = self
            .opening
            .values()
            .filter(|open| open.deadline <= Instant::now())
            .map(|open| open.command.clone())
            .collect();
        for command in expired {
            self.cancel(ui, Some(&command), ErrorCode::RequestFailed);
        }
    }

    pub fn cancel(&mut self, ui: &Ui, command: Option<&RequestId>, code: ErrorCode) {
        self.opening.retain(|id, open| {
            if command.is_some_and(|command| command != &open.command) {
                return true;
            }
            let _ = ui.sender.send(
                lsp_server::Notification::new("$/cancelRequest".into(), json!({"id": id})).into(),
            );
            reply(
                ui,
                open.command.clone(),
                Err(ResponseError {
                    code: code as i32,
                    message: if matches!(code, ErrorCode::RequestFailed) {
                        "Opening the translation preview timed out"
                    } else {
                        "Opening the translation preview was canceled"
                    }
                    .into(),
                    data: None,
                }),
            );
            false
        });
    }
}

fn failed(message: &str) -> ResponseError {
    ResponseError {
        code: ErrorCode::RequestFailed as i32,
        message: message.into(),
        data: None,
    }
}

fn reply(ui: &Ui, id: RequestId, result: Result<Value, ResponseError>) {
    if let Err(error) = &result
        && error.code != ErrorCode::RequestCanceled as i32
    {
        let _ = ui.error(&error.message);
    }
    let _ = ui.sender.send(
        Response {
            id,
            response_result: result,
        }
        .into(),
    );
}

impl Drop for PreviewFiles {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use lsp_server::{Connection, Message};

    use super::*;

    #[test]
    fn cancel_and_timeout_answer_once_and_ignore_late_editor_acknowledgements() {
        for timeout in [false, true] {
            let (server, client) = Connection::memory();
            let ui = Ui::new(server.sender, Default::default());
            let mut files = PreviewFiles::default();
            files.open(&ui, 1.into(), "translation", true).unwrap();
            let Message::Request(request) = client.receiver.recv().unwrap() else {
                panic!("Expected showDocument");
            };
            if timeout {
                files.opening.get_mut(&request.id).unwrap().deadline = Instant::now();
                files.expire(&ui);
            } else {
                files.cancel(&ui, Some(&1.into()), ErrorCode::RequestCanceled);
            }
            let messages: Vec<_> = client.receiver.try_iter().collect();
            assert!(matches!(&messages[0], Message::Notification(cancel)
                if cancel.method == "$/cancelRequest" && cancel.params["id"] == json!(request.id)));
            if timeout {
                assert!(matches!(&messages[1], Message::Notification(notice)
                    if notice.method == "window/showMessage" && notice.params["type"] == 1));
            }
            assert_eq!(messages.len(), if timeout { 3 } else { 2 });
            let Message::Response(response) = messages.last().unwrap() else {
                panic!("Expected command response");
            };
            assert_eq!(response.id, 1.into());
            assert_eq!(
                response.response_result.as_ref().unwrap_err().code,
                if timeout {
                    ErrorCode::RequestFailed
                } else {
                    ErrorCode::RequestCanceled
                } as i32
            );
            assert!(files.deadline().is_none());
            assert!(!files.contains(&1.into()));
            assert!(!files.response(&ui, &Response::new_ok(request.id, json!({"success":true}))));
            assert!(client.receiver.try_recv().is_err());
        }
    }
}
