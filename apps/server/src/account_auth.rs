//! Single-owner web authentication. This installation does not provide tenant isolation.
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::{extract::State, http::StatusCode, Json};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use wealthfolio_core::secrets::SecretStore;

use crate::{
    auth::{AuthError, AuthState},
    secrets::FileSecretStore,
};

const ACCOUNT_KEY: &str = "web_owner_account_v1";
const RECOVERY_TTL: u64 = 600;
const RECOVERY_COOLDOWN: u64 = 60;
const MAX_ATTEMPTS: u8 = 5;

#[derive(Clone)]
pub struct AccountAuthConfig {
    pub path: PathBuf,
    pub encryption_key: [u8; 32],
    pub invitation_hash: String,
    pub resend_api_key: String,
    pub email_from: String,
}

impl AccountAuthConfig {
    pub fn from_env(path: PathBuf, encryption_key: [u8; 32]) -> anyhow::Result<Option<Self>> {
        let enabled = match std::env::var("WF_ACCOUNT_AUTH") {
            Err(std::env::VarError::NotPresent) => false,
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => anyhow::bail!("WF_ACCOUNT_AUTH must be true or false"),
            },
            Err(_) => anyhow::bail!("WF_ACCOUNT_AUTH must be valid UTF-8"),
        };
        if !enabled {
            return Ok(None);
        }
        let invitation_hash = std::env::var("WF_SIGNUP_INVITE_HASH")?;
        anyhow::ensure!(
            invitation_hash.len() == 64 && invitation_hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "WF_SIGNUP_INVITE_HASH must be a SHA-256 hex digest"
        );
        let resend_api_key = std::env::var("WF_RESEND_API_KEY")?;
        let email_from = std::env::var("WF_AUTH_EMAIL_FROM")?;
        anyhow::ensure!(
            !resend_api_key.trim().is_empty() && !email_from.trim().is_empty(),
            "Resend key and sender are required"
        );
        Ok(Some(Self {
            path,
            encryption_key,
            invitation_hash: invitation_hash.to_ascii_lowercase(),
            resend_api_key,
            email_from,
        }))
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Recovery {
    hash: String,
    expires_at: u64,
    attempts: u8,
}

#[derive(Clone, Serialize, Deserialize)]
struct Owner {
    email: String,
    password_hash: String,
    version: u64,
    recovery: Option<Recovery>,
    last_recovery_at: u64,
}

pub struct AccountAuth {
    config: AccountAuthConfig,
    owner: Mutex<Option<Owner>>,
    version: AtomicU64,
    // Bound Argon2 work across peers, in addition to per-IP request limits.
    pub work: Arc<tokio::sync::Semaphore>,
    pub mail_client: reqwest::Client,
}

fn internal(_: impl std::fmt::Display) -> AuthError {
    AuthError::Internal("Account operation failed".into())
}

fn now() -> Result<u64, AuthError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(internal)
}

pub(crate) fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn matches_hash(value: &str, hash: &str) -> bool {
    bool::from(digest(value).as_bytes().ct_eq(hash.as_bytes()))
}

fn email(value: &str) -> Result<String, AuthError> {
    let value = value.trim().to_ascii_lowercase();
    let valid = value.len() <= 254
        && value.is_ascii()
        && !value
            .bytes()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if !valid {
        return Err(AuthError::BadRequest("Enter a valid email address".into()));
    }
    Ok(value)
}

fn password(value: &str) -> Result<(), AuthError> {
    if value.chars().count() < 12 || value.len() > 1024 {
        return Err(AuthError::BadRequest(
            "Use a password of at least 12 characters and at most 1024 bytes".into(),
        ));
    }
    Ok(())
}

fn hash_password(value: &str) -> Result<String, AuthError> {
    Argon2::default()
        .hash_password(value.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|v| v.to_string())
        .map_err(internal)
}

impl AccountAuth {
    pub fn new(config: AccountAuthConfig) -> anyhow::Result<Self> {
        let store =
            FileSecretStore::new_from_bytes(config.path.clone(), Some(config.encryption_key))?;
        let owner = if config.path.exists() {
            let raw = store
                .get_secret(ACCOUNT_KEY)?
                .ok_or_else(|| anyhow::anyhow!("Account store is missing its owner record"))?;
            Some(serde_json::from_str::<Owner>(&raw)?)
        } else {
            None
        };
        let mail_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            config,
            version: AtomicU64::new(owner.as_ref().map_or(0, |o| o.version)),
            owner: Mutex::new(owner),
            work: Arc::new(tokio::sync::Semaphore::new(2)),
            mail_client,
        })
    }

    // Write the encrypted record atomically so a crash cannot reopen signup or
    // leave only half of a password reset on disk. Use a separate installation
    // store so portfolio restore/replacement cannot reset the authentication owner.
    fn save(&self, owner: &Owner) -> Result<(), AuthError> {
        let parent = self
            .config
            .path
            .parent()
            .ok_or_else(|| internal("Invalid account path"))?;
        std::fs::create_dir_all(parent).map_err(internal)?;
        let tmp = tempfile::NamedTempFile::new_in(parent).map_err(internal)?;
        let store = FileSecretStore::new_from_bytes(
            tmp.path().to_path_buf(),
            Some(self.config.encryption_key),
        )
        .map_err(internal)?;
        store
            .set_secret(
                ACCOUNT_KEY,
                &serde_json::to_string(owner).map_err(internal)?,
            )
            .map_err(internal)?;
        tmp.as_file().sync_all().map_err(internal)?;
        tmp.persist(&self.config.path).map_err(internal)?;
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(internal)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_reset_password(&self, address: &str) {
        let (_, code) = self.recovery(address).unwrap().unwrap();
        self.reset(address, &code, "replacement password for testing")
            .unwrap();
    }

    pub fn version(&self) -> Result<Option<u64>, AuthError> {
        let version = self.version.load(Ordering::Acquire);
        Ok((version > 0).then_some(version))
    }

    pub fn signup(
        &self,
        address: &str,
        candidate: &str,
        invitation: &str,
    ) -> Result<(), AuthError> {
        let address = email(address)?;
        password(candidate)?;
        let mut slot = self.owner.lock().map_err(internal)?;
        if slot.is_some()
            || invitation.len() > 256
            || !matches_hash(invitation, &self.config.invitation_hash)
        {
            return Err(AuthError::BadRequest(
                "Invitation is invalid or has already been used".into(),
            ));
        }
        let owner = Owner {
            email: address,
            password_hash: hash_password(candidate)?,
            version: 1,
            recovery: None,
            last_recovery_at: 0,
        };
        self.save(&owner)?;
        self.version.store(owner.version, Ordering::Release);
        *slot = Some(owner);
        Ok(())
    }

    pub fn login(&self, address: &str, candidate: &str) -> Result<u64, AuthError> {
        if candidate.len() > 1024 {
            return Err(AuthError::InvalidCredentials);
        }
        let address = email(address).map_err(|_| AuthError::InvalidCredentials)?;
        let slot = self.owner.lock().map_err(internal)?;
        let owner = slot.clone().ok_or(AuthError::InvalidCredentials)?;
        drop(slot);
        let parsed = PasswordHash::new(&owner.password_hash).map_err(internal)?;
        // Always verify the hash, even for a different email address.
        let correct = Argon2::default()
            .verify_password(candidate.as_bytes(), &parsed)
            .is_ok();
        if !correct || address != owner.email {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(owner.version)
    }

    fn recovery(&self, address: &str) -> Result<Option<(String, String)>, AuthError> {
        let address = email(address)?;
        let mut slot = self.owner.lock().map_err(internal)?;
        let Some(mut owner) = slot.clone() else {
            return Ok(None);
        };
        let time = now()?;
        if owner.email != address || time.saturating_sub(owner.last_recovery_at) < RECOVERY_COOLDOWN
        {
            return Ok(None);
        }
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        let code: String = bytes.iter().map(|v| format!("{v:02x}")).collect();
        owner.recovery = Some(Recovery {
            hash: digest(&code),
            expires_at: time + RECOVERY_TTL,
            attempts: 0,
        });
        owner.last_recovery_at = time;
        self.save(&owner)?;
        self.version.store(owner.version, Ordering::Release);
        *slot = Some(owner);
        Ok(Some((address, code)))
    }

    pub fn reset(&self, address: &str, code: &str, candidate: &str) -> Result<(), AuthError> {
        password(candidate)?;
        let address = email(address)?;
        let mut slot = self.owner.lock().map_err(internal)?;
        let invalid = || AuthError::BadRequest("Recovery code is invalid or expired".into());
        let mut owner = slot.clone().ok_or_else(invalid)?;
        // Unrelated email addresses must not consume the owner's recovery budget.
        if address != owner.email {
            return Err(invalid());
        }
        let recovery = owner.recovery.as_mut().ok_or_else(invalid)?;
        if recovery.attempts >= MAX_ATTEMPTS || now()? >= recovery.expires_at {
            return Err(invalid());
        }
        recovery.attempts += 1;
        let valid = code.len() <= 128 && matches_hash(code, &recovery.hash);
        // Persist failed attempts too, so restarting cannot reset the limit.
        if !valid {
            self.save(&owner)?;
            self.version.store(owner.version, Ordering::Release);
            *slot = Some(owner);
            return Err(invalid());
        }
        owner.password_hash = hash_password(candidate)?;
        owner.version = owner
            .version
            .checked_add(1)
            .ok_or_else(|| internal("Account version overflow"))?;
        owner.recovery = None;
        self.save(&owner)?;
        self.version.store(owner.version, Ordering::Release);
        *slot = Some(owner);
        Ok(())
    }

    async fn send_recovery(&self, address: String, code: String) -> Result<(), AuthError> {
        let body = serde_json::json!({
            "from": self.config.email_from,
            "to": [address],
            "subject": "Your portfolio password recovery code",
            "text": format!("Your recovery code is: {code}\n\nIt expires in 10 minutes and can be used once. If you did not request this, ignore this email."),
        });
        let response = self
            .mail_client
            .post("https://api.resend.com/emails")
            .bearer_auth(&self.config.resend_api_key)
            .json(&body)
            .send()
            .await
            .map_err(internal)?;
        if !response.status().is_success() {
            return Err(internal("Recovery email delivery failed"));
        }
        Ok(())
    }
}

// Run password hashing and filesystem operations outside the async executor.
pub(crate) async fn work<T: Send + 'static>(
    accounts: Arc<AccountAuth>,
    f: impl FnOnce(&AccountAuth) -> Result<T, AuthError> + Send + 'static,
) -> Result<T, AuthError> {
    let permit = accounts
        .work
        .clone()
        .try_acquire_owned()
        .map_err(|_| AuthError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f(&accounts)
    })
    .await
    .map_err(internal)?
}

fn accounts(state: &AuthState) -> Result<Arc<AccountAuth>, AuthError> {
    state
        .auth
        .as_ref()
        .and_then(|a| a.accounts.clone())
        .ok_or(AuthError::NotConfigured)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignupRequest {
    email: String,
    password: String,
    invitation_code: String,
}
#[derive(Deserialize)]
pub(crate) struct RecoveryRequest {
    email: String,
}
#[derive(Deserialize)]
pub(crate) struct ResetRequest {
    email: String,
    code: String,
    password: String,
}

pub(crate) async fn signup(
    State(state): State<AuthState>,
    Json(body): Json<SignupRequest>,
) -> Result<StatusCode, AuthError> {
    work(accounts(&state)?, move |a| {
        a.signup(&body.email, &body.password, &body.invitation_code)
    })
    .await?;
    Ok(StatusCode::CREATED)
}

pub(crate) async fn recover(
    State(state): State<AuthState>,
    Json(body): Json<RecoveryRequest>,
) -> Result<StatusCode, AuthError> {
    let accounts = accounts(&state)?;
    let delivery = work(accounts.clone(), move |a| a.recovery(&body.email)).await?;
    if let Some((address, code)) = delivery {
        if accounts.send_recovery(address, code).await.is_err() {
            // Do not expose provider errors, recovery codes, or account existence.
            tracing::warn!("Account recovery email could not be delivered");
        }
    }
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn reset(
    State(state): State<AuthState>,
    Json(body): Json<ResetRequest>,
) -> Result<StatusCode, AuthError> {
    work(accounts(&state)?, move |a| {
        a.reset(&body.email, &body.code, &body.password)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PASSWORD: &str = "test password with enough length";
    fn config(path: PathBuf) -> AccountAuthConfig {
        AccountAuthConfig {
            path,
            encryption_key: [7; 32],
            invitation_hash: digest("test-invitation"),
            resend_api_key: "test-only".into(),
            email_from: "test@example.com".into(),
        }
    }
    fn setup() -> (tempfile::TempDir, AccountAuth) {
        let dir = tempfile::tempdir().unwrap();
        let auth = AccountAuth::new(config(dir.path().join("owner.json"))).unwrap();
        (dir, auth)
    }
    #[test]
    fn input_boundaries_reject_malformed_email_and_oversized_secrets() {
        for address in [
            "a".repeat(255),
            "é@example.com".into(),
            "a b@example.com".into(),
            "a\u{0001}@example.com".into(),
            "missing-at".into(),
            "@example.com".into(),
            "a@b@example.com".into(),
            "a@localhost".into(),
            "a@.example.com".into(),
            "a@example.com.".into(),
        ] {
            assert!(
                matches!(email(&address), Err(AuthError::BadRequest(_))),
                "{address:?}"
            );
        }
        assert_eq!(email(" Owner@Example.com ").unwrap(), "owner@example.com");
        assert!(password(&"x".repeat(12)).is_ok());
        assert!(password(&"x".repeat(1024)).is_ok());
        assert!(password(&"🔐".repeat(12)).is_ok());
        assert!(password(&"🔐".repeat(11)).is_err());
        assert!(password(&"x".repeat(1025)).is_err());
        let (_dir, auth) = setup();
        assert!(auth
            .signup("owner@example.com", PASSWORD, &"x".repeat(257))
            .is_err());
        assert_eq!(auth.version().unwrap(), None);
        assert!(auth.recovery("owner@example.com").unwrap().is_none());
        assert!(auth.login("owner@example.com", PASSWORD).is_err());
        assert!(auth.login("owner@example.com", &"x".repeat(1025)).is_err());
        assert!(auth.login("not-an-email", PASSWORD).is_err());
        assert!(auth.reset("owner@example.com", "code", PASSWORD).is_err());
    }

    #[test]
    fn failed_persistence_and_version_overflow_preserve_auth_state() {
        let (dir, mut auth) = setup();
        // An existing encrypted store without an owner must fail closed.
        FileSecretStore::new_from_bytes(dir.path().join("missing.json"), Some([7; 32]))
            .unwrap()
            .set_secret("unrelated", "value")
            .unwrap();
        assert!(AccountAuth::new(config(dir.path().join("missing.json"))).is_err());
        auth.signup("owner@example.com", PASSWORD, "test-invitation")
            .unwrap();
        assert!(auth
            .reset("owner@example.com", "no-recovery", PASSWORD)
            .is_err());
        let (_, code) = auth.recovery("owner@example.com").unwrap().unwrap();
        assert!(auth
            .reset("owner@example.com", &"x".repeat(129), PASSWORD)
            .is_err());
        assert_eq!(
            auth.owner
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .recovery
                .as_ref()
                .unwrap()
                .attempts,
            1
        );
        {
            let mut slot = auth.owner.lock().unwrap();
            slot.as_mut().unwrap().version = u64::MAX;
        }
        assert!(matches!(
            auth.reset("owner@example.com", &code, PASSWORD),
            Err(AuthError::Internal(_))
        ));
        assert_eq!(auth.version().unwrap(), Some(1));
        auth.owner.lock().unwrap().as_mut().unwrap().version = 1;
        let original = auth.config.path.clone();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "file, not a directory").unwrap();
        auth.config.path = blocker.join("owner.json");
        assert!(matches!(
            auth.reset("owner@example.com", &code, "replacement password for test"),
            Err(AuthError::Internal(_))
        ));
        assert_eq!(auth.login("owner@example.com", PASSWORD).unwrap(), 1);
        auth.config.path = PathBuf::from("/");
        assert!(matches!(
            auth.reset("owner@example.com", &code, PASSWORD),
            Err(AuthError::Internal(_))
        ));
        auth.config.path = original;
        auth.reset("owner@example.com", &code, "replacement password for test")
            .unwrap();
        assert_eq!(auth.version().unwrap(), Some(2));
    }

    #[tokio::test]
    async fn password_work_is_bounded_and_permits_return_after_failure() {
        let (_dir, auth) = setup();
        let auth = Arc::new(auth);
        let permits = auth.work.acquire_many(2).await.unwrap();
        assert!(matches!(
            work(auth.clone(), |_| Ok(())).await,
            Err(AuthError::Busy)
        ));
        drop(permits);
        assert!(matches!(
            work(auth.clone(), |_| Err::<(), _>(
                AuthError::InvalidCredentials
            ))
            .await,
            Err(AuthError::InvalidCredentials)
        ));
        assert_eq!(work(auth, |_| Ok(42)).await.unwrap(), 42);
    }

    // DNS is overridden only on this test client. The production HTTPS URL,
    // certificate checks, request body, and provider-status handling are exercised.
    pub(super) async fn mail_server(
        status: Option<u16>,
    ) -> (reqwest::Client, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_rustls::{rustls, TlsAcceptor};
        let cert = rcgen::generate_simple_self_signed(vec!["api.resend.com".into()]).unwrap();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .resolve("api.resend.com", address)
            .add_root_certificate(reqwest::Certificate::from_der(cert.cert.der()).unwrap())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = TlsAcceptor::from(Arc::new(tls))
                .accept(socket)
                .await
                .unwrap();
            let mut data = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client closed before sending the recovery email");
                data.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&data);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            if let Some(status) = status {
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            String::from_utf8(data).unwrap()
        });
        (client, server)
    }

    #[tokio::test]
    async fn recovery_delivery_checks_payload_status_and_transport_failure() {
        for status in [Some(200), Some(500), Some(302), None] {
            let (_dir, mut auth) = setup();
            let (client, server) = mail_server(status).await;
            auth.mail_client = client;
            let result = auth
                .send_recovery("owner@example.com".into(), "test-recovery-code".into())
                .await;
            assert_eq!(result.is_ok(), status == Some(200));
            if let Err(error) = result {
                assert!(
                    matches!(error, AuthError::Internal(ref message) if message == "Account operation failed")
                );
            }
            let request = tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            let (headers, body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with("POST /emails HTTP/1.1"));
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer test-only"));
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert_eq!(body["from"], "test@example.com");
            assert_eq!(body["to"], serde_json::json!(["owner@example.com"]));
            assert!(body["text"]
                .as_str()
                .unwrap()
                .contains("test-recovery-code"));
            assert!(body["text"].as_str().unwrap().contains("10 minutes"));
        }
    }

    #[test]
    fn account_environment_validation_is_opt_in_and_complete() {
        let run = |overrides: &[(&str, &str)], removed: Option<&str>, expected: &str| {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "account_auth::tests::account_environment_worker",
                    "--nocapture",
                ])
                .env_clear()
                .envs(
                    std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)),
                )
                .env("WF_ACCOUNT_TEST_EXPECT", expected)
                .env("WF_ACCOUNT_AUTH", "true")
                .env(
                    "WF_SIGNUP_INVITE_HASH",
                    digest("invitation").to_ascii_uppercase(),
                )
                .env("WF_RESEND_API_KEY", "test-only")
                .env("WF_AUTH_EMAIL_FROM", "test@example.com")
                .envs(overrides.iter().copied());
            if let Some(key) = removed {
                command.env_remove(key);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&[], None, "enabled");
        for flag in [" 1 ", "YeS"] {
            run(&[("WF_ACCOUNT_AUTH", flag)], None, "enabled");
        }
        for flag in ["false", "0", "NO"] {
            run(
                &[("WF_ACCOUNT_AUTH", flag)],
                Some("WF_RESEND_API_KEY"),
                "disabled",
            );
        }
        run(&[], Some("WF_ACCOUNT_AUTH"), "disabled");
        run(
            &[("WF_ACCOUNT_AUTH", "perhaps")],
            None,
            "must be true or false",
        );
        run(&[("WF_SIGNUP_INVITE_HASH", "short")], None, "SHA-256");
        run(
            &[("WF_SIGNUP_INVITE_HASH", &"z".repeat(64))],
            None,
            "SHA-256",
        );
        run(&[("WF_RESEND_API_KEY", "  ")], None, "key and sender");
        run(&[("WF_AUTH_EMAIL_FROM", "  ")], None, "key and sender");
        for key in [
            "WF_SIGNUP_INVITE_HASH",
            "WF_RESEND_API_KEY",
            "WF_AUTH_EMAIL_FROM",
        ] {
            run(&[], Some(key), "not found");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "account_auth::tests::account_environment_worker"])
                .env_clear()
                .envs(
                    std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)),
                )
                .env("WF_ACCOUNT_AUTH", std::ffi::OsString::from_vec(vec![0xff]))
                .env("WF_ACCOUNT_TEST_EXPECT", "valid UTF-8")
                .output()
                .unwrap();
            assert!(output.status.success());
        }
    }

    #[test]
    fn account_environment_worker() {
        let Ok(expected) = std::env::var("WF_ACCOUNT_TEST_EXPECT") else {
            return;
        };
        let result = AccountAuthConfig::from_env(PathBuf::from("unused-account.json"), [7; 32]);
        match expected.as_str() {
            "enabled" => {
                let loaded = result.unwrap().unwrap();
                assert_eq!(loaded.invitation_hash, digest("invitation"));
                assert_eq!(loaded.path, PathBuf::from("unused-account.json"));
            }
            "disabled" => assert!(result.unwrap().is_none()),
            message => assert!(result.err().unwrap().to_string().contains(message)),
        }
    }

    #[test]
    fn invitation_is_single_use_and_owner_survives_restart() {
        let (dir, auth) = setup();
        assert!(auth.signup("owner@example.com", PASSWORD, "wrong").is_err());
        assert!(auth
            .signup("bad-email", PASSWORD, "test-invitation")
            .is_err());
        assert!(auth
            .signup("owner@example.com", "short", "test-invitation")
            .is_err());
        auth.signup(" Owner@Example.com ", PASSWORD, "test-invitation")
            .unwrap();
        assert_eq!(auth.login("OWNER@example.com", PASSWORD).unwrap(), 1);
        assert!(auth.login("other@example.com", PASSWORD).is_err());
        assert!(auth.login("owner@example.com", "wrong").is_err());
        let restarted = AccountAuth::new(config(dir.path().join("owner.json"))).unwrap();
        assert!(restarted
            .signup("other@example.com", PASSWORD, "test-invitation")
            .is_err());
        assert!(restarted.login("owner@example.com", PASSWORD).is_ok());
        let raw = std::fs::read_to_string(dir.path().join("owner.json")).unwrap();
        assert!(!raw.contains("owner@example.com"));
        assert!(!raw.contains("argon2"));
        let mut wrong_key = config(dir.path().join("owner.json"));
        wrong_key.encryption_key = [8; 32];
        assert!(AccountAuth::new(wrong_key).is_err());
    }
    #[test]
    fn recovery_is_single_use_and_changes_session_version() {
        let (_dir, auth) = setup();
        auth.signup("owner@example.com", PASSWORD, "test-invitation")
            .unwrap();
        assert!(auth.recovery("unknown@example.com").unwrap().is_none());
        let (_, code) = auth.recovery("owner@example.com").unwrap().unwrap();
        assert!(auth.recovery("owner@example.com").unwrap().is_none());
        assert!(auth.reset("other@example.com", &code, PASSWORD).is_err());
        auth.reset("owner@example.com", &code, "replacement password for test")
            .unwrap();
        assert!(auth.login("owner@example.com", PASSWORD).is_err());
        assert_eq!(
            auth.login("owner@example.com", "replacement password for test")
                .unwrap(),
            2
        );
        assert!(auth.reset("owner@example.com", &code, PASSWORD).is_err());
    }
    #[test]
    fn wrong_email_resets_do_not_consume_owner_recovery_attempts() {
        let (dir, auth) = setup();
        auth.signup("owner@example.com", PASSWORD, "test-invitation")
            .unwrap();
        let (_, code) = auth.recovery("owner@example.com").unwrap().unwrap();
        let path = dir.path().join("owner.json");
        let before = std::fs::read(&path).unwrap();
        for _ in 0..=MAX_ATTEMPTS {
            for candidate_code in ["bad-code", code.as_str()] {
                assert!(matches!(
                    auth.reset("other@example.com", candidate_code, PASSWORD),
                    Err(AuthError::BadRequest(_))
                ));
            }
        }
        assert!(std::fs::read(&path).unwrap() == before);
        assert_eq!(
            auth.owner
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .recovery
                .as_ref()
                .unwrap()
                .attempts,
            0
        );
        let restarted = AccountAuth::new(config(path)).unwrap();
        restarted
            .reset(
                " OWNER@Example.com ",
                &code,
                "replacement password for test",
            )
            .unwrap();
        assert_eq!(restarted.version().unwrap(), Some(2));
    }

    #[test]
    fn expired_and_exhausted_codes_are_rejected_across_restart() {
        let (dir, auth) = setup();
        auth.signup("owner@example.com", PASSWORD, "test-invitation")
            .unwrap();
        let (_, code) = auth.recovery("owner@example.com").unwrap().unwrap();
        for _ in 0..MAX_ATTEMPTS {
            assert!(auth
                .reset("owner@example.com", "bad-code", PASSWORD)
                .is_err());
        }
        let restarted = AccountAuth::new(config(dir.path().join("owner.json"))).unwrap();
        assert!(restarted
            .reset("owner@example.com", &code, PASSWORD)
            .is_err());
        {
            let mut slot = restarted.owner.lock().unwrap();
            let owner = slot.as_mut().unwrap();
            owner.recovery.as_mut().unwrap().attempts = 0;
            owner.recovery.as_mut().unwrap().expires_at = now().unwrap();
        }
        assert!(restarted
            .reset("owner@example.com", &code, PASSWORD)
            .is_err());
    }
    #[test]
    fn concurrent_signups_have_exactly_one_winner() {
        let (_dir, auth) = setup();
        let auth = Arc::new(auth);
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let auth = auth.clone();
                std::thread::spawn(move || {
                    auth.signup(
                        &format!("owner{i}@example.com"),
                        PASSWORD,
                        "test-invitation",
                    )
                    .is_ok()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| usize::from(h.join().unwrap()))
                .sum::<usize>(),
            1
        );
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use crate::auth::{AuthConfig, AuthManager, CookieSecurePolicy};
    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::{
            header::{COOKIE, SET_COOKIE},
            Request,
        },
    };
    use tower::ServiceExt;

    fn setup() -> (tempfile::TempDir, axum::Router, Arc<AccountAuth>) {
        setup_with_mail_client(None)
    }

    fn setup_with_mail_client(
        client: Option<reqwest::Client>,
    ) -> (tempfile::TempDir, axum::Router, Arc<AccountAuth>) {
        let dir = tempfile::tempdir().unwrap();
        let mut manager = Arc::new(
            AuthManager::new(&AuthConfig {
                password_hash: None,
                accounts: Some(AccountAuthConfig {
                    path: dir.path().join("owner.json"),
                    encryption_key: [9; 32],
                    invitation_hash: digest("invitation"),
                    resend_api_key: "test".into(),
                    email_from: "test@example.com".into(),
                }),
                jwt_secret: vec![6; 32],
                access_token_ttl: Duration::from_secs(3600),
                cookie_secure: CookieSecurePolicy::Always,
            })
            .unwrap(),
        );
        if let Some(client) = client {
            Arc::get_mut(
                Arc::get_mut(&mut manager)
                    .unwrap()
                    .accounts
                    .as_mut()
                    .unwrap(),
            )
            .unwrap()
            .mail_client = client;
        }
        let accounts = manager.accounts.clone().unwrap();
        let router = crate::auth::router(AuthState {
            auth: Some(manager),
            oidc: None,
        });
        (dir, router, accounts)
    }
    async fn request(
        router: &axum::Router,
        path: &str,
        body: Option<serde_json::Value>,
        cookie: Option<&str>,
        peer: u8,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .uri(path)
            .method(if body.is_some() { "POST" } else { "GET" })
            .extension(ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, peer],
                12345,
            ))));
        if let Some(cookie) = cookie {
            builder = builder.header(COOKIE, cookie);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        router
            .clone()
            .oneshot(
                builder
                    .body(
                        body.map(|v| Body::from(v.to_string()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    #[tokio::test]
    async fn recovery_http_hides_delivery_failure_and_retains_valid_code() {
        for status in [Some(200), Some(500)] {
            let (client, server) = super::tests::mail_server(status).await;
            let (_dir, router, accounts) = setup_with_mail_client(Some(client));
            accounts
                .signup("owner@example.com", "original test password", "invitation")
                .unwrap();
            let response = request(
                &router,
                "/auth/recover",
                Some(serde_json::json!({"email":"owner@example.com"})),
                None,
                1,
            )
            .await;
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            assert!(to_bytes(response.into_body(), 4096)
                .await
                .unwrap()
                .is_empty());
            let request = tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap();
            let (_, body) = request.split_once("\r\n\r\n").unwrap();
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            let text = body["text"].as_str().unwrap();
            let code = text
                .strip_prefix("Your recovery code is: ")
                .unwrap()
                .split('\n')
                .next()
                .unwrap();
            accounts
                .reset("owner@example.com", code, "replacement test password")
                .unwrap();
            assert_eq!(
                accounts
                    .login("owner@example.com", "replacement test password")
                    .unwrap(),
                2
            );
        }
    }

    #[tokio::test]
    async fn signup_login_reset_and_relogin_through_http() {
        let (_dir, router, accounts) = setup();
        assert_eq!(
            request(&router, "/auth/me", None, None, 1).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let response = request(&router, "/auth/status", None, None, 1).await;
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["accountAuth"], true);
        assert_eq!(body["requiresPassword"], true);
        let signup = serde_json::json!({"email": "owner@example.com", "password": "original test password", "invitationCode": "invitation"});
        assert_eq!(
            request(&router, "/auth/signup", Some(signup.clone()), None, 1)
                .await
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            request(&router, "/auth/signup", Some(signup), None, 1)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let login =
            serde_json::json!({"email": "owner@example.com", "password": "original test password"});
        let response = request(&router, "/auth/login", Some(login.clone()), None, 1).await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[SET_COOKIE].to_str().unwrap().to_owned();
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("Secure")
                && cookie.contains("SameSite=Lax")
        );
        assert_eq!(
            request(&router, "/auth/me", None, Some(&cookie), 1)
                .await
                .status(),
            StatusCode::OK
        );
        let (_, code) = accounts.recovery("owner@example.com").unwrap().unwrap();
        let reset = serde_json::json!({"email":"owner@example.com", "code":code, "password":"replacement test password"});
        assert_eq!(
            request(&router, "/auth/reset", Some(reset.clone()), None, 2)
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            request(&router, "/auth/me", None, Some(&cookie), 2)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&router, "/auth/login", Some(login), None, 2)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&router, "/auth/reset", Some(reset), None, 2)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(request(&router, "/auth/login", Some(serde_json::json!({"email": "owner@example.com", "password": "replacement test password"})), None, 3).await.status(), StatusCode::OK);
        assert_eq!(
            request(
                &router,
                "/auth/recover",
                Some(serde_json::json!({"email": "unknown@example.com"})),
                None,
                3
            )
            .await
            .status(),
            StatusCode::ACCEPTED
        );
    }
    #[tokio::test]
    async fn exhausted_password_workers_reject_mutations_without_writing() {
        let (dir, router, accounts) = setup();
        let _permits = accounts.work.acquire_many(2).await.unwrap();
        for (peer, path, body) in [
            (
                1,
                "/auth/signup",
                serde_json::json!({"email":"owner@example.com", "password":"original test password", "invitationCode":"invitation"}),
            ),
            (
                2,
                "/auth/recover",
                serde_json::json!({"email":"owner@example.com"}),
            ),
            (
                3,
                "/auth/reset",
                serde_json::json!({"email":"owner@example.com", "password":"replacement test password", "code":"code"}),
            ),
        ] {
            assert_eq!(
                request(&router, path, Some(body), None, peer)
                    .await
                    .status(),
                StatusCode::TOO_MANY_REQUESTS
            );
        }
        assert!(!dir.path().join("owner.json").exists());
        assert_eq!(accounts.version().unwrap(), None);
    }

    #[tokio::test]
    async fn endpoints_share_throttle_and_limit_body_size() {
        let (_dir, router, _) = setup();
        for _ in 0..5 {
            assert_eq!(
                request(
                    &router,
                    "/auth/login",
                    Some(serde_json::json!({"password":"bad"})),
                    None,
                    1
                )
                .await
                .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(request(&router, "/auth/signup", Some(serde_json::json!({"email":"owner@example.com", "password":"valid long password", "invitationCode":"invitation"})), None, 1).await.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            request(
                &router,
                "/auth/recover",
                Some(serde_json::json!({"email":"x".repeat(5000)})),
                None,
                2
            )
            .await
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
