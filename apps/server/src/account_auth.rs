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
        let recovery = owner.recovery.as_mut().ok_or_else(invalid)?;
        if recovery.attempts >= MAX_ATTEMPTS || now()? >= recovery.expires_at {
            return Err(invalid());
        }
        recovery.attempts += 1;
        let valid =
            code.len() <= 128 && matches_hash(code, &recovery.hash) && address == owner.email;
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
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
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
