use super::super::test_support::Fixture;
use super::*;
use http_body_util::Full;
use hyper::body::Bytes;
use serde_json::json;

#[test]
fn only_store_contention_is_retryable() {
    assert_eq!(
        Error::from(CredentialError::Busy).kind(),
        ErrorKind::Unavailable
    );
    for error in [
        CredentialError::Invalid,
        CredentialError::Permissions,
        CredentialError::io(
            "test",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ),
    ] {
        assert_eq!(Error::from(error).kind(), ErrorKind::Storage);
    }
}

#[test]
fn callback_checks_state_path_duplicates_and_issued_client_before_exchange() {
    let state = CsrfToken::new("expected".into());
    let parse = |query: &str, path: &str, returning| {
        let request = hyper::Request::builder()
            .uri(format!("{path}?{query}"))
            .header("host", "127.0.0.1:3456")
            .body(Full::new(Bytes::new()))
            .unwrap();
        callback(&request, &state, returning, "127.0.0.1:3456")
    };
    assert!(matches!(
        parse(
            "state=expected&code=c&client_id=oaiapp_new",
            "/auth/callback",
            None
        ),
        Some(Ok(_))
    ));
    assert!(matches!(
        parse(
            "state=expected&code=c",
            "/auth/callback",
            Some("oaiapp_saved")
        ),
        Some(Ok(_))
    ));
    // Requests that are not this sign-in's callback leave it waiting.
    for (query, path) in [
        ("state=wrong&code=c&client_id=oaiapp_new", "/auth/callback"),
        (
            "state=expected&state=expected&code=c&client_id=oaiapp_new",
            "/auth/callback",
        ),
        ("state=expected&code=c&client_id=oaiapp_new", "/wrong"),
    ] {
        assert!(parse(query, path, None).is_none(), "{query} {path}");
    }
    for query in [
        "state=expected&code=c&code=d&client_id=oaiapp_new",
        "state=expected&code=c",
        "state=expected&code=c&client_id=dynamic_agent_client",
        "state=expected&error=access_denied",
    ] {
        assert!(matches!(parse(query, "/auth/callback", None), Some(Err(_))));
    }
    assert!(matches!(
        parse(
            "state=expected&code=c&client_id=oaiapp_other",
            "/auth/callback",
            Some("oaiapp_saved")
        ),
        Some(Err(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn denied_or_invalid_loopback_callbacks_never_exchange_or_activate() {
    for mode in ["denied", "missing_client", "duplicate_query", "wrong_state"] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().mode = mode.into();
        let auth = fixture.auth();
        assert!(fixture.connect(&auth, None, false).await.is_err());
        assert!(
            auth.store
                .snapshot()
                .unwrap()
                .unwrap()
                .state
                .registrations
                .is_empty()
        );
        assert!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .all(|record| record.path != "/token")
        );
    }
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let id = fixture.connected(&auth, 3600).await;
    fixture.state.lock().unwrap().mode = "client_mismatch".into();
    assert!(fixture.connect(&auth, Some(id), false).await.is_err());
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .all(|record| record.path != "/token")
    );
    assert_eq!(
        auth.store
            .snapshot()
            .unwrap()
            .unwrap()
            .state
            .registrations
            .len(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn oidc_binds_client_pkce_nonce_identity_and_plan_permission() {
    for mode in ["", "signature", "issuer", "audience", "nonce", "expired"] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().mode = mode.into();
        let auth = fixture.auth();
        let result = fixture.connect(&auth, None, false).await;
        assert_eq!(result.is_ok(), mode.is_empty(), "validation mode {mode}");
        let snapshot = auth.store.snapshot().unwrap().unwrap();
        assert_eq!(
            snapshot.state.registrations.len(),
            usize::from(mode.is_empty())
        );
        if mode.is_empty() {
            assert!(auth.key().is_ok());
        }
    }
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    fixture.state.lock().unwrap().direct = false;
    assert!(
        fixture
            .connect(&auth, None, false)
            .await
            .unwrap()
            .contains("disabled")
    );
    let snapshot = auth.store.snapshot().unwrap().unwrap();
    let id = snapshot.state.active_registration.unwrap();
    let host = snapshot.state.ext_agent_host_id.clone();
    assert!(auth.key().is_err());
    fixture.state.lock().unwrap().direct = true;
    fixture.connect(&auth, Some(id), true).await.unwrap();
    assert!(auth.key().is_ok());
    fixture.state.lock().unwrap().mode = "subject".into();
    assert!(fixture.connect(&auth, Some(id), false).await.is_err());
    let after = auth.store.snapshot().unwrap().unwrap();
    assert_eq!(after.state.registrations[&id].subject, "synthetic-subject");
    assert_eq!(after.state.ext_agent_host_id, host);
    assert_eq!(after.state.registrations.len(), 1);
    fixture.state.lock().unwrap().mode.clear();
    fixture.connect(&auth, None, false).await.unwrap();
    let after = auth.store.snapshot().unwrap().unwrap();
    assert_eq!(after.state.registrations.len(), 2);
    assert!(
        after
            .state
            .registrations
            .values()
            .all(|registration| registration.email == "same@example.invalid")
    );
    assert_ne!(after.state.active_registration, Some(id));
}

#[tokio::test(flavor = "current_thread")]
async fn identity_only_without_refresh_keeps_verified_registration_and_can_enable() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().mode = "identity_only".into();
    let auth = fixture.auth();
    assert!(
        fixture
            .connect(&auth, None, false)
            .await
            .unwrap()
            .contains("disabled")
    );
    let snapshot = auth.store.snapshot().unwrap().unwrap();
    let id = snapshot.state.active_registration.unwrap();
    let registration = &snapshot.state.registrations[&id];
    assert_eq!(registration.client_id, "oaiapp_synthetic");
    assert_eq!(registration.issuer, fixture.base);
    assert_eq!(registration.subject, "synthetic-subject");
    assert_eq!(registration.email, "same@example.invalid");
    assert!(registration.credentials.is_none());
    assert!(auth.key().is_err());
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .all(|record| !record.path.starts_with("/v1/"))
    );
    let encoded = serde_json::to_value(&snapshot.state).unwrap();
    assert!(
        encoded["registrations"][id.to_string()]
            .get("credentials")
            .is_none()
    );
    fixture.state.lock().unwrap().mode.clear();
    fixture.connect(&auth, Some(id), true).await.unwrap();
    assert!(auth.key().is_ok());
    assert_eq!(
        auth.store
            .snapshot()
            .unwrap()
            .unwrap()
            .state
            .registrations
            .len(),
        1
    );
    for mode in ["missing_refresh", "missing_refresh_renewable"] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().mode = mode.into();
        let auth = fixture.auth();
        assert!(fixture.connect(&auth, None, false).await.is_err());
        assert!(
            auth.store
                .snapshot()
                .unwrap()
                .unwrap()
                .state
                .registrations
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_rotates_as_one_write_and_temporary_errors_retain_credentials() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let id = fixture.connected(&auth, 0).await;
    let old = auth.key().unwrap();
    let access = auth.access(&old).await.unwrap();
    assert_eq!(access.key, old);
    assert_eq!(access.credentials.access_token, "synthetic-access-1");
    assert_eq!(access.credentials.refresh_token, "synthetic-refresh-1");
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
    let mut locked = auth.store.lock(false).await.unwrap().unwrap();
    locked
        .state
        .registrations
        .get_mut(&id)
        .unwrap()
        .credentials
        .as_mut()
        .unwrap()
        .expires_in = 0;
    locked.save().unwrap();
    drop(locked);
    fixture.state.lock().unwrap().refresh_failure =
        Some((503, json!({"detail":"temporary refresh outage"})));
    assert_eq!(
        auth.access(&auth.key().unwrap())
            .await
            .err()
            .unwrap()
            .kind(),
        ErrorKind::Temporary
    );
    assert!(
        auth.store.snapshot().unwrap().unwrap().state.registrations[&id]
            .credentials
            .is_some()
    );
    fixture.state.lock().unwrap().refresh_failure = Some((400, json!({"error":"invalid_grant"})));
    assert_eq!(
        auth.access(&auth.key().unwrap())
            .await
            .err()
            .unwrap()
            .kind(),
        ErrorKind::RefreshUnusable
    );
    let state = auth.store.snapshot().unwrap().unwrap().state;
    assert!(state.registrations[&id].credentials.is_none());
    assert_eq!(state.registrations[&id].client_id, "oaiapp_synthetic");
}

fn saved_credentials(auth: &Auth, id: Uuid) -> Credentials {
    auth.store.snapshot().unwrap().unwrap().state.registrations[&id]
        .credentials
        .clone()
        .unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_is_saved_after_the_key_that_signed_the_saved_id_token_is_retired() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let id = fixture.connected(&auth, 0).await;
    let key = auth.key().unwrap();
    let before = saved_credentials(&auth, id).id_token;
    fixture.rotate_signing_key();
    let access = auth.access(&key).await.unwrap();
    assert_eq!(access.credentials.refresh_token, "synthetic-refresh-1");
    let saved = saved_credentials(&auth, id);
    assert_eq!(saved.access_token, "synthetic-access-1");
    assert_eq!(saved.refresh_token, "synthetic-refresh-1");
    assert_ne!(saved.id_token, before);
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_with_an_unacceptable_id_token_is_not_saved_with_or_without_the_old_key() {
    for retired in [false, true] {
        for mode in [
            "signature",
            "issuer",
            "audience",
            "expired",
            "nonce",
            "subject",
        ] {
            let fixture = Fixture::new().await;
            let auth = fixture.auth();
            let id = fixture.connected(&auth, 0).await;
            let key = auth.key().unwrap();
            if retired {
                fixture.rotate_signing_key();
            }
            fixture.state.lock().unwrap().mode = mode.into();
            let error = auth.access(&key).await.err().unwrap();
            assert_eq!(
                error.to_string(),
                if mode == "subject" {
                    "Refreshed ChatGPT identity does not match the registration"
                } else {
                    "Refreshed ChatGPT ID token is invalid"
                },
                "mode {mode}, old key retired: {retired}"
            );
            assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
            assert_eq!(
                saved_credentials(&auth, id).refresh_token,
                "synthetic-refresh-0"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn refresh_still_compares_the_saved_identity_with_the_registration() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth();
    let id = fixture.connected(&auth, 0).await;
    let key = auth.key().unwrap();
    let mut locked = auth.store.lock(false).await.unwrap().unwrap();
    let registration = locked.state.registrations.get_mut(&id).unwrap();
    registration.credentials.as_mut().unwrap().id_token =
        fixture.id_token("oaiapp_synthetic", "nonce", "subject");
    locked.save().unwrap();
    drop(locked);
    fixture.rotate_signing_key();
    let error = auth.access(&key).await.err().unwrap();
    assert_eq!(
        error.to_string(),
        "Saved ChatGPT identity does not match the registration"
    );
    assert_eq!(
        saved_credentials(&auth, id).refresh_token,
        "synthetic-refresh-0"
    );
}

#[test]
#[ignore = "child entry invoked by the multiprocess refresh test"]
fn refresh_child_process() {
    let directory = std::path::PathBuf::from(std::env::var("ZED_SYNTHETIC_REFRESH_DIR").unwrap());
    assert!(directory.starts_with(std::env::temp_dir()));
    assert!(
        directory
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("zed-auth-multiprocess-")
    );
    let issuer = std::env::var("ZED_SYNTHETIC_ISSUER").unwrap();
    assert!(issuer.starts_with("http://127.0.0.1:"));
    let auth = Auth::synthetic(
        AuthStore::synthetic(directory),
        Http::new().unwrap(),
        issuer,
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                assert!(std::time::Instant::now() < deadline);
                if let Ok(key) = auth.key() {
                    match auth.access(&key).await {
                        Ok(access) => {
                            assert_eq!(access.credentials.refresh_token, "synthetic-refresh-1");
                            break;
                        }
                        Err(error) => assert_eq!(error.kind(), ErrorKind::Modified),
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
}

#[tokio::test(flavor = "current_thread")]
async fn multiprocess_refresh_serializes_rotation_and_rereads_latest_credentials() {
    let fixture = Fixture::new().await;
    fixture.state.lock().unwrap().delay = Duration::from_millis(150);
    let directory = std::env::temp_dir().join(format!("zed-auth-multiprocess-{}", Uuid::new_v4()));
    let auth = Auth::synthetic(
        AuthStore::synthetic(directory.clone()),
        Http::new().unwrap(),
        fixture.base.clone(),
    );
    fixture.connected(&auth, 0).await;
    let mut children = Vec::new();
    for _ in 0..2 {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "providers::chatgpt::auth::tests::refresh_child_process",
                "--nocapture",
            ])
            .env("ZED_SYNTHETIC_REFRESH_DIR", &directory)
            .env("ZED_SYNTHETIC_ISSUER", &fixture.base)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        children.push(tokio::task::spawn_blocking(move || {
            child.wait_with_output().unwrap()
        }));
    }
    for child in children {
        let output = child.await.unwrap();
        assert!(
            output.status.success(),
            "synthetic refresh child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(fixture.state.lock().unwrap().refreshes, 1);
    let snapshot = auth.store.snapshot().unwrap().unwrap();
    let credentials = snapshot.state.registrations[&snapshot.state.active_registration.unwrap()]
        .credentials
        .as_ref()
        .unwrap();
    assert_eq!(credentials.refresh_token, "synthetic-refresh-1");
    assert!(
        !serde_json::to_string(&snapshot.state)
            .unwrap()
            .contains("expires_at")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn signout_preserves_other_same_email_registration_and_host() {
    for fail in [false, true] {
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().revoke_failure = fail;
        let auth = fixture.auth();
        let first = fixture.connected(&auth, 3600).await;
        let selected = fixture.connected(&auth, 3600).await;
        let host = auth
            .store
            .snapshot()
            .unwrap()
            .unwrap()
            .state
            .ext_agent_host_id;
        let message = auth.sign_out().await.unwrap();
        assert_eq!(message.contains("not confirmed"), fail);
        let state = auth.store.snapshot().unwrap().unwrap().state;
        assert!(state.registrations[&selected].credentials.is_none());
        assert!(state.registrations[&first].credentials.is_some());
        assert_eq!(state.registrations.len(), 2);
        assert_eq!(state.ext_agent_host_id, host);
        assert_eq!(
            fixture.state.lock().unwrap().revocations,
            if fail { 3 } else { 1 }
        );
        let record = fixture
            .state
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|record| record.path == "/revoke")
            .unwrap()
            .body
            .clone();
        assert!(record.contains("token_type_hint=refresh_token"));
        assert!(record.contains("client_id=oaiapp_synthetic"));
    }
}
