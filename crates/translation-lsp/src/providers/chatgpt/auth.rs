use crate::providers::error::{Cause, credentials::CredentialError};
mod callback;
#[cfg(test)]
use callback::callback;
use callback::wait_callback;
use std::{collections::HashMap, time::Duration};

use openidconnect::{
    AdditionalProviderMetadata, AuthType, AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce,
    NonceVerifier, OAuth2TokenResponse, PkceCodeChallenge, ProviderMetadata, RedirectUrl,
    RefreshToken, RequestTokenError, RevocationUrl, Scope, StandardErrorResponse, TokenResponse,
    core::*,
};
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, task::JoinSet};
use uuid::Uuid;

use crate::providers::{
    AuthUiState, Interaction,
    error::{Error, ErrorKind},
};

use super::{
    auth_store::{AuthStore, Credentials, Registration, now},
    http::{Http, response_error},
};

pub(super) fn account_changed() -> Error {
    Cause::Modified("ChatGPT account or authorization").into()
}

const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: [&str; 6] = [
    "openid",
    "profile",
    "email",
    "offline_access",
    "resource.invoke",
    "chatgpt.tokens.use.direct",
];

#[derive(Clone, Deserialize, Serialize, Debug)]
struct DiscoveryExtra {
    revocation_endpoint: Option<RevocationUrl>,
}
impl AdditionalProviderMetadata for DiscoveryExtra {}
type Metadata = ProviderMetadata<
    DiscoveryExtra,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

#[derive(Clone)]
pub(crate) struct Auth {
    pub store: AuthStore,
    pub http: Http,
    issuer: IssuerUrl,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountKey {
    pub registration: Uuid,
    pub authorization_id: Uuid,
    issuer: String,
    subject: String,
    client_id: String,
}

impl AccountKey {
    fn for_registration(id: Uuid, registration: &Registration) -> Self {
        Self {
            registration: id,
            authorization_id: registration.authorization_id,
            issuer: registration.issuer.clone(),
            subject: registration.subject.clone(),
            client_id: registration.client_id.clone(),
        }
    }
}

pub(crate) struct Access {
    pub key: AccountKey,
    pub credentials: Credentials,
}

impl Auth {
    pub fn production(http: Http) -> Result<Self, Error> {
        Ok(Self {
            store: AuthStore::production()?,
            http,
            issuer: IssuerUrl::new("https://auth.openai.com".into()).unwrap(),
        })
    }

    #[cfg(test)]
    pub fn synthetic(store: AuthStore, http: Http, issuer: String) -> Self {
        Self {
            store,
            http,
            issuer: IssuerUrl::new(issuer).unwrap(),
        }
    }

    pub fn ui_state(&self) -> AuthUiState {
        let snapshot = match self.store.snapshot() {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => return AuthUiState::SignedOut,
            Err(_) => return AuthUiState::Unavailable,
        };
        let Some(id) = snapshot.state.active_registration else {
            return AuthUiState::SignedOut;
        };
        let registration = &snapshot.state.registrations[&id];
        if registration.issuer != self.issuer.as_str() {
            return AuthUiState::Unavailable;
        }
        match &registration.credentials {
            None => AuthUiState::SignedOut,
            Some(credentials) if credentials.plan_enabled() => AuthUiState::Ready,
            Some(_) => AuthUiState::PlanDisabled,
        }
    }

    pub fn key(&self) -> Result<AccountKey, Error> {
        let snapshot = self
            .store
            .snapshot()?
            .ok_or_else(|| Error::failed("Use Continue with ChatGPT before translating"))?;
        let id = snapshot
            .state
            .active_registration
            .ok_or_else(|| Error::failed("Choose a ChatGPT account before translating"))?;
        let registration = &snapshot.state.registrations[&id];
        if registration.issuer != self.issuer.as_str() {
            return Err(Error::failed("The selected account has a different issuer"));
        }
        let credentials = registration.credentials.as_ref().ok_or_else(|| {
            Error::failed("Use Continue with ChatGPT / Choose account to reconnect the saved registration before translating")
        })?;
        if !credentials.plan_enabled() {
            return Err(Error::failed(
                "ChatGPT plan use is disabled. Explicitly choose Enable ChatGPT plan use",
            ));
        }
        Ok(AccountKey::for_registration(id, registration))
    }

    async fn discovery(&self) -> Result<Metadata, Error> {
        let metadata =
            Metadata::discover_async(self.issuer.clone(), &|request| self.http.oidc(request))
                .await
                .map_err(|error| match error {
                    openidconnect::DiscoveryError::Request(error) => error,
                    _ => Error::failed(
                        "OpenAI OIDC discovery or JWKS validation failed; retry later",
                    ),
                })?;
        if self.issuer.as_str() == "https://auth.openai.com"
            && metadata.authorization_endpoint().as_str()
                != "https://auth.openai.com/api/accounts/authorize"
        {
            return Err(Error::failed(
                "OpenAI discovery returned an unexpected authorization endpoint",
            ));
        }
        Ok(metadata)
    }

    pub async fn connect(
        &self,
        ui: &impl Interaction,
        selected: Option<Uuid>,
        enable: bool,
    ) -> Result<String, Error> {
        // Create the host only after the explicit command, then release the lock
        // while the user interacts with the browser.
        let (host, previous, before) = {
            let locked = self.store.lock(true).await?.unwrap();
            let previous =
                selected
                    .map(|id| {
                        locked.state.registrations.get(&id).cloned().ok_or_else(|| {
                            Error::failed("The saved ChatGPT registration was removed")
                        })
                    })
                    .transpose()?;
            let revision = match locked.revision()? {
                Some(revision) => revision,
                None => locked.save()?,
            };
            (locked.state.ext_agent_host_id.clone(), previous, revision)
        };
        if previous
            .as_ref()
            .is_some_and(|account| account.issuer != self.issuer.as_str())
        {
            return Err(Error::failed(
                "The saved ChatGPT account has a different issuer",
            ));
        }
        let metadata = self.discovery().await?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| Error::failed("Could not open the local ChatGPT callback listener"))?;
        let address = listener
            .local_addr()
            .map_err(|_| Error::failed("Could not read the callback port"))?;
        let redirect =
            RedirectUrl::new(format!("http://127.0.0.1:{}/auth/callback", address.port())).unwrap();
        let initial_id = previous
            .as_ref()
            .map_or("dynamic_agent_client", |account| account.client_id.as_str());
        let client = CoreClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(initial_id.into()),
            None,
        )
        .set_auth_type(AuthType::RequestBody)
        .set_redirect_uri(redirect.clone());
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut authorize = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge)
            .add_extra_param("resource", RESOURCE)
            .add_extra_param("ext_agent_host_id", &host);
        for scope in SCOPES.into_iter().filter(|scope| *scope != "openid") {
            authorize = authorize.add_scope(Scope::new(scope.into()));
        }
        if let Some(previous) = &previous {
            authorize = authorize.add_extra_param("login_hint", &previous.email);
            if let Some(credentials) = &previous.credentials {
                authorize = authorize.add_extra_param("id_token_hint", &credentials.id_token);
            }
        } else {
            authorize = authorize.add_extra_param("agent_name_hint", "Translation LSP");
        }
        if enable {
            authorize = authorize.add_extra_param("prompt", "consent");
        }
        let (url, state, nonce) = authorize.url();
        let returning_id = previous.as_ref().map(|account| account.client_id.clone());
        let mut callbacks = JoinSet::new();
        callbacks.spawn(wait_callback(listener, state, returning_id));
        // URLs are sent only to the editor/browser, never to diagnostics.
        ui.open(url.as_str()).await?;
        let callback = callbacks
            .join_next()
            .await
            .ok_or_else(|| Error::failed("Callback listener stopped"))?
            .map_err(|_| Error::failed("Callback listener stopped"))??;
        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(callback.client_id.clone()),
            None,
        )
        .set_auth_type(AuthType::RequestBody)
        .set_redirect_uri(redirect);
        let response = client
            .exchange_code(AuthorizationCode::new(callback.code))
            .map_err(|_| Error::failed("OIDC discovery did not provide a token endpoint"))?
            .set_pkce_verifier(verifier)
            .add_extra_param("resource", RESOURCE)
            .request_async(&|request| self.http.oidc(request))
            .await
            .map_err(token_error)?;
        let token = response
            .id_token()
            .ok_or_else(|| Error::failed("OpenAI did not return an ID token"))?;
        let claims = token
            .claims(&client.id_token_verifier(), &nonce)
            .map_err(|_| {
                Error::failed(
                    "ChatGPT ID token signature, issuer, audience, expiry or nonce is invalid",
                )
            })?;
        if previous.as_ref().is_some_and(|previous| {
            previous.issuer != claims.issuer().as_str()
                || previous.subject != claims.subject().as_str()
        }) {
            return Err(Error::failed(
                "The returning ChatGPT identity does not match this registration",
            ));
        }
        let registration = Registration {
            authorization_id: Uuid::new_v4(),
            issuer: claims.issuer().to_string(),
            subject: claims.subject().to_string(),
            email: claims
                .email()
                .map_or_else(String::new, |email| email.as_str().to_owned()),
            client_id: callback.client_id,
            credentials: initial_credentials(&response)?,
        };
        let enabled = registration
            .credentials
            .as_ref()
            .is_some_and(Credentials::plan_enabled);
        let mut locked = self.store.lock(false).await?.ok_or_else(account_changed)?;
        let current = locked.revision()?;
        if current.as_ref() != Some(&before) {
            return Err(account_changed());
        }
        let id = selected.unwrap_or_else(Uuid::new_v4);
        locked.state.registrations.insert(id, registration);
        locked.state.active_registration = Some(id);
        locked.save()?;
        let message = if enabled {
            "ChatGPT is connected. Hover over a comment or translate a selection."
        } else {
            concat!(
                "Signed in to ChatGPT. Plan use is disabled; ",
                "choose Enable ChatGPT plan use to authorize translation."
            )
        };
        Ok(message.into())
    }

    pub async fn access(&self, expected: &AccountKey) -> Result<Access, Error> {
        // Unchanged credentials are a read. Refresh rechecks under the writer lock.
        match self.store.snapshot() {
            Ok(Some(snapshot)) => {
                if snapshot.state.active_registration != Some(expected.registration)
                    || snapshot
                        .state
                        .registrations
                        .get(&expected.registration)
                        .is_none_or(|registration| {
                            AccountKey::for_registration(expected.registration, registration)
                                != *expected
                        })
                {
                    return Err(account_changed());
                }
                let credentials = snapshot
                    .state
                    .registrations
                    .get(&expected.registration)
                    .and_then(|registration| registration.credentials.as_ref())
                    .ok_or_else(account_changed)?;
                if !credentials.plan_enabled() {
                    return Err(account_changed());
                }
                if !credentials.needs_refresh() {
                    return Ok(Access {
                        key: expected.clone(),
                        credentials: credentials.clone(),
                    });
                }
            }
            Ok(None) => return Err(account_changed()),
            Err(CredentialError::Busy) => (),
            Err(error) => return Err(Error::from(error)),
        }
        let mut locked = self.store.lock(false).await?.ok_or_else(account_changed)?;
        if locked.state.active_registration != Some(expected.registration)
            || locked
                .state
                .registrations
                .get(&expected.registration)
                .is_none_or(|registration| {
                    AccountKey::for_registration(expected.registration, registration) != *expected
                })
        {
            return Err(account_changed());
        }
        let registration = locked
            .state
            .registrations
            .get_mut(&expected.registration)
            .ok_or_else(account_changed)?;
        let previous = registration
            .credentials
            .clone()
            .ok_or_else(account_changed)?;
        if !previous.plan_enabled() {
            return Err(account_changed());
        }
        if previous.needs_refresh() {
            let metadata = self.discovery().await?;
            let client = CoreClient::from_provider_metadata(
                metadata,
                ClientId::new(registration.client_id.clone()),
                None,
            )
            .set_auth_type(AuthType::RequestBody);
            let refreshed = client
                .exchange_refresh_token(&RefreshToken::new(previous.refresh_token.clone()))
                .map_err(|_| Error::failed("OIDC discovery did not provide a token endpoint"))?
                .add_extra_param("resource", RESOURCE)
                .request_async(&|request| self.http.oidc(request))
                .await;
            let response = match refreshed {
                Ok(response) => response,
                Err(error) => {
                    let error = token_error(error);
                    if error.kind() == ErrorKind::RefreshUnusable {
                        registration.credentials = None;
                        locked.save()?;
                    }
                    return Err(error);
                }
            };
            if let Some(token) = response.id_token() {
                // The saved token gives the identity and nonce of the sign-in. Its
                // signature and expiry were checked before it was saved, and are not
                // checked again: OpenAI may have retired the key that signed it, and
                // the refresh token it is stored with is trusted the same way. Read
                // no other token without checking its signature.
                let old: CoreIdToken =
                    serde_json::from_value(serde_json::Value::String(previous.id_token.clone()))
                        .map_err(|_| Error::failed("Saved ID token is invalid"))?;
                let mut original_nonce = None;
                let historical = client
                    .id_token_verifier()
                    .insecure_disable_signature_check()
                    .set_time_fn(|| std::time::UNIX_EPOCH.into());
                let old_claims = old
                    .claims(&historical, |nonce: Option<&Nonce>| {
                        original_nonce = nonce.cloned();
                        Ok(())
                    })
                    .map_err(|_| Error::failed("Saved ID token could not be verified"))?;
                if old_claims.issuer().as_str() != registration.issuer
                    || old_claims.subject().as_str() != registration.subject
                {
                    return Err(Error::failed(
                        "Saved ChatGPT identity does not match the registration",
                    ));
                }
                let claims = token
                    .claims(&client.id_token_verifier(), |nonce: Option<&Nonce>| {
                        if nonce.is_none() {
                            return Ok(());
                        }
                        original_nonce
                            .as_ref()
                            .ok_or_else(|| "Unexpected refresh nonce".to_owned())?
                            .verify(nonce)
                    })
                    .map_err(|_| Error::failed("Refreshed ChatGPT ID token is invalid"))?;
                if claims.issuer().as_str() != registration.issuer
                    || claims.subject().as_str() != registration.subject
                {
                    return Err(Error::failed(
                        "Refreshed ChatGPT identity does not match the registration",
                    ));
                }
            }
            registration.credentials = Some(credentials(&response, Some(&previous))?);
            locked.save()?;
        }
        let credentials = locked.state.registrations[&expected.registration]
            .credentials
            .clone()
            .unwrap();
        if !credentials.plan_enabled() {
            return Err(Error::failed(
                "ChatGPT plan use is disabled; choose Enable ChatGPT plan use",
            ));
        }
        Ok(Access {
            key: expected.clone(),
            credentials,
        })
    }

    pub async fn sign_out(&self) -> Result<String, Error> {
        let Some(mut locked) = self.store.lock(false).await? else {
            return Ok("No ChatGPT account is connected.".into());
        };
        let Some(id) = locked.state.active_registration else {
            return Ok("No ChatGPT account is selected.".into());
        };
        let registration = &locked.state.registrations[&id];
        let mut confirmed = registration.credentials.is_none();
        if let Some(credentials) = &registration.credentials {
            for attempt in 0..3 {
                let result = async {
                    let metadata = self.discovery().await?;
                    let endpoint = metadata
                        .additional_metadata()
                        .revocation_endpoint
                        .clone()
                        .ok_or_else(|| {
                            Error::failed("OpenAI did not provide a revocation endpoint")
                        })?;
                    let client = CoreClient::from_provider_metadata(
                        metadata,
                        ClientId::new(registration.client_id.clone()),
                        None,
                    )
                    .set_auth_type(AuthType::RequestBody)
                    .set_revocation_url(endpoint);
                    client
                        .revoke_token(CoreRevocableToken::RefreshToken(RefreshToken::new(
                            credentials.refresh_token.clone(),
                        )))
                        .map_err(|_| Error::failed("OpenAI revocation endpoint is invalid"))?
                        .request_async(&|request| self.http.oidc(request))
                        .await
                        .map_err(|error| match error {
                            RequestTokenError::Request(error) => error,
                            _ => Error::failed("Remote ChatGPT revocation was not confirmed"),
                        })
                }
                .await;
                match result {
                    Ok(()) => {
                        confirmed = true;
                        break;
                    }
                    Err(error) if error.kind() == ErrorKind::Temporary => (),
                    Err(_) => break,
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                }
            }
        }
        locked.state.registrations.get_mut(&id).unwrap().credentials = None;
        locked.save()?;
        let message = if confirmed {
            "Signed out of this ChatGPT registration."
        } else {
            concat!(
                "Signed out locally. Remote revocation was not confirmed; ",
                "disconnect this app in ChatGPT Settings if needed."
            )
        };
        Ok(message.into())
    }

    pub async fn choose_account(
        &self,
        ui: &impl Interaction,
        enable: bool,
    ) -> Result<String, Error> {
        let snapshot = self.store.snapshot()?;
        let mut choices = Vec::new();
        let mut ids = HashMap::new();
        if let Some(snapshot) = snapshot {
            for (id, registration) in snapshot.state.registrations {
                let label = format!(
                    "{} [{}]{}",
                    registration.email,
                    id,
                    if snapshot.state.active_registration == Some(id) {
                        " (active)"
                    } else {
                        ""
                    }
                );
                ids.insert(label.clone(), id);
                choices.push(label);
            }
        }
        choices.push("Add another ChatGPT account".into());
        let Some(choice) = ui
            .choose(
                "Continue with ChatGPT: choose a saved registration or add an account.",
                &choices,
            )
            .await?
        else {
            return Ok("ChatGPT sign-in cancelled.".into());
        };
        self.connect(ui, ids.get(&choice).copied(), enable).await
    }
}

fn initial_credentials(response: &CoreTokenResponse) -> Result<Option<Credentials>, Error> {
    let renewable_or_direct = response.scopes().is_some_and(|scopes| {
        scopes.iter().any(|scope| {
            matches!(
                scope.as_str(),
                "offline_access" | "chatgpt.tokens.use.direct"
            )
        })
    });
    if response.refresh_token().is_none() && !renewable_or_direct {
        // A verified identity-only grant cannot be used for inference. Retain
        // its registration without incomplete credentials or empty secrets.
        Ok(None)
    } else {
        credentials(response, None).map(Some)
    }
}

fn credentials(
    response: &CoreTokenResponse,
    previous: Option<&Credentials>,
) -> Result<Credentials, Error> {
    let id_token = response
        .id_token()
        .map(ToString::to_string)
        .or_else(|| previous.map(|credentials| credentials.id_token.clone()))
        .ok_or_else(|| Error::failed("OpenAI did not return an ID token"))?;
    let refresh_token = response
        .refresh_token()
        .map(|token| token.secret().clone())
        .ok_or_else(|| Error::failed("OpenAI did not return a replacement refresh token"))?;
    let scopes = response
        .scopes()
        .map(|scopes| {
            scopes
                .iter()
                .map(|scope| scope.as_str().to_owned())
                .collect()
        })
        .or_else(|| previous.map(|credentials| credentials.scopes.clone()))
        .unwrap_or_default();
    if *response.token_type() != CoreTokenType::Bearer
        || response.access_token().secret().is_empty()
        || refresh_token.is_empty()
    {
        return Err(Error::failed("OpenAI returned unusable credentials"));
    }
    Ok(Credentials {
        id_token,
        access_token: response.access_token().secret().clone(),
        refresh_token,
        token_type: "Bearer".into(),
        scopes,
        expires_in: response
            .expires_in()
            .ok_or_else(|| Error::failed("OpenAI did not return token expiry"))?
            .as_secs(),
        saved_at: now(),
    })
}

fn token_error(
    error: RequestTokenError<Error, StandardErrorResponse<CoreErrorResponseType>>,
) -> Error {
    match error {
        RequestTokenError::Request(error) => error,
        RequestTokenError::ServerResponse(response) => response_error(
            400,
            &serde_json::json!({"error":response.error().as_ref(),"error_description":"ChatGPT token exchange failed"}),
            None,
            &[],
        ),
        _ => Error::failed("ChatGPT token response is invalid; existing credentials are retained"),
    }
}

#[cfg(test)]
mod tests {
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
        assert!(
            parse(
                "state=expected&code=c&client_id=oaiapp_new",
                "/auth/callback",
                None
            )
            .is_ok()
        );
        assert!(
            parse(
                "state=expected&code=c",
                "/auth/callback",
                Some("oaiapp_saved")
            )
            .is_ok()
        );
        for query in [
            "state=wrong&code=c&client_id=oaiapp_new",
            "state=expected&state=expected&code=c&client_id=oaiapp_new",
            "state=expected&code=c&code=d&client_id=oaiapp_new",
            "state=expected&code=c",
            "state=expected&code=c&client_id=dynamic_agent_client",
            "state=expected&error=access_denied",
        ] {
            assert!(parse(query, "/auth/callback", None).is_err());
        }
        assert!(
            parse(
                "state=expected&code=c&client_id=oaiapp_other",
                "/auth/callback",
                Some("oaiapp_saved")
            )
            .is_err()
        );
        assert!(parse("state=expected&code=c&client_id=oaiapp_new", "/wrong", None).is_err());
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
        fixture.state.lock().unwrap().refresh_failure =
            Some((400, json!({"error":"invalid_grant"})));
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
        let directory =
            std::path::PathBuf::from(std::env::var("ZED_SYNTHETIC_REFRESH_DIR").unwrap());
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
        let directory =
            std::env::temp_dir().join(format!("zed-auth-multiprocess-{}", Uuid::new_v4()));
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
        let credentials = snapshot.state.registrations
            [&snapshot.state.active_registration.unwrap()]
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
}
