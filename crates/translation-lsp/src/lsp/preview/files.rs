use super::Preview;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use lsp_server::{ErrorCode, Request, RequestId, Response, ResponseError};
use serde_json::json;

use crate::lsp::ui::Ui;

const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(in crate::lsp) struct PreviewFiles {
    files: Vec<PathBuf>,
    reusable: HashMap<(uuid::Uuid, bool), PathBuf>,
    opening: HashMap<RequestId, Instant>,
}

impl PreviewFiles {
    pub fn open(
        &mut self,
        ui: &Ui,
        preview: &Preview,
        markdown: bool,
    ) -> Result<(), ResponseError> {
        let key = (preview.id, markdown);
        let path = match self.reusable.get(&key) {
            Some(path) if matches_contents(path, &preview.translation) => path.clone(),
            _ => {
                let path = self.create(&preview.translation, markdown)?;
                self.reusable.insert(key, path.clone());
                path
            }
        };
        self.show(ui, &path)
    }

    fn create(&mut self, text: &str, markdown: bool) -> Result<PathBuf, ResponseError> {
        let extension = if markdown { "md" } else { "txt" };
        let directory =
            std::env::temp_dir().join(format!("translation-lsp-{}", uuid::Uuid::new_v4()));
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
        Ok(path)
    }

    fn show(&mut self, ui: &Ui, path: &Path) -> Result<(), ResponseError> {
        let uri = reqwest::Url::from_file_path(path)
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
        self.opening.insert(id, Instant::now() + OPEN_TIMEOUT);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.reusable.clear();
        for path in self.files.drain(..) {
            let _ = fs::remove_file(&path);
            if let Some(directory) = path.parent() {
                let _ = fs::remove_dir(directory);
            }
        }
    }

    pub fn response(&mut self, ui: &Ui, response: &Response) -> bool {
        if self.opening.remove(&response.id).is_none() {
            return false;
        }
        if !matches!(&response.response_result, Ok(value) if value["success"] == true) {
            let _ = ui.error("The editor could not open the translation preview");
        }
        true
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.opening.values().copied().min()
    }

    pub fn expire(&mut self, ui: &Ui) {
        let now = Instant::now();
        self.opening.retain(|id, deadline| {
            if *deadline > now {
                return true;
            }
            cancel_open(ui, id);
            let _ = ui.error("Opening the translation preview timed out");
            false
        });
    }

    pub fn cancel_all(&mut self, ui: &Ui) {
        for (id, _) in self.opening.drain() {
            cancel_open(ui, &id);
        }
    }
}

// Compare bounded chunks; never overwrite a preview that the user edited.
fn matches_contents(path: &Path, text: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != text.len() as u64 {
        return false;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut buffer = [0u8; 8192];
    for expected in text.as_bytes().chunks(buffer.len()) {
        let actual = &mut buffer[..expected.len()];
        if file.read_exact(actual).is_err() || actual != expected {
            return false;
        }
    }
    // A concurrent append must not make a changed file look reusable.
    matches!(file.read(&mut buffer[..1]), Ok(0))
}

fn failed(message: &str) -> ResponseError {
    ResponseError {
        code: ErrorCode::RequestFailed as i32,
        message: message.into(),
        data: None,
    }
}

fn cancel_open(ui: &Ui, id: &RequestId) {
    let _ = ui
        .sender
        .send(lsp_server::Notification::new("$/cancelRequest".into(), json!({"id": id})).into());
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

    fn preview(text: &str) -> Preview {
        Preview::new(
            crate::providers::Context::Mock,
            lsp_types::Range::default(),
            text.into(),
            false,
        )
    }

    fn shown_path(client: &Connection) -> PathBuf {
        let Message::Request(request) = client.receiver.recv().unwrap() else {
            panic!("Expected showDocument")
        };
        reqwest::Url::parse(request.params["uri"].as_str().unwrap())
            .unwrap()
            .to_file_path()
            .unwrap()
    }

    #[test]
    fn cached_result_reuses_file_but_changed_missing_and_new_results_do_not() {
        let (server, client) = Connection::memory();
        let ui = Ui::new(server.sender, Default::default());
        let mut files = PreviewFiles::default();
        let result = preview("translation");
        files.open(&ui, &result, true).unwrap();
        let first = shown_path(&client);
        files.open(&ui, &result.clone(), true).unwrap();
        assert_eq!(shown_path(&client), first);
        assert_eq!(files.files.len(), 1);
        fs::write(&first, "edited text").unwrap();
        files.open(&ui, &result, true).unwrap();
        let second = shown_path(&client);
        assert_ne!(second, first);
        assert_eq!(fs::read_to_string(&first).unwrap(), "edited text");
        assert_eq!(fs::read_to_string(&second).unwrap(), "translation");
        fs::remove_file(&second).unwrap();
        files.open(&ui, &result, true).unwrap();
        let third = shown_path(&client);
        assert_ne!(third, second);
        files.open(&ui, &result, true).unwrap();
        assert_eq!(shown_path(&client), third);
        files.open(&ui, &preview("translation"), true).unwrap();
        assert_ne!(shown_path(&client), third);
        files.open(&ui, &result, false).unwrap();
        assert_eq!(shown_path(&client).extension().unwrap(), "txt");
        let owned = files.files.clone();
        drop(files);
        assert!(owned.iter().all(|path| !path.exists()));
    }

    #[test]
    fn cancel_and_timeout_ignore_late_editor_acknowledgements() {
        for timeout in [false, true] {
            let (server, client) = Connection::memory();
            let ui = Ui::new(server.sender, Default::default());
            let mut files = PreviewFiles::default();
            files.open(&ui, &preview("translation"), true).unwrap();
            let Message::Request(request) = client.receiver.recv().unwrap() else {
                panic!("Expected showDocument");
            };
            if timeout {
                *files.opening.get_mut(&request.id).unwrap() = Instant::now();
                files.expire(&ui);
            } else {
                files.cancel_all(&ui);
            }
            let messages: Vec<_> = client.receiver.try_iter().collect();
            assert!(matches!(&messages[0], Message::Notification(cancel)
                if cancel.method == "$/cancelRequest" && cancel.params["id"] == json!(request.id)));
            if timeout {
                assert!(matches!(&messages[1], Message::Notification(notice)
                    if notice.method == "window/showMessage" && notice.params["type"] == 1));
            }
            assert_eq!(messages.len(), if timeout { 2 } else { 1 });
            assert!(files.deadline().is_none());
            assert!(!files.response(&ui, &Response::new_ok(request.id, json!({"success":true}))));
            assert!(client.receiver.try_recv().is_err());
        }
    }
}
