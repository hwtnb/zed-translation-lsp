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
    auth_store::{AuthState, AuthStore, Credentials, Registration, now},
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
    #[cfg(test)]
    pub fn synthetic() -> Self {
        Self {
            registration: Uuid::nil(),
            authorization_id: Uuid::nil(),
            issuer: "https://issuer.invalid".into(),
            subject: "synthetic".into(),
            client_id: "synthetic".into(),
        }
    }

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

/// The selected account as the credential store records it.
enum Selection {
    NoStore,
    NoAccount,
    OtherIssuer,
    NoCredentials,
    PlanDisabled,
    Ready(AccountKey),
}

/// The credentials of `expected`, which must still be the selected account with
/// plan use enabled.
fn expected_credentials<'a>(
    state: &'a AuthState,
    expected: &AccountKey,
) -> Result<&'a Credentials, Error> {
    state
        .registrations
        .get(&expected.registration)
        .filter(|registration| {
            state.active_registration == Some(expected.registration)
                && AccountKey::for_registration(expected.registration, registration) == *expected
        })
        .and_then(|registration| registration.credentials.as_ref())
        .filter(|credentials| credentials.plan_enabled())
        .ok_or_else(account_changed)
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

    fn selection(&self) -> Result<Selection, CredentialError> {
        let Some(snapshot) = self.store.snapshot()? else {
            return Ok(Selection::NoStore);
        };
        let Some(id) = snapshot.state.active_registration else {
            return Ok(Selection::NoAccount);
        };
        let registration = &snapshot.state.registrations[&id];
        if registration.issuer != self.issuer.as_str() {
            return Ok(Selection::OtherIssuer);
        }
        Ok(match &registration.credentials {
            None => Selection::NoCredentials,
            Some(credentials) if credentials.plan_enabled() => {
                Selection::Ready(AccountKey::for_registration(id, registration))
            }
            Some(_) => Selection::PlanDisabled,
        })
    }

    pub fn ui_state(&self) -> AuthUiState {
        match self.selection() {
            Ok(Selection::NoStore | Selection::NoAccount | Selection::NoCredentials) => {
                AuthUiState::SignedOut
            }
            Ok(Selection::PlanDisabled) => AuthUiState::PlanDisabled,
            Ok(Selection::Ready(_)) => AuthUiState::Ready,
            Ok(Selection::OtherIssuer) | Err(_) => AuthUiState::Unavailable,
        }
    }

    pub fn key(&self) -> Result<AccountKey, Error> {
        let message = match self.selection()? {
            Selection::Ready(key) => return Ok(key),
            Selection::NoStore => "Use Continue with ChatGPT before translating",
            Selection::NoAccount => "Choose a ChatGPT account before translating",
            Selection::OtherIssuer => "The selected account has a different issuer",
            Selection::NoCredentials => {
                "Use Continue with ChatGPT / Choose account to reconnect the saved registration before translating"
            }
            Selection::PlanDisabled => {
                "ChatGPT plan use is disabled. Explicitly choose Enable ChatGPT plan use"
            }
        };
        Err(Error::failed(message))
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
                let credentials = expected_credentials(&snapshot.state, expected)?;
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
        let previous = expected_credentials(&locked.state, expected)?.clone();
        let registration = locked
            .state
            .registrations
            .get_mut(&expected.registration)
            .expect("expected_credentials found the registration");
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
mod tests;
