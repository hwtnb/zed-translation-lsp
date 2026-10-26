use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use crossbeam_channel::Sender;
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{ClientCapabilities, MessageType};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::providers::{Interaction, error::Error};

/// Every request to the editor, whether a task awaits its response or the
/// server loop handles it, is registered in one [`Ui`].
#[derive(Clone)]
pub(crate) struct Ui {
    pub(super) sender: Sender<Message>,
    replies: Arc<Mutex<HashMap<RequestId, Waiter>>>,
    pub(super) capabilities: UiCapabilities,
}

/// What the server loop does with the response to one of its requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    CreateProgress,
    OpenDocument,
}

enum Waiter {
    Task(oneshot::Sender<Response>),
    Server(Purpose),
}

#[derive(Clone, Copy, Default)]
pub(crate) struct UiCapabilities {
    pub show_document: bool,
    pub work_done_progress: bool,
    pub server_status: bool,
}

impl From<&ClientCapabilities> for UiCapabilities {
    fn from(client: &ClientCapabilities) -> Self {
        Self {
            show_document: client
                .window
                .as_ref()
                .and_then(|window| window.show_document.as_ref())
                .is_some_and(|document| document.support),
            work_done_progress: client
                .window
                .as_ref()
                .is_some_and(|window| window.work_done_progress == Some(true)),
            server_status: client
                .experimental
                .as_ref()
                .is_some_and(|extra| extra["serverStatusNotification"] == true),
        }
    }
}

struct PendingUi {
    ui: Ui,
    id: RequestId,
}

impl Ui {
    pub fn new(sender: Sender<Message>, capabilities: UiCapabilities) -> Self {
        Self {
            sender,
            replies: Arc::default(),
            capabilities,
        }
    }

    /// Hands a response to the task that awaits it, or returns it with its
    /// purpose for the server loop. A response to no live request is dropped.
    pub fn response(&self, response: Response) -> Option<(Purpose, Response)> {
        let waiter = self.replies.lock().unwrap().remove(&response.id)?;
        match waiter {
            Waiter::Task(sender) => {
                let _ = sender.send(response);
                None
            }
            Waiter::Server(purpose) => Some((purpose, response)),
        }
    }

    /// Sends a request whose response [`Ui::response`] returns with `purpose`.
    pub(super) fn send_request(
        &self,
        id: RequestId,
        method: &str,
        params: Value,
        purpose: Purpose,
    ) -> Result<(), Error> {
        self.replies
            .lock()
            .unwrap()
            .insert(id.clone(), Waiter::Server(purpose));
        self.send(id, method, params)
    }

    /// Withdraws a request that is still waiting for its response.
    pub(super) fn cancel_request(&self, id: &RequestId) {
        if self.replies.lock().unwrap().remove(id).is_some() {
            let _ = self
                .sender
                .send(Notification::new("$/cancelRequest".into(), json!({"id": id})).into());
        }
    }

    fn send(&self, id: RequestId, method: &str, params: Value) -> Result<(), Error> {
        self.sender
            .send(
                Request {
                    id,
                    method: method.into(),
                    params,
                }
                .into(),
            )
            .map_err(|_| Error::failed("The editor disconnected"))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let id = RequestId::from(format!("translation-ui-{}", Uuid::new_v4()));
        let (sender, reply) = oneshot::channel();
        self.replies
            .lock()
            .unwrap()
            .insert(id.clone(), Waiter::Task(sender));
        let _pending = PendingUi {
            ui: self.clone(),
            id: id.clone(),
        };
        self.send(id, method, params)?;
        let response = reply
            .await
            .map_err(|_| Error::failed("The editor disconnected"))?;
        response
            .response_result
            .map_err(|_| Error::failed("The editor declined the operation"))
    }

    pub fn inform(&self, message: &str) -> Result<(), Error> {
        self.show_message(MessageType::INFO, message)
    }

    pub fn warn(&self, message: &str) -> Result<(), Error> {
        self.show_message(MessageType::WARNING, message)
    }

    pub fn error(&self, message: &str) -> Result<(), Error> {
        self.show_message(MessageType::ERROR, message)
    }

    /// Whether the editor reports that it showed `uri`.
    async fn show_document(&self, uri: &str, external: bool) -> Result<bool, Error> {
        let result = self
            .request(
                "window/showDocument",
                json!({"uri":uri,"external":external,"takeFocus":true}),
            )
            .await?;
        Ok(result["success"] == true)
    }

    fn show_message(&self, kind: MessageType, message: &str) -> Result<(), Error> {
        self.sender
            .send(
                Notification::new(
                    "window/showMessage".into(),
                    json!({"type":kind,"message":message}),
                )
                .into(),
            )
            .map_err(|_| Error::failed("The editor disconnected"))
    }
}

impl Drop for PendingUi {
    fn drop(&mut self) {
        self.ui.cancel_request(&self.id);
    }
}

impl Interaction for Ui {
    async fn open(&self, uri: &str) -> Result<(), Error> {
        if !self.capabilities.show_document {
            return Err(Error::failed(
                "The editor must support window/showDocument with external URLs",
            ));
        }
        if !self.show_document(uri, true).await? {
            return Err(Error::failed(
                "The editor could not open the requested page",
            ));
        }
        Ok(())
    }

    async fn edit(&self, path: &Path) -> Result<(), Error> {
        if !self.capabilities.show_document {
            return Err(Error::failed("The editor must support window/showDocument"));
        }
        let unopened = || Error::failed("The editor could not open the file");
        let uri = Url::from_file_path(path).map_err(|_| unopened())?;
        if !self.show_document(uri.as_str(), false).await? {
            return Err(unopened());
        }
        Ok(())
    }

    async fn choose(&self, message: &str, choices: &[String]) -> Result<Option<String>, Error> {
        let actions: Vec<_> = choices.iter().map(|title| json!({"title":title})).collect();
        let value = self
            .request(
                "window/showMessageRequest",
                json!({"type":3,"message":message,"actions":actions}),
            )
            .await?;
        Ok(value
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| choices.iter().any(|choice| choice == title))
            .map(str::to_owned))
    }
}
