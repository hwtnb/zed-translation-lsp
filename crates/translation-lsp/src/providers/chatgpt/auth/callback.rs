use super::Error;
use http_body_util::Full;
use hyper::{body::Bytes, service::service_fn};
use hyper_util::rt::TokioIo;
use openidconnect::CsrfToken;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinSet};

pub(super) struct Callback {
    pub code: String,
    pub client_id: String,
}

pub(super) async fn wait_callback(
    listener: TcpListener,
    state: CsrfToken,
    returning: Option<String>,
) -> Result<Callback, Error> {
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let address = listener
        .local_addr()
        .map_err(|_| Error::failed("Callback listener stopped"))?
        .to_string();
    let mut connections = JoinSet::new();
    tokio::pin!(receiver);
    loop {
        tokio::select! {
            result = &mut receiver => return result.map_err(|_| Error::failed("Callback listener stopped"))?,
            accepted = listener.accept(), if connections.len() < 8 => {
                let (socket, _) = accepted.map_err(|_| Error::failed("Callback listener stopped"))?;
                let (state, returning, sender, address) = (
                    state.clone(), returning.clone(), sender.clone(), address.clone(),
                );
                connections.spawn(async move {
                    let service = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                        let result = callback(&request, &state, returning.as_deref(), &address);
                        let status = if result.is_ok() { 200 } else { 400 };
                        if request.uri().path() == "/auth/callback"
                            && request.headers().get("host").and_then(|host| host.to_str().ok())
                                == Some(address.as_str())
                            && valid_state(request.uri().query().unwrap_or(""), &state)
                            && let Some(sender) = sender.lock().unwrap().take()
                        {
                            let _ = sender.send(result);
                        }
                        async move {
                            let message: &'static [u8] = if status == 200 {
                                b"ChatGPT callback received. You can return to Zed."
                            } else {
                                b"Invalid ChatGPT callback."
                            };
                            Ok::<_, Infallible>(
                                hyper::Response::builder()
                                    .status(status)
                                    .header("content-type", "text/plain; charset=utf-8")
                                    .body(Full::new(Bytes::from_static(message)))
                                    .unwrap()
                            )
                        }
                    });
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder.max_buf_size(32 * 1024);
                    let _ = tokio::time::timeout(
                        Duration::from_secs(5),
                        builder.serve_connection(TokioIo::new(socket), service),
                    ).await;
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => (),
        }
    }
}

fn valid_state(query: &str, state: &CsrfToken) -> bool {
    let values: Vec<_> = openidconnect::url::form_urlencoded::parse(query.as_bytes())
        .filter(|(key, _)| key == "state")
        .map(|(_, value)| value)
        .collect();
    matches!(values.as_slice(), [received] if CsrfToken::new(received.to_string()) == *state)
}

pub(super) fn callback<B>(
    request: &hyper::Request<B>,
    state: &CsrfToken,
    returning: Option<&str>,
    address: &str,
) -> Result<Callback, Error> {
    if request.method() != hyper::Method::GET
        || request.uri().path() != "/auth/callback"
        || request
            .headers()
            .get("host")
            .and_then(|host| host.to_str().ok())
            != Some(address)
        || request.uri().to_string().len() > 16 * 1024
    {
        return Err(Error::failed("Invalid ChatGPT callback path or host"));
    }
    let mut parameters = HashMap::new();
    for (key, value) in
        openidconnect::url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
    {
        if parameters
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(Error::failed("Duplicate ChatGPT callback parameter"));
        }
    }
    let received = parameters
        .get("state")
        .ok_or_else(|| Error::failed("Missing callback state"))?;
    if CsrfToken::new(received.clone()) != *state {
        return Err(Error::failed("ChatGPT callback state does not match"));
    }
    if parameters.contains_key("error") {
        return Err(Error::failed(
            "ChatGPT consent was declined; no code was exchanged",
        ));
    }
    let client_id = match (returning, parameters.get("client_id")) {
        (Some(saved), None) => saved.to_owned(),
        (Some(saved), Some(received)) if saved == received => saved.to_owned(),
        (None, Some(received)) if !received.is_empty() && received != "dynamic_agent_client" => {
            received.clone()
        }
        _ => {
            return Err(Error::failed(
                "ChatGPT callback did not provide the expected issued client ID",
            ));
        }
    };
    let code = parameters
        .get("code")
        .filter(|code| !code.is_empty())
        .cloned()
        .ok_or_else(|| Error::failed("Missing callback code"))?;
    Ok(Callback { code, client_id })
}
