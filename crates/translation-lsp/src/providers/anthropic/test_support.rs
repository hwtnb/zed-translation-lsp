// Local-only fixture. The key is a synthetic test value; it cannot authenticate
// to Anthropic.
use std::{
    collections::VecDeque,
    convert::Infallible,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures_util::StreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    body::{Bytes, Frame},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::{Notify, Semaphore},
    task::{JoinHandle, JoinSet},
};

use super::AnthropicProvider;
use crate::providers::credential_directory;

pub(crate) const KEY: &str = "sk-ant-synthetic-key";

// A credential directory of a test's own, removed with the value. Tests read
// the key file here and never in the directory of the user.
pub(crate) struct KeyDirectory {
    pub path: PathBuf,
}

impl KeyDirectory {
    // A private directory without a key file.
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("zed-anthropic-key-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).unwrap();
        Self { path }
    }

    // A private directory whose private key file holds `contents`.
    pub fn with_key(contents: impl AsRef<[u8]>) -> Self {
        let directory = Self::new();
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        std::io::Write::write_all(
            &mut options.open(directory.file()).unwrap(),
            contents.as_ref(),
        )
        .unwrap();
        directory
    }

    pub fn file(&self) -> PathBuf {
        self.path.join("anthropic-api-key")
    }

    // Replaces the key file at once, as the key command does.
    pub fn replace(&self, contents: &str) {
        credential_directory::replace(&self.path, "anthropic-api-key", contents.as_bytes())
            .unwrap();
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(&self.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

// Stands for the terminal of the OS. It opens no window: it records the
// executable it was given and answers what `fails` says.
#[derive(Clone, Default)]
pub(crate) struct Terminal {
    pub opened: Arc<Mutex<Vec<PathBuf>>>,
    pub fails: Arc<AtomicBool>,
}

impl Terminal {
    pub fn open(&self, executable: &Path) -> bool {
        self.opened.lock().unwrap().push(executable.into());
        !self.fails.load(Ordering::SeqCst)
    }
}

impl Drop for KeyDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct Record {
    pub path: String,
    pub headers: hyper::HeaderMap,
    pub body: String,
}

#[derive(Clone)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    // Close the connection after the body without ending the response.
    pub interrupted: bool,
}

impl Reply {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            interrupted: false,
        }
    }

    pub fn stream(body: impl Into<String>) -> Self {
        Self::new(200, body)
    }

    pub fn interrupted(body: impl Into<String>) -> Self {
        Self {
            interrupted: true,
            ..Self::new(200, body)
        }
    }

    pub fn failure(status: u16, error_type: &str, message: &str) -> Self {
        Self::new(
            status,
            json!({
                "type": "error",
                "error": {"type": error_type, "message": message},
                "request_id": "req_body"
            })
            .to_string(),
        )
    }
}

#[derive(Default)]
pub(crate) struct FixtureState {
    pub records: Vec<Record>,
    // Scripted replies are consumed first, then `fallback` repeats. Without
    // either, the fixture streams `訳` followed by the submitted text.
    pub scripted: VecDeque<Reply>,
    pub fallback: Option<Reply>,
    pub retry_after: Option<String>,
    pub omit_request_id: bool,
    // Leave each response open after its body, as an unfinished stream.
    pub hold: bool,
    // A recorded request is answered only when the test adds a permit.
    pub gate: Option<Arc<Semaphore>>,
}

#[derive(Default)]
pub(crate) struct OpenResponses {
    count: AtomicUsize,
    changed: Notify,
}

impl OpenResponses {
    pub async fn wait(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.count.load(Ordering::SeqCst) != count {
                self.changed.notified().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Expected {count} open responses; observed {}",
                self.count.load(Ordering::SeqCst)
            )
        });
    }
}

struct Held(Arc<OpenResponses>);

impl Held {
    fn new(open: Arc<OpenResponses>) -> Self {
        open.count.fetch_add(1, Ordering::SeqCst);
        open.changed.notify_one();
        Self(open)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_one();
    }
}

pub(crate) struct Fixture {
    pub base: String,
    // Where `provider` reads its key, which is `KEY` until a test changes it.
    pub keys: KeyDirectory,
    pub state: Arc<Mutex<FixtureState>>,
    pub open: Arc<OpenResponses>,
    task: JoinHandle<()>,
}

impl Fixture {
    pub async fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FixtureState::default()));
        let open = Arc::new(OpenResponses::default());
        let (service_state, service_open) = (state.clone(), open.clone());
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let (state, open) = (service_state.clone(), service_open.clone());
                        connections.spawn(async move {
                            let service = service_fn(move |request| {
                                endpoint(request, state.clone(), open.clone())
                            });
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(socket), service)
                                .await;
                        });
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => (),
                }
            }
        });
        Self {
            base,
            keys: KeyDirectory::with_key(KEY),
            state,
            open,
            task,
        }
    }

    pub fn provider(&self) -> AnthropicProvider {
        AnthropicProvider::synthetic(format!("{}/v1", self.base), &self.keys.path)
    }

    pub fn calls(&self) -> usize {
        self.state.lock().unwrap().records.len()
    }

    pub fn reply(&self, reply: Reply) {
        self.state.lock().unwrap().scripted.push_back(reply);
    }

    pub fn always(&self, reply: Option<Reply>) {
        self.state.lock().unwrap().fallback = reply;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) fn event(kind: &str, value: Value) -> String {
    format!("event: {kind}\ndata: {value}\n\n")
}

pub(crate) fn started() -> String {
    event(
        "message_start",
        json!({"type":"message_start","message":{
            "id":"msg_synthetic","type":"message","role":"assistant","content":[],
            "stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}
        }}),
    )
}

// One text block whose content arrives as two deltas.
pub(crate) fn text_block(index: u64, text: &str) -> String {
    let split = (0..=text.len() / 2)
        .rev()
        .find(|offset| text.is_char_boundary(*offset))
        .unwrap();
    let delta = |text: &str| {
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,
                "delta":{"type":"text_delta","text":text}}),
        )
    };
    format!(
        "{}{}{}{}",
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,
                "content_block":{"type":"text","text":""}}),
        ),
        delta(&text[..split]),
        delta(&text[split..]),
        event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ),
    )
}

pub(crate) fn thinking_block(index: u64) -> String {
    format!(
        "{}{}{}{}",
        event(
            "content_block_start",
            json!({"type":"content_block_start","index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}}),
        ),
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,
                "delta":{"type":"thinking_delta","thinking":"synthetic reasoning"}}),
        ),
        event(
            "content_block_delta",
            json!({"type":"content_block_delta","index":index,
                "delta":{"type":"signature_delta","signature":"synthetic-signature"}}),
        ),
        event(
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ),
    )
}

pub(crate) fn stopped(delta: Value) -> String {
    format!(
        "{}{}",
        event(
            "message_delta",
            json!({"type":"message_delta","delta":delta,"usage":{"output_tokens":1}}),
        ),
        event("message_stop", json!({"type":"message_stop"})),
    )
}

pub(crate) fn completed(text: &str) -> String {
    format!(
        "{}{}{}",
        started(),
        text_block(0, text),
        stopped(json!({"stop_reason":"end_turn","stop_sequence":null})),
    )
}

async fn endpoint(
    request: hyper::Request<hyper::body::Incoming>,
    shared: Arc<Mutex<FixtureState>>,
    open: Arc<OpenResponses>,
) -> Result<hyper::Response<BoxBody<Bytes, std::io::Error>>, Infallible> {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    let gate = {
        let mut state = shared.lock().unwrap();
        state.records.push(Record {
            path: path.clone(),
            headers,
            body: body.clone(),
        });
        state.gate.clone()
    };
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    let mut state = shared.lock().unwrap();
    let reply = if path != "/v1/messages" {
        Reply::new(404, "unknown synthetic endpoint")
    } else if let Some(reply) = state
        .scripted
        .pop_front()
        .or_else(|| state.fallback.clone())
    {
        reply
    } else {
        let request: Value = serde_json::from_str(&body).unwrap();
        Reply::stream(completed(&format!(
            "訳{}",
            request["messages"][0]["content"].as_str().unwrap()
        )))
    };
    let mut response = hyper::Response::builder().status(reply.status).header(
        "content-type",
        if reply.status == 200 {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    if !state.omit_request_id {
        response = response.header("request-id", "req_synthetic");
    }
    if (300..400).contains(&reply.status) {
        response = response.header("location", "/v1/messages");
    }
    if let Some(delay) = &state.retry_after {
        response = response.header("retry-after", delay);
    }
    let first = |body: String| {
        futures_util::stream::once(async move {
            Ok::<_, std::io::Error>(Frame::data(Bytes::from(body)))
        })
    };
    let body = if reply.interrupted {
        let cut = futures_util::stream::once(async {
            // Let the server flush the status and body before the connection drops.
            tokio::task::yield_now().await;
            Err(std::io::Error::other("synthetic connection loss"))
        });
        BodyExt::boxed(StreamBody::new(first(reply.body).chain(cut)))
    } else if state.hold {
        let held = futures_util::stream::once(async move {
            let _held = Held::new(open);
            std::future::pending::<Result<Frame<Bytes>, std::io::Error>>().await
        });
        BodyExt::boxed(StreamBody::new(first(reply.body).chain(held)))
    } else {
        Full::new(Bytes::from(reply.body))
            .map_err(|never| match never {})
            .boxed()
    };
    Ok(response.body(body).unwrap())
}
