// Local-only fixtures. Each fixture generates its own signing key in memory.
// Tokens are synthetic test values; they cannot authenticate to OpenAI.
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    body::{Bytes, Frame},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, PrivateSigningKey, StandardClaims, SubjectIdentifier, core::*,
};
use rand_core::OsRng;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use uuid::Uuid;

use crate::lsp::ui::{Ui, UiCapabilities};

use super::{
    auth::Auth,
    auth_store::{AuthStore, Credentials, Registration, now},
    http::Http,
};

#[derive(Clone)]
pub(crate) struct Record {
    pub path: String,
    pub authorization: String,
    pub body: String,
}

pub(crate) struct FixtureState {
    pub records: Vec<Record>,
    pub authorization: HashMap<String, String>,
    pub mode: String,
    pub response_mode: String,
    pub models: Vec<Value>,
    pub stream_error: Option<Value>,
    pub translation_segments: Option<Vec<Value>>,
    pub failure: Option<(u16, Value)>,
    pub direct: bool,
    pub delay: Duration,
    pub stream_tail_delay: Duration,
    pub refreshes: usize,
    pub revocations: usize,
    pub refresh_failure: Option<(u16, Value)>,
    pub revoke_failure: bool,
}

pub(crate) struct Fixture {
    pub base: String,
    pub state: Arc<Mutex<FixtureState>>,
    signing_key: Arc<CoreEdDsaPrivateSigningKey>,
    task: JoinHandle<()>,
}

impl Fixture {
    pub async fn new() -> Self {
        let signing_key = {
            let key = SigningKey::generate(&mut OsRng);
            let pem = key.to_pkcs8_pem(Default::default()).unwrap();
            Arc::new(CoreEdDsaPrivateSigningKey::from_ed25519_pem(&pem, None).unwrap())
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FixtureState {
            records: Vec::new(),
            authorization: HashMap::new(),
            mode: String::new(),
            response_mode: String::new(),
            models: vec![
                json!({"slug":"hidden","display_name":"Hidden","visibility":"hidden"}),
                json!({"slug":"synthetic-model","display_name":"Synthetic model","visibility":"list"}),
                json!({"slug":"second","display_name":"Second","visibility":"list"}),
            ],
            stream_error: None,
            translation_segments: None,
            failure: None,
            direct: true,
            delay: Duration::ZERO,
            stream_tail_delay: Duration::ZERO,
            refreshes: 0,
            revocations: 0,
            refresh_failure: None,
            revoke_failure: false,
        }));
        let (service_base, service_state, service_key) =
            (base.clone(), state.clone(), signing_key.clone());
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let (base, state, signing_key) =
                            (service_base.clone(), service_state.clone(), service_key.clone());
                        connections.spawn(async move {
                            let service = service_fn(move |request| {
                                endpoint(request, base.clone(), state.clone(), signing_key.clone())
                            });
                            let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
                        });
                    }
                    Some(_) = connections.join_next(), if !connections.is_empty() => (),
                }
            }
        });
        Self {
            base,
            state,
            signing_key,
            task,
        }
    }

    pub fn auth(&self) -> Auth {
        let directory = std::env::temp_dir().join(format!("zed-auth-synthetic-{}", Uuid::new_v4()));
        Auth::synthetic(
            AuthStore::synthetic(directory),
            Http::synthetic_revocation(&self.base),
            self.base.clone(),
        )
    }

    pub async fn connected(&self, auth: &Auth, expiry: u64) -> Uuid {
        let id = Uuid::new_v4();
        let mut locked = auth.store.lock(true).await.unwrap().unwrap();
        locked.state.registrations.insert(
            id,
            Registration {
                issuer: self.base.clone(),
                subject: "synthetic-subject".into(),
                email: "same@example.invalid".into(),
                client_id: "oaiapp_synthetic".into(),
                credentials: Some(Credentials {
                    id_token: self.id_token("oaiapp_synthetic", "nonce", ""),
                    access_token: "synthetic-access-0".into(),
                    refresh_token: "synthetic-refresh-0".into(),
                    token_type: "Bearer".into(),
                    scopes: vec!["chatgpt.tokens.use.direct".into()],
                    expires_in: expiry,
                    saved_at: now(),
                }),
            },
        );
        locked.state.active_registration = Some(id);
        locked.save().unwrap();
        self.state
            .lock()
            .unwrap()
            .authorization
            .insert("nonce".into(), "nonce".into());
        id
    }

    pub fn id_token(&self, audience: &str, nonce: &str, mode: &str) -> String {
        signed_token(&self.signing_key, &self.base, audience, nonce, mode)
    }

    pub async fn connect(
        &self,
        auth: &Auth,
        selected: Option<Uuid>,
        enable: bool,
    ) -> Result<String, crate::providers::error::Error> {
        let (connection, client) = lsp_server::Connection::memory();
        let ui = Ui::new(
            connection.sender.clone(),
            UiCapabilities {
                external_documents: true,
                ..Default::default()
            },
        );
        let ui_driver = ui.clone();
        let state = self.state.clone();
        let browser = async move {
            let request = tokio::task::spawn_blocking(move || {
                client
                    .receiver
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
            })
            .await
            .unwrap();
            let lsp_server::Message::Request(request) = request else {
                panic!("Expected external showDocument");
            };
            assert_eq!(request.method, "window/showDocument");
            assert_eq!(request.params["external"], true);
            let url =
                openidconnect::url::Url::parse(request.params["uri"].as_str().unwrap()).unwrap();
            let query: HashMap<String, String> = url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            if selected.is_none() {
                assert_eq!(query["client_id"], "dynamic_agent_client");
                assert_eq!(query["agent_name_hint"], "Zed Code Translation");
            } else {
                assert_eq!(query["client_id"], "oaiapp_synthetic");
                assert!(!query.contains_key("agent_name_hint"));
            }
            assert_eq!(query["code_challenge_method"], "S256");
            for scope in [
                "openid",
                "profile",
                "email",
                "offline_access",
                "resource.invoke",
                "chatgpt.tokens.use.direct",
            ] {
                assert!(
                    query["scope"]
                        .split_whitespace()
                        .any(|value| value == scope)
                );
            }
            assert_eq!(query["resource"], "https://api.openai.com/v1");
            assert!(query["ext_agent_host_id"].starts_with("urn:uuid:"));
            if enable {
                assert_eq!(query["prompt"], "consent");
            }
            state.lock().unwrap().authorization = query.clone();
            ui_driver.response(lsp_server::Response::new_ok(
                request.id,
                json!({"success":true}),
            ));
            let mut callback = openidconnect::url::Url::parse(&query["redirect_uri"]).unwrap();
            let mode = state.lock().unwrap().mode.clone();
            callback.query_pairs_mut().append_pair(
                "state",
                if mode == "wrong_state" {
                    "wrong"
                } else {
                    &query["state"]
                },
            );
            if mode == "denied" {
                callback
                    .query_pairs_mut()
                    .append_pair("error", "access_denied");
            } else {
                callback
                    .query_pairs_mut()
                    .append_pair("code", "synthetic-code");
                if mode != "missing_client" {
                    callback.query_pairs_mut().append_pair(
                        "client_id",
                        if mode == "client_mismatch" {
                            "other-client"
                        } else {
                            "oaiapp_synthetic"
                        },
                    );
                }
                if mode == "duplicate_query" {
                    callback
                        .query_pairs_mut()
                        .append_pair("code", "duplicate-code");
                }
            }
            let response = reqwest::Client::new().get(callback).send().await.unwrap();
            assert_eq!(
                response.status().as_u16(),
                if matches!(
                    mode.as_str(),
                    "wrong_state"
                        | "denied"
                        | "missing_client"
                        | "duplicate_query"
                        | "client_mismatch"
                ) {
                    400
                } else {
                    200
                }
            );
        };
        let attempt = async {
            tokio::time::timeout(Duration::from_secs(1), auth.connect(&ui, selected, enable))
                .await
                .unwrap_or_else(|_| {
                    Err(crate::providers::error::Error::failed(
                        "Synthetic callback was rejected and the attempt was cancelled",
                    ))
                })
        };
        let (result, ()) = tokio::join!(attempt, browser);
        result
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn signed_token(
    signing_key: &CoreEdDsaPrivateSigningKey,
    base: &str,
    audience: &str,
    nonce: &str,
    mode: &str,
) -> String {
    let issuer = if mode == "issuer" {
        "https://wrong-issuer.invalid"
    } else {
        base
    };
    let audience = if mode == "audience" {
        "wrong-client"
    } else {
        audience
    };
    let nonce = if mode == "nonce" {
        "wrong-nonce"
    } else {
        nonce
    };
    let expiration = if mode == "expired" {
        SystemTime::now() - Duration::from_secs(60)
    } else {
        SystemTime::now() + Duration::from_secs(3600)
    };
    let subject = if mode == "subject" {
        "different-subject"
    } else {
        "synthetic-subject"
    };
    let claims = CoreIdTokenClaims::new(
        IssuerUrl::new(issuer.into()).unwrap(),
        vec![Audience::new(audience.into())],
        expiration.into(),
        SystemTime::now().into(),
        StandardClaims::new(SubjectIdentifier::new(subject.into()))
            .set_email(Some(EndUserEmail::new("same@example.invalid".into()))),
        EmptyAdditionalClaims {},
    )
    .set_nonce(Some(Nonce::new(nonce.into())));
    let token = CoreIdToken::new(
        claims,
        signing_key,
        CoreJwsSigningAlgorithm::EdDsa,
        None,
        None,
    )
    .unwrap()
    .to_string();
    if mode == "signature" {
        let mut bytes = token.into_bytes();
        let end = bytes.len() - 10;
        bytes[end] = if bytes[end] == b'A' { b'B' } else { b'A' };
        String::from_utf8(bytes).unwrap()
    } else {
        token
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixture_keys_are_distinct_and_jwks_verify_only_their_own_tokens() {
    let first = Fixture::new().await;
    let second = Fixture::new().await;
    let client = reqwest::Client::new();
    let first_jwks: Value = client
        .get(format!("{}/jwks", first.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_jwks: Value = client
        .get(format!("{}/jwks", second.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(first_jwks, second_jwks);
    let verifier = |jwks| {
        CoreIdTokenVerifier::new_public_client(
            openidconnect::ClientId::new("oaiapp_synthetic".into()),
            IssuerUrl::new(first.base.clone()).unwrap(),
            serde_json::from_value(jwks).unwrap(),
        )
        .set_allowed_algs([CoreJwsSigningAlgorithm::EdDsa])
    };
    let own = verifier(first_jwks.clone());
    let other = verifier(second_jwks);
    let nonce = Nonce::new("nonce".into());
    for _ in 0..2 {
        let token: CoreIdToken = first
            .id_token("oaiapp_synthetic", "nonce", "")
            .parse()
            .unwrap();
        assert!(token.claims(&own, &nonce).is_ok());
        assert!(matches!(
            token.claims(&other, &nonce),
            Err(openidconnect::ClaimsVerificationError::SignatureVerification(_))
        ));
    }
    let after: Value = client
        .get(format!("{}/jwks", first.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after, first_jwks);
}

fn event(kind: &str, value: Value) -> String {
    format!("event: {kind}\ndata: {value}\n\n")
}

fn response_stream(mode: &str, text: &str, model: &str) -> String {
    let message = |text: &str| {
        json!({
            "id":"synthetic-message", "type":"message", "role":"assistant", "status":"completed",
            "content":[{"type":"output_text", "text":text}]
        })
    };
    let item_done = |index: u64, item: Value| {
        event(
            "response.output_item.done",
            json!({
                "type":"response.output_item.done", "output_index":index, "item":item
            }),
        )
    };
    let text_event = |kind: &str, index: u64, content: u64, id: &str, field: &str, text: &str| {
        event(
            kind,
            json!({
                "type":kind, "output_index":index, "content_index":content, "item_id":id, (field):text
            }),
        )
    };
    let delta_event = |index, content, id, text| {
        text_event(
            "response.output_text.delta",
            index,
            content,
            id,
            "delta",
            text,
        )
    };
    let mut snapshot = message(text);
    // Preserve the earlier full snapshot path without an item ID.
    snapshot.as_object_mut().unwrap().remove("id");
    let completion = event(
        "response.completed",
        json!({
            "type":"response.completed", "response":{"status":"completed", "output":[snapshot]}
        }),
    );
    let compact = event(
        "response.completed",
        json!({
            "type":"response.completed", "response":{"status":"completed", "error":null, "model":model, "output":[]}
        }),
    );
    let split = (0..=text.len() / 2)
        .rev()
        .find(|offset| text.is_char_boundary(*offset))
        .unwrap();
    let fragments = format!(
        "{}{}",
        delta_event(0, 0, "synthetic-message", &text[..split]),
        delta_event(0, 0, "synthetic-message", &text[split..])
    );
    let delta = delta_event(0, 0, "synthetic-message", "partial");
    let text_done = text_event(
        "response.output_text.done",
        0,
        0,
        "synthetic-message",
        "text",
        text,
    );
    let done = item_done(0, message(text));
    let valid = format!("{fragments}{text_done}{done}");
    let bad_item = |field: &str, value: Value| {
        let mut item = message(text);
        item[field] = value;
        item_done(0, item)
    };
    let failed = event(
        "response.failed",
        json!({
            "type":"response.failed", "response":{"error":{
                "code":"subscription_sharing_usage_limit_exceeded", "message":"synthetic limit"
            }}
        }),
    );
    match mode {
        "delta_compact"
        | "public_compact"
        | "compact_missing"
        | "compact_duplicate"
        | "compact_code_change"
        | "compact_output_limit" => format!("{valid}{compact}"),
        "done_compact" => format!("{done}{compact}"),
        "text_done_compact" => format!("{text_done}{done}{compact}"),
        "delta_snapshot" => format!("{valid}{completion}"),
        "delta_without_item" => format!("{fragments}{compact}"),
        "delta_mismatch" => format!("{delta}{completion}"),
        "empty_completed" => compact.clone(),
        "delta_after_completed" => format!("{completion}{delta}"),
        "compact_late_failed" => format!("{valid}{compact}{failed}"),
        "compact_incomplete" => format!(
            "{valid}{}",
            event("response.incomplete", json!({"type":"response.incomplete"}))
        ),
        "compact_invalid_json" => format!("{delta}{}{compact}", item_done(0, message("partial"))),
        "compact_partial_json" => format!("{}{compact}", item_done(0, message("{\"segments\":["))),
        "item_role" => format!("{}{compact}", bad_item("role", json!("user"))),
        "item_status" => format!("{}{compact}", bad_item("status", json!("in_progress"))),
        "item_type" => format!("{}{compact}", bad_item("type", json!("function_call"))),
        "item_content" => format!(
            "{}{compact}",
            bad_item("content", json!([{"type":"refusal","refusal":"no"}]))
        ),
        "item_missing_id" => {
            let mut item = message(text);
            item.as_object_mut().unwrap().remove("id");
            format!("{}{compact}", item_done(0, item))
        }
        "item_id_mismatch" => format!(
            "{fragments}{}{compact}",
            bad_item("id", json!("different-item"))
        ),
        "item_duplicate" => format!("{done}{done}{compact}"),
        "item_duplicate_id" => format!("{done}{}{compact}", item_done(1, message(text))),
        "item_after_completed" => format!("{completion}{done}"),
        "item_without_completed" => done,
        "item_snapshot_mismatch" => format!("{}{completion}", item_done(0, message("other"))),
        "item_snapshot_index_mismatch" => format!("{}{completion}", item_done(1, message(text))),
        "item_snapshot_id_mismatch" => {
            let mut item = message(text);
            item["id"] = json!("different-item");
            format!(
                "{done}{}",
                event(
                    "response.completed",
                    json!({
                        "type":"response.completed", "response":{"status":"completed", "output":[item]}
                    })
                )
            )
        }
        "item_text_order" => format!(
            "{fragments}{}{compact}",
            bad_item(
                "content",
                json!([
                    {"type":"output_text", "text":&text[split..]}, {"type":"output_text", "text":&text[..split]}
                ])
            )
        ),
        "item_output_limit" => format!(
            "{}{compact}",
            item_done(0, message(&"x".repeat(1024 * 1024 + 1)))
        ),
        "item_total_limit" => {
            let mut second = message(&"x".repeat(600 * 1024));
            second["id"] = json!("second-item");
            format!(
                "{}{}{compact}",
                item_done(0, message(&"x".repeat(600 * 1024))),
                item_done(1, second)
            )
        }
        "item_index_order" => {
            let mut second = message(&text[split..]);
            second["id"] = json!("second-item");
            format!(
                "{}{}{}{}{compact}",
                delta_event(0, 0, "synthetic-message", &text[..split]),
                delta_event(1, 0, "second-item", &text[split..]),
                item_done(1, second),
                item_done(0, message(&text[..split]))
            )
        }
        "text_done_mismatch" => format!(
            "{fragments}{}{done}{compact}",
            text_event(
                "response.output_text.done",
                0,
                0,
                "synthetic-message",
                "text",
                "other"
            )
        ),
        "text_done_duplicate" => format!("{fragments}{text_done}{text_done}{done}{compact}"),
        "delta_after_text_done" => format!("{fragments}{text_done}{delta}{done}{compact}"),
        "delta_after_item_done" => format!("{done}{delta}{compact}"),
        "late_failed" => format!("{delta}{failed}"),
        "after_complete_failed" => format!("{completion}{failed}"),
        "explicit_error" => format!(
            "{delta}{}",
            event(
                "error",
                json!({"type":"error","code":"server_error","message":"synthetic explicit stream failure"})
            )
        ),
        "incomplete" => format!(
            "{delta}{}",
            event("response.incomplete", json!({"type":"response.incomplete"}))
        ),
        "disconnect" => delta,
        "invalid_json" => "data: {broken}\n\n".into(),
        "raw_limit" => format!("data: {}", "x".repeat(2 * 1024 * 1024 + 1)),
        "stream_limit" => event(
            "response.in_progress",
            json!({"type":"response.in_progress","padding":"x".repeat(16*1024)}),
        )
        .repeat(520),
        "delta_limit" => delta_event(0, 0, "synthetic-message", &"x".repeat(1024 * 1024 + 1)),
        "trailing_partial" => format!("{completion}data: {{unfinished"),
        "compact_truncated" => format!("{valid}{compact}data: {{unfinished"),
        "crlf" => completion.replace('\n', "\r\n"),
        _ => completion,
    }
}

async fn endpoint(
    request: hyper::Request<hyper::body::Incoming>,
    base: String,
    shared: Arc<Mutex<FixtureState>>,
    signing_key: Arc<CoreEdDsaPrivateSigningKey>,
) -> Result<hyper::Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let path = request.uri().path().to_owned();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    let delay = {
        let mut state = shared.lock().unwrap();
        state.records.push(Record {
            path: path.clone(),
            authorization,
            body: body.clone(),
        });
        state.delay
    };
    if path == "/v1/responses" || path == "/token" {
        tokio::time::sleep(delay).await;
    }
    let mut state = shared.lock().unwrap();
    let (status, content_type, result) = match path.as_str() {
        "/.well-known/openid-configuration" => (200, "application/json", json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"jwks_uri":format!("{base}/jwks"),"revocation_endpoint":format!("{}/revoke",base.replacen("http:","https:",1)),"response_types_supported":["code"],"subject_types_supported":["public"],"id_token_signing_alg_values_supported":["EdDSA"]}).to_string()),
        "/jwks" => (200, "application/json", json!({"keys":[signing_key.as_verification_key()]}).to_string()),
        "/token" => {
            let form: HashMap<String, String> = openidconnect::url::form_urlencoded::parse(body.as_bytes()).map(|(key,value)| (key.into_owned(),value.into_owned())).collect();
            assert_eq!(form["resource"], "https://api.openai.com/v1");
            assert_eq!(form["client_id"], "oaiapp_synthetic");
            assert!(!form.contains_key("client_secret"));
            let refresh = form["grant_type"] == "refresh_token";
            if refresh { state.refreshes += 1; assert!(!form.contains_key("scope")); }
            else {
                assert_eq!(form["redirect_uri"], state.authorization["redirect_uri"]);
                assert_eq!(PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(form["code_verifier"].clone())).as_str(), state.authorization["code_challenge"]);
            }
            if let Some((status, body)) = state.refresh_failure.clone().filter(|_| refresh) { (status,"application/json",body.to_string()) }
            else {
                let count = state.refreshes;
                let nonce = state.authorization.get("nonce").map_or("nonce", String::as_str);
                let token = signed_token(&signing_key, &base, "oaiapp_synthetic", nonce, &state.mode);
                let scope = match state.mode.as_str() {
                    "identity_only" => "openid profile email",
                    "missing_refresh_renewable" => "openid profile email offline_access",
                    _ if state.direct => "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
                    _ => "openid profile email offline_access",
                };
                let mut response = json!({"id_token":token,"access_token":format!("synthetic-access-{count}"),"refresh_token":format!("synthetic-refresh-{count}"),"token_type":"Bearer","scope":scope,"expires_in":3600});
                if matches!(state.mode.as_str(),"identity_only"|"missing_refresh"|"missing_refresh_renewable") {
                    response.as_object_mut().unwrap().remove("refresh_token");
                }
                (200,"application/json",response.to_string())

            }
        }
        "/revoke" => { state.revocations += 1; if state.revoke_failure { (503,"application/json",json!({"detail":"synthetic revoke outage"}).to_string()) } else { (200,"text/plain",String::new()) } }
        "/v1/models" => (200,"application/json",json!({"models":state.models}).to_string()),
        "/v1/responses" => {
            if let Some((status, body)) = &state.failure { (*status,"application/json",body.as_str().map_or_else(|| body.to_string(),str::to_owned)) }
            else {
                let request: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(request["store"], false); assert_eq!(request["stream"], true);
                assert!(request["input"].is_array());
                for field in ["temperature","max_output_tokens","previous_response_id","tools","prompt"] { assert!(request.get(field).is_none()); }
                assert!(request["input"].as_array().unwrap().iter().all(|item| item["role"] != "system"));
                let segments: Value = serde_json::from_str(request["input"][0]["content"].as_str().unwrap()).unwrap();
                let mut translated: Vec<Value> = segments["segments"].as_array().unwrap().iter().filter(|segment| segment["protected"] == false)
                    .map(|segment| json!({"id":segment["id"],"text":format!("訳{}",segment["text"].as_str().unwrap())})).collect();
                if let Some(translations) = &state.translation_segments {
                    translated = translations.clone();
                }
                match state.response_mode.as_str() {
                    "missing" | "compact_missing" => { translated.pop(); },
                    "duplicate" | "compact_duplicate" => { translated.push(translated[0].clone()); },
                    "output_limit" | "compact_output_limit" => { translated[0]["text"] = json!("字".repeat(64*1024)); },
                    "compact_code_change" => { translated[0]["text"] = json!("`changed prose`"); },
                    _ => (),
                }
                let text = if state.response_mode == "public_compact" {
                    let text = match request["model"].as_str().unwrap() {
                        "gpt-6.1-sol" => "リモートサービスが一時的な障害を報告した場合にのみ再試行してください。",
                        "gpt-6-luna" => "リモートサービスが一時的な障害を報告した場合に限り、再試行してください。",
                        _ => panic!("Unexpected public trace model"),
                    };
                    json!({"segments":[{"id":0,"text":text}]}).to_string()
                } else {
                    json!({"segments":translated}).to_string()
                };
                let stream = match &state.stream_error {
                    Some(error) => format!("data: {error}\n\n"),
                    None => response_stream(&state.response_mode, &text, request["model"].as_str().unwrap()),
                };
                (200,"text/event-stream",stream)
            }
        }
        _ => (404,"text/plain","unknown synthetic endpoint".into()),
    };
    let mut response = hyper::Response::builder();
    if (300..400).contains(&status) {
        response = response.header("location", format!("{base}/v1/responses"));
    }
    let tail_delay = state.stream_tail_delay;
    let body = if path == "/v1/responses" && !tail_delay.is_zero() {
        let frames = futures_util::stream::once(async move {
            Ok::<_, Infallible>(Frame::data(Bytes::from(result)))
        })
        .chain(futures_util::stream::once(async move {
            tokio::time::sleep(tail_delay).await;
            Ok::<_, Infallible>(Frame::data(Bytes::new()))
        }));
        BodyExt::boxed(StreamBody::new(frames))
    } else {
        Full::new(Bytes::from(result)).boxed()
    };
    Ok(response
        .status(status)
        .header("content-type", content_type)
        .header("x-request-id", "req-synthetic")
        .body(body)
        .unwrap())
}
