use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crossbeam_channel::Sender;
use lsp_server::{Message, Request, RequestId, Response};
use lsp_types::{ClientCapabilities, MessageType};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::providers::error::Error;

#[derive(Clone)]
pub(crate) struct Ui {
    pub(super) sender: Sender<Message>,
    replies: Arc<Mutex<HashMap<RequestId, oneshot::Sender<Response>>>>,
    pub(super) capabilities: UiCapabilities,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct UiCapabilities {
    pub external_documents: bool,
    pub work_done_progress: bool,
    pub server_status: bool,
}

impl From<&ClientCapabilities> for UiCapabilities {
    fn from(client: &ClientCapabilities) -> Self {
        Self {
            external_documents: client
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

    pub fn response(&self, response: Response) {
        if let Some(sender) = self.replies.lock().unwrap().remove(&response.id) {
            let _ = sender.send(response);
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let id = RequestId::from(format!("translation-ui-{}", uuid::Uuid::new_v4()));
        let (sender, reply) = oneshot::channel();
        self.replies.lock().unwrap().insert(id.clone(), sender);
        let _pending = PendingUi {
            ui: self.clone(),
            id: id.clone(),
        };
        self.sender
            .send(
                Request {
                    id,
                    method: method.into(),
                    params,
                }
                .into(),
            )
            .map_err(|_| Error::failed("The editor disconnected"))?;
        let response = reply
            .await
            .map_err(|_| Error::failed("The editor disconnected"))?;
        response
            .response_result
            .map_err(|_| Error::failed("The editor declined the ChatGPT operation"))
    }

    pub async fn open(&self, uri: &str) -> Result<(), Error> {
        if !self.capabilities.external_documents {
            return Err(Error::failed(
                "The editor must support window/showDocument with external URLs",
            ));
        }
        let result = self
            .request(
                "window/showDocument",
                json!({"uri":uri,"external":true,"takeFocus":true}),
            )
            .await?;
        if result["success"] != true {
            return Err(Error::failed("The editor could not open the ChatGPT page"));
        }
        Ok(())
    }

    pub async fn choose(&self, message: &str, choices: &[String]) -> Result<Option<String>, Error> {
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

    pub fn inform(&self, message: &str) -> Result<(), Error> {
        self.show_message(MessageType::INFO, message)
    }

    pub fn error(&self, message: &str) -> Result<(), Error> {
        self.show_message(MessageType::ERROR, message)
    }

    fn show_message(&self, kind: MessageType, message: &str) -> Result<(), Error> {
        self.sender
            .send(
                lsp_server::Notification::new(
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
        if self.ui.replies.lock().unwrap().remove(&self.id).is_some() {
            let _ = self.ui.sender.send(
                lsp_server::Notification::new("$/cancelRequest".into(), json!({"id":self.id}))
                    .into(),
            );
        }
    }
}
