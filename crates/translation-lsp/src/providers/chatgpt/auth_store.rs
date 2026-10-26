use crate::providers::{
    credential_directory::{self, Revision, check_path},
    error::credentials::CredentialError,
};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions, TryLockError},
    io::Read,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use uuid::Uuid;

const MAX_AUTH_BYTES: u64 = 1024 * 1024;
pub(crate) const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";

#[derive(Clone)]
pub(crate) struct AuthStore {
    directory: PathBuf,
}

/// Credential-bearing types deliberately have no Debug implementation.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthState {
    pub version: u32,
    pub ext_agent_host_id: String,
    pub active_registration: Option<Uuid>,
    pub registrations: BTreeMap<Uuid, Registration>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Registration {
    pub authorization_id: Uuid,
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Credentials>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credentials {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub scopes: Vec<String>,
    pub expires_in: u64,
    pub saved_at: u64,
}

pub(crate) struct Snapshot {
    pub state: AuthState,
    pub revision: Revision,
}

pub(crate) struct LockedStore {
    lock: File,
    store: AuthStore,
    pub state: AuthState,
}

impl Credentials {
    pub fn plan_enabled(&self) -> bool {
        self.token_type.eq_ignore_ascii_case("Bearer")
            && self.scopes.iter().any(|scope| scope == DIRECT_SCOPE)
    }

    pub fn needs_refresh(&self) -> bool {
        self.saved_at.saturating_add(self.expires_in) <= now().saturating_add(30)
    }
}

impl AuthState {
    fn new() -> Self {
        Self {
            version: 1,
            ext_agent_host_id: Uuid::new_v4().urn().to_string(),
            active_registration: None,
            registrations: BTreeMap::new(),
        }
    }

    fn validate(&self) -> Result<(), CredentialError> {
        if self.version != 1
            || !self.ext_agent_host_id.starts_with("urn:uuid:")
            || Uuid::parse_str(&self.ext_agent_host_id).is_err()
            || self
                .active_registration
                .is_some_and(|id| !self.registrations.contains_key(&id))
        {
            return Err(CredentialError::Invalid);
        }
        for registration in self.registrations.values() {
            if registration.authorization_id.is_nil()
                || registration.issuer.is_empty()
                || registration.subject.is_empty()
                || registration.client_id.is_empty()
                || registration.client_id == "dynamic_agent_client"
            {
                return Err(CredentialError::Invalid);
            }
            if let Some(credentials) = &registration.credentials
                && (credentials.id_token.is_empty()
                    || credentials.access_token.is_empty()
                    || credentials.refresh_token.is_empty()
                    || !credentials.token_type.eq_ignore_ascii_case("Bearer"))
            {
                return Err(CredentialError::Invalid);
            }
        }
        Ok(())
    }
}

impl AuthStore {
    pub fn production() -> Result<Self, CredentialError> {
        Ok(Self {
            directory: credential_directory::locate()?,
        })
    }

    #[cfg(test)]
    pub fn synthetic(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Atomic rename permits coherent reads without holding a lock over display.
    /// Check a stable lock separately so another process's signout/refresh can
    /// suppress a cache hit while that credential operation is in progress.
    pub fn snapshot(&self) -> Result<Option<Snapshot>, CredentialError> {
        if !check_path(&self.directory, true)? {
            return Ok(None);
        }
        let Some(lock) = self.open_lock(false)? else {
            return Ok(None);
        };
        match lock.try_lock_shared() {
            Ok(()) => self.read(),
            Err(TryLockError::WouldBlock) => Err(CredentialError::Busy),
            Err(TryLockError::Error(source)) => Err(CredentialError::io("lock", source)),
        }
    }

    pub async fn lock(&self, create: bool) -> Result<Option<LockedStore>, CredentialError> {
        if create {
            credential_directory::create(&self.directory)?;
        } else if !check_path(&self.directory, true)? {
            return Ok(None);
        }
        let Some(lock) = self.open_lock(create)? else {
            return Ok(None);
        };
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => sleep(Duration::from_millis(25)).await,
                Err(TryLockError::Error(source)) => {
                    return Err(CredentialError::io("lock", source));
                }
            }
        }
        let state = self
            .read()?
            .map_or_else(AuthState::new, |snapshot| snapshot.state);
        Ok(Some(LockedStore {
            lock,
            store: self.clone(),
            state,
        }))
    }

    fn open_lock(&self, create: bool) -> Result<Option<File>, CredentialError> {
        let path = self.directory.join("auth.lock");
        if !check_path(&path, false)? && !create {
            return Ok(None);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)
            .map(Some)
            .map_err(|source| CredentialError::io("open lock", source))
    }

    fn read(&self) -> Result<Option<Snapshot>, CredentialError> {
        let path = self.directory.join("auth.json");
        if !check_path(&path, false)? {
            return Ok(None);
        }
        let mut file =
            File::open(path).map_err(|source| CredentialError::io("open credentials", source))?;
        let metadata = file
            .metadata()
            .map_err(|source| CredentialError::io("read metadata", source))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_AUTH_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| CredentialError::io("read credentials", source))?;
        if bytes.len() as u64 > MAX_AUTH_BYTES {
            return Err(CredentialError::Invalid);
        }
        let state: AuthState = serde_json::from_slice(&bytes).map_err(CredentialError::Json)?;
        state.validate()?;
        Ok(Some(Snapshot {
            state,
            revision: Revision::of(&metadata)?,
        }))
    }
}

impl LockedStore {
    pub fn revision(&self) -> Result<Option<Revision>, CredentialError> {
        self.store
            .read()
            .map(|snapshot| snapshot.map(|snapshot| snapshot.revision))
    }
    pub fn save(&self) -> Result<Revision, CredentialError> {
        self.state.validate()?;
        let bytes = serde_json::to_vec(&self.state).map_err(CredentialError::Json)?;
        if bytes.len() as u64 > MAX_AUTH_BYTES {
            return Err(CredentialError::Invalid);
        }
        Revision::of(&credential_directory::replace(
            &self.store.directory,
            "auth.json",
            &bytes,
        )?)
    }
}

impl Drop for LockedStore {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use tokio::time::timeout;

    use crate::providers::{Intent, Output};

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn read_only_snapshots_coexist_and_exclude_credential_writers() {
        let directory = env::temp_dir().join(format!("zed-auth-readers-{}", Uuid::new_v4()));
        let store = AuthStore::synthetic(directory.clone());
        let locked = store.lock(true).await.unwrap().unwrap();
        let revision = locked.save().unwrap();
        drop(locked);
        let reader = store.open_lock(false).unwrap().unwrap();
        reader.try_lock_shared().unwrap();
        assert_eq!(store.snapshot().unwrap().unwrap().revision, revision);
        assert!(
            timeout(Duration::from_millis(60), store.lock(false))
                .await
                .is_err()
        );
        drop(reader);
        let writer = store.lock(false).await.unwrap().unwrap();
        assert!(matches!(store.snapshot(), Err(CredentialError::Busy)));
        drop(writer);
        assert_eq!(store.snapshot().unwrap().unwrap().revision, revision);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_only_access_preserves_rewrites_and_rejects_new_authorization() {
        use super::super::{ChatGptProvider, auth::Auth, http::Http, test_support::Fixture};
        use crate::providers::{TranslationProvider, error::ErrorKind};
        let fixture = Fixture::new().await;
        fixture.state.lock().unwrap().response_mode = "delta_compact".into();
        let directory = env::temp_dir().join(format!("zed-auth-access-readers-{}", Uuid::new_v4()));
        let store = AuthStore::synthetic(directory.clone());
        let auth = Auth::synthetic(
            store.clone(),
            Http::synthetic_revocation(&fixture.base),
            fixture.base.clone(),
        );
        fixture.connected(&auth, 3600).await;
        let provider = ChatGptProvider::synthetic(
            auth.clone(),
            format!("{}/v1", fixture.base),
            "synthetic-model".into(),
        );
        let context = provider.context().unwrap();
        let reader = store.open_lock(false).unwrap().unwrap();
        reader.try_lock_shared().unwrap();
        let translated = timeout(
            Duration::from_secs(5),
            provider.translate(
                Intent::Automatic,
                Output::Range,
                "Shared source.",
                "ja",
                context.clone(),
            ),
        )
        .await
        .expect("A valid read must not need an exclusive credential lock")
        .unwrap();
        assert_eq!(translated.context, context);
        assert!(translated.text.contains("Shared source."));
        assert_eq!(fixture.state.lock().unwrap().refreshes, 0);
        drop(reader);
        let writer = store.lock(false).await.unwrap().unwrap();
        assert!(provider.context().is_err());
        writer.save().unwrap();
        drop(writer);
        assert_eq!(provider.context().unwrap(), context);
        let mut writer = store.lock(false).await.unwrap().unwrap();
        let id = writer.state.active_registration.unwrap();
        writer
            .state
            .registrations
            .get_mut(&id)
            .unwrap()
            .authorization_id = Uuid::new_v4();
        writer.save().unwrap();
        drop(writer);
        let stale = provider
            .translate(
                Intent::Automatic,
                Output::Range,
                "Stale source.",
                "ja",
                context,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(stale.kind(), ErrorKind::Modified);
        assert_eq!(
            fixture
                .state
                .lock()
                .unwrap()
                .records
                .iter()
                .filter(|r| r.path == "/v1/responses")
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn store_rejects_records_other_than_version_1() {
        use super::super::{auth::Auth, http::Http, test_support::Fixture};
        let fixture = Fixture::new().await;
        let directory = env::temp_dir().join(format!("zed-auth-version-{}", Uuid::new_v4()));
        let store = AuthStore::synthetic(directory.clone());
        let auth = Auth::synthetic(
            store.clone(),
            Http::synthetic_revocation(&fixture.base),
            fixture.base.clone(),
        );
        fixture.connected(&auth, 3600).await;
        let state = store.snapshot().unwrap().unwrap().state;
        let current = serde_json::to_value(&state).unwrap();

        let mut version_2 = current.clone();
        version_2["version"] = 2.into();
        fs::write(
            directory.join("auth.json"),
            serde_json::to_vec(&version_2).unwrap(),
        )
        .unwrap();
        assert!(matches!(store.snapshot(), Err(CredentialError::Invalid)));

        let mut without_authorization = current.clone();
        for registration in without_authorization["registrations"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            registration
                .as_object_mut()
                .unwrap()
                .remove("authorization_id");
        }
        fs::write(
            directory.join("auth.json"),
            serde_json::to_vec(&without_authorization).unwrap(),
        )
        .unwrap();
        assert!(matches!(store.snapshot(), Err(CredentialError::Json(_))));

        let mut nil = state;
        nil.registrations
            .values_mut()
            .next()
            .unwrap()
            .authorization_id = Uuid::nil();
        assert!(matches!(nil.validate(), Err(CredentialError::Invalid)));
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn sign_out_clears_credentials_when_revocation_fails() {
        use super::super::{auth::Auth, http::Http, test_support::Fixture};
        let fixture = Fixture::new().await;
        let directory = env::temp_dir().join(format!("zed-auth-signout-{}", Uuid::new_v4()));
        let store = AuthStore::synthetic(directory.clone());
        let auth = Auth::synthetic(
            store.clone(),
            Http::synthetic_revocation(&fixture.base),
            fixture.base.clone(),
        );
        fixture.connected(&auth, 3600).await;
        fixture.state.lock().unwrap().revoke_failure = true;
        auth.sign_out().await.unwrap();
        let signed_out = store.snapshot().unwrap().unwrap();
        assert!(
            signed_out
                .state
                .registrations
                .values()
                .all(|registration| registration.credentials.is_none())
        );
        let mut files: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        files.sort();
        assert_eq!(files, ["auth.json", "auth.lock"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn store_is_lazy_atomic_and_lock_wait_is_cancellable() {
        let directory = env::temp_dir().join(format!("zed-auth-{}", Uuid::new_v4()));
        let store = AuthStore::synthetic(directory.clone());
        assert!(store.snapshot().unwrap().is_none());
        assert!(!directory.exists());
        let locked = store.lock(true).await.unwrap().unwrap();
        let host = locked.state.ext_agent_host_id.clone();
        locked.save().unwrap();
        assert!(matches!(store.snapshot(), Err(CredentialError::Busy)));
        assert!(
            timeout(Duration::from_millis(60), store.lock(false))
                .await
                .is_err()
        );
        drop(locked);
        assert_eq!(
            store.snapshot().unwrap().unwrap().state.ext_agent_host_id,
            host
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for name in ["auth.json", "auth.lock"] {
                assert_eq!(
                    fs::metadata(directory.join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
