//! A local HTTP service for tests of the providers that send API keys. It
//! records each request and answers with the replies a test gives it.
use std::{
    collections::VecDeque,
    convert::Infallible,
    mem,
    sync::{Arc, Mutex},
};

use http_body_util::{BodyExt, Full};
use hyper::{
    Response,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, task::JoinHandle};

pub(crate) struct Record {
    pub path: String,
    pub headers: hyper::HeaderMap,
    pub body: serde_json::Value,
}

#[derive(Clone)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(&'static str, &'static str)>,
}

impl Reply {
    /// A stream of server-sent events.
    pub fn stream(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            headers: vec![("content-type", "text/event-stream")],
        }
    }

    pub fn failure(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: vec![
                ("content-type", "application/json"),
                ("x-request-id", "req_synthetic"),
            ],
        }
    }
}

#[derive(Default)]
struct State {
    records: Vec<Record>,
    replies: VecDeque<Reply>,
}

pub(crate) struct HttpFixture {
    /// Such as `http://127.0.0.1:49152`.
    pub base: String,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
}

impl HttpFixture {
    pub async fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let state = shared.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| answer(request, state.clone()));
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(socket), service)
                        .await;
                });
            }
        });
        Self { base, state, task }
    }

    pub fn reply(&self, reply: Reply) {
        self.state.lock().unwrap().replies.push_back(reply);
    }

    pub fn records(&self) -> Vec<Record> {
        mem::take(&mut self.state.lock().unwrap().records)
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(
    request: hyper::Request<Incoming>,
    state: Arc<Mutex<State>>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let mut state = state.lock().unwrap();
    state.records.push(Record {
        path,
        headers,
        body: serde_json::from_slice(&body).unwrap_or_default(),
    });
    let reply = state
        .replies
        .pop_front()
        .unwrap_or_else(|| Reply::failure(500, "no reply was given"));
    let mut response = Response::builder().status(reply.status);
    for (name, value) in reply.headers {
        response = response.header(name, value);
    }
    Ok(response.body(Full::new(Bytes::from(reply.body))).unwrap())
}
