use super::*;
use crate::{delivery::FamilyDelivery, sources::FamilyFactSource};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};
use wealthfolio_core::{secrets::SecretStore, Error};

#[derive(Default)]
struct Repository {
    state: Mutex<FamilyState>,
    reject: AtomicBool,
}
#[async_trait]
impl FamilyRepository for Repository {
    async fn load(&self) -> Result<FamilyState> {
        Ok(self.state.lock().unwrap().clone())
    }
    async fn save(&self, state: FamilyState) -> Result<()> {
        if self.reject.load(Ordering::SeqCst) {
            return Err(Error::Repository("Synthetic write failure".into()));
        }
        *self.state.lock().unwrap() = state;
        Ok(())
    }
}
#[derive(Default)]
struct Secrets(Mutex<HashMap<String, String>>);
impl SecretStore for Secrets {
    fn set_secret(&self, k: &str, v: &str) -> Result<()> {
        self.0.lock().unwrap().insert(k.into(), v.into());
        Ok(())
    }
    fn get_secret(&self, k: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(k).cloned())
    }
    fn delete_secret(&self, k: &str) -> Result<()> {
        self.0.lock().unwrap().remove(k);
        Ok(())
    }
}
struct Facts {
    calls: AtomicUsize,
    fail: AtomicBool,
}
#[async_trait]
impl FamilyFactSource for Facts {
    async fn evaluate(&self, _: &FamilySettings, _: DateTime<Utc>) -> Result<Evaluation> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Ok(Evaluation {
                warnings: vec!["Synthetic source unavailable".into()],
                ..Default::default()
            });
        }
        Ok(Evaluation {
            candidates: vec![Candidate {
                key: "large:one".into(),
                source: "charges".into(),
                kind: "large_charge".into(),
                severity: "warning".into(),
                title: "Synthetic charge".into(),
                detail: "Synthetic merchant".into(),
                href: "/family".into(),
                amount: Some(1500.0),
                currency: Some("USD".into()),
                notify: true,
            }],
            complete_sources: vec!["charges".into()],
            warnings: vec![],
        })
    }
    async fn digest(
        &self,
        month: &str,
        _: &FamilySettings,
        now: DateTime<Utc>,
    ) -> Result<MonthlyDigest> {
        Ok(MonthlyDigest {
            month: month.into(),
            generated_at: now,
            currency: "USD".into(),
            net_worth: None,
            net_worth_change: None,
            income: None,
            spending: None,
            savings_rate: None,
            movers: vec![],
            goals: vec![],
            upcoming_bills: vec![],
            warnings: vec!["Incomplete synthetic data".into()],
            text: "No fabricated amounts".into(),
        })
    }
    fn timezone(&self) -> Result<String> {
        Ok("America/Los_Angeles".into())
    }
    fn validate_account(&self, id: &str) -> Result<()> {
        if id == "account" {
            Ok(())
        } else {
            Err(engine::invalid("Account not found"))
        }
    }
}
struct Delivery {
    calls: AtomicUsize,
    fail: AtomicBool,
    poison: AtomicBool,
    pause: AtomicBool,
    started: tokio::sync::Notify,
    finish: tokio::sync::Notify,
    repo: Arc<Repository>,
}
#[async_trait]
impl FamilyDelivery for Delivery {
    fn email_configured(&self, _: &dyn SecretStore) -> bool {
        true
    }
    async fn send(
        &self,
        _: &DeliveryRecord,
        _: &FamilySettings,
        _: &dyn SecretStore,
    ) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.finish.notified().await;
        }
        if self.poison.load(Ordering::SeqCst) {
            self.repo.reject.store(true, Ordering::SeqCst);
        }
        if self.fail.load(Ordering::SeqCst) {
            Err(engine::invalid("Synthetic delivery failure"))
        } else {
            Ok(())
        }
    }
}
fn setup() -> (
    FamilyService,
    Arc<Repository>,
    Arc<Facts>,
    Arc<Delivery>,
    Arc<Secrets>,
) {
    let repo = Arc::new(Repository::default());
    let facts = Arc::new(Facts {
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    let delivery = Arc::new(Delivery {
        calls: AtomicUsize::new(0),
        fail: AtomicBool::new(false),
        poison: AtomicBool::new(false),
        pause: AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
        finish: tokio::sync::Notify::new(),
        repo: repo.clone(),
    });
    let secrets = Arc::new(Secrets::default());
    (
        FamilyService::new(
            repo.clone(),
            facts.clone(),
            secrets.clone(),
            delivery.clone(),
        ),
        repo,
        facts,
        delivery,
        secrets,
    )
}
fn now() -> DateTime<Utc> {
    "2026-05-15T17:00:00Z".parse().unwrap()
}
fn settings() -> SaveFamilySettings {
    SaveFamilySettings {
        settings: FamilySettings {
            email_alerts: true,
            email_from: "family@example.test".into(),
            recipients: vec!["partner@example.test".into()],
            ..Default::default()
        },
        resend_api_key: None,
        webhook_token: None,
        webhook_url: None,
    }
}

#[tokio::test]
async fn external_delivery_is_opt_in_and_refresh_itself_never_sends() {
    let (s, _, _, d, _) = setup();
    s.refresh(now()).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
    assert!(s.snapshot().await.unwrap().state.deliveries.is_empty());
}
#[tokio::test]
async fn concurrent_refreshes_and_scheduler_ticks_deliver_once() {
    let (s, _, _, d, _) = setup();
    s.save_settings(settings()).await.unwrap();
    let (a, b) = tokio::join!(s.refresh(now()), s.refresh(now()));
    a.unwrap();
    b.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
    s.dispatch(now()).await.unwrap();
    s.refresh(now()).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    assert_eq!(s.snapshot().await.unwrap().state.deliveries.len(), 1);
}
#[tokio::test]
async fn disabling_the_feature_stops_scans_and_sends() {
    let (s, _, f, d, _) = setup();
    let mut input = settings();
    input.settings.enabled = false;
    s.save_settings(input).await.unwrap();
    s.refresh(now()).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn changed_recipients_cancel_pending_messages_instead_of_retargeting() {
    let (s, _, _, d, _) = setup();
    s.save_settings(settings()).await.unwrap();
    s.refresh(now()).await.unwrap();
    let mut input = settings();
    input.settings.recipients = vec!["new@example.test".into()];
    s.save_settings(input).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        s.snapshot().await.unwrap().state.deliveries[0].status,
        DeliveryStatus::Cancelled
    );
}
#[tokio::test]
async fn failed_source_preserves_read_state_and_notes() {
    let (s, _, f, _, _) = setup();
    let state = s.refresh(now()).await.unwrap();
    let id = state.state.items[0].id.clone();
    s.action(
        InboxAction {
            id,
            action: "acknowledge".into(),
            note: Some("Discuss at dinner".into()),
            snoozed_until: None,
        },
        now(),
    )
    .await
    .unwrap();
    f.fail.store(true, Ordering::SeqCst);
    let snapshot = s.refresh(now()).await.unwrap();
    assert!(snapshot.state.items[0].active);
    assert_eq!(snapshot.state.items[0].status, InboxStatus::Acknowledged);
    assert_eq!(snapshot.state.items[0].note, "Discuss at dinner");
    assert_eq!(snapshot.state.warnings.len(), 1);
}
#[tokio::test]
async fn failed_claim_write_prevents_any_network_delivery() {
    let (s, r, _, d, _) = setup();
    s.save_settings(settings()).await.unwrap();
    s.refresh(now()).await.unwrap();
    r.reject.store(true, Ordering::SeqCst);
    assert!(s.dispatch(now()).await.is_err());
    assert_eq!(d.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn interrupted_completion_is_not_automatically_sent_again() {
    let (s, r, _, d, _) = setup();
    s.save_settings(settings()).await.unwrap();
    s.refresh(now()).await.unwrap();
    d.poison.store(true, Ordering::SeqCst);
    assert!(s.dispatch(now()).await.is_err());
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    r.reject.store(false, Ordering::SeqCst);
    d.poison.store(false, Ordering::SeqCst);
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.snapshot().await.unwrap().state.deliveries[0].status,
        DeliveryStatus::Sending
    );
}
#[tokio::test]
async fn delivery_failures_are_visible_and_retry_is_explicit() {
    let (s, _, _, d, _) = setup();
    s.save_settings(settings()).await.unwrap();
    s.refresh(now()).await.unwrap();
    d.fail.store(true, Ordering::SeqCst);
    s.dispatch(now()).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    let record = s.snapshot().await.unwrap().state.deliveries[0].clone();
    assert_eq!(record.status, DeliveryStatus::Failed);
    d.fail.store(false, Ordering::SeqCst);
    s.retry_delivery(&record.id, now()).await.unwrap();
    s.dispatch(now()).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn monthly_digest_is_queued_once_per_profile_calendar_month() {
    let (s, _, _, d, _) = setup();
    let mut input = settings();
    input.settings.email_alerts = false;
    input.settings.email_digest = true;
    s.save_settings(input).await.unwrap();
    let before = "2026-04-01T15:59:00Z".parse().unwrap();
    s.refresh(before).await.unwrap();
    assert!(s.snapshot().await.unwrap().state.digests.is_empty());
    let after = "2026-04-01T16:00:00Z".parse().unwrap();
    s.refresh(after).await.unwrap();
    s.refresh(after).await.unwrap();
    s.dispatch(after).await.unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    assert!(s
        .snapshot()
        .await
        .unwrap()
        .state
        .digests
        .contains_key("2026-03"));
}
#[tokio::test]
async fn snapshot_never_returns_delivery_credentials_or_full_webhook_urls() {
    let (s, _, _, _, secrets) = setup();
    let mut input = settings();
    input.resend_api_key = Some("synthetic-private-key".into());
    input.webhook_token = Some("synthetic-private-token".into());
    input.webhook_url = Some("https://example.com/private-hook-token".into());
    let snapshot = s.save_settings(input).await.unwrap();
    let json = serde_json::to_string(&snapshot).unwrap();
    for secret in [
        "synthetic-private-key",
        "synthetic-private-token",
        "private-hook-token",
    ] {
        assert!(!json.contains(secret));
    }
    assert_eq!(
        secrets.get_secret(WEBHOOK_URL_KEY).unwrap().unwrap(),
        "https://example.com/private-hook-token"
    );
}
#[tokio::test]
async fn manual_statement_reminder_does_not_clear_source_warnings_or_edit_accounts() {
    let (s, _, f, _, _) = setup();
    f.fail.store(true, Ordering::SeqCst);
    let old = s.refresh(now()).await.unwrap();
    let new = s
        .task(
            FamilyTask {
                account_id: "account".into(),
                title: "Review statements".into(),
                detail: "Select original files to resume".into(),
                resolved: false,
            },
            now() + Duration::hours(1),
        )
        .await
        .unwrap();
    assert_eq!(new.state.warnings, old.state.warnings);
    assert_eq!(new.state.last_evaluated, old.state.last_evaluated);
    assert!(s
        .task(
            FamilyTask {
                account_id: "other-profile-account".into(),
                title: "Review".into(),
                detail: String::new(),
                resolved: false
            },
            now()
        )
        .await
        .is_err());
}

#[tokio::test]
async fn disabling_delivery_takes_effect_between_messages_in_a_backlog() {
    use std::future::Future;
    let (s, _, _, d, _) = setup();
    let s = Arc::new(s);
    let mut input = settings();
    input.settings.email_digest = true;
    s.save_settings(input).await.unwrap();
    s.refresh(now()).await.unwrap();
    assert_eq!(s.snapshot().await.unwrap().state.deliveries.len(), 2);
    d.pause.store(true, Ordering::SeqCst);
    let sending_service = s.clone();
    let sending = tokio::spawn(async move { sending_service.dispatch(now()).await });
    d.started.notified().await;
    let mut disabled = settings();
    disabled.settings.enabled = false;
    let mut changing = Box::pin(s.save_settings(disabled));
    // Register a waiting settings change before the first request completes.
    let poll = std::future::poll_fn(|cx| std::task::Poll::Ready(changing.as_mut().poll(cx))).await;
    assert!(poll.is_pending());
    d.finish.notify_one();
    let (changed, sent) = tokio::join!(changing, sending);
    changed.unwrap();
    sent.unwrap().unwrap();
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
    assert!(s
        .snapshot()
        .await
        .unwrap()
        .state
        .deliveries
        .iter()
        .any(|r| r.status == DeliveryStatus::Cancelled));
}

#[tokio::test]
async fn invalid_channel_enablement_does_not_mutate_proposed_credentials() {
    let (mut service, repo, _, _, secrets) = setup();
    service.delivery = Arc::new(crate::delivery::HttpFamilyDelivery::new(None, None));
    secrets.set_secret(RESEND_KEY, "synthetic-old-key").unwrap();
    secrets
        .set_secret(WEBHOOK_TOKEN_KEY, "synthetic-old-token")
        .unwrap();
    let before = secrets.0.lock().unwrap().clone();
    let mut input = settings();
    input.resend_api_key = Some("synthetic-new-key".into());
    input.webhook_token = Some("synthetic-new-token".into());
    input.settings.webhook_alerts = true; // Neither a saved nor proposed URL exists.
    assert!(service.save_settings(input).await.is_err());
    assert_eq!(*secrets.0.lock().unwrap(), before);
    let state = repo.load().await.unwrap();
    assert_eq!(state.config_revision, 0);
    assert!(!state.settings.email_alerts && !state.settings.webhook_alerts);
}

#[tokio::test]
async fn clearing_a_required_key_is_validated_before_other_credentials_are_saved() {
    let (mut service, repo, _, _, secrets) = setup();
    service.delivery = Arc::new(crate::delivery::HttpFamilyDelivery::new(None, None));
    secrets.set_secret(RESEND_KEY, "synthetic-old-key").unwrap();
    secrets
        .set_secret(WEBHOOK_URL_KEY, "https://example.com/old-hook")
        .unwrap();
    let before = secrets.0.lock().unwrap().clone();
    let mut input = settings();
    input.resend_api_key = Some(String::new());
    input.webhook_url = Some("https://example.com/proposed-hook".into());
    input.webhook_token = Some("synthetic-new-token".into());
    assert!(service.save_settings(input).await.is_err());
    assert_eq!(*secrets.0.lock().unwrap(), before);
    assert_eq!(repo.load().await.unwrap().config_revision, 0);
}

#[tokio::test]
async fn valid_proposed_credentials_and_server_fallback_are_accepted_before_writes() {
    let (mut service, _, _, _, secrets) = setup();
    service.delivery = Arc::new(crate::delivery::HttpFamilyDelivery::new(None, None));
    let mut input = settings();
    input.resend_api_key = Some("synthetic-new-key".into());
    input.webhook_url = Some("https://example.com/proposed-hook".into());
    input.settings.webhook_alerts = true;
    let snapshot = service.save_settings(input).await.unwrap();
    assert!(snapshot.resend_configured && snapshot.webhook_configured);
    assert_eq!(snapshot.state.config_revision, 1);
    assert_eq!(
        secrets.get_secret(RESEND_KEY).unwrap().as_deref(),
        Some("synthetic-new-key")
    );
    service.delivery = Arc::new(crate::delivery::HttpFamilyDelivery::new(
        Some("synthetic-server-key".into()),
        None,
    ));
    let mut input = settings();
    input.resend_api_key = Some(String::new());
    input.settings.webhook_alerts = true; // Reuse the saved webhook URL.
    let snapshot = service.save_settings(input).await.unwrap();
    assert!(snapshot.resend_configured && snapshot.webhook_configured);
    assert_eq!(secrets.get_secret(RESEND_KEY).unwrap().as_deref(), Some(""));
    assert_eq!(snapshot.state.config_revision, 2);
}

#[tokio::test(start_paused = true)]
async fn runtime_owned_worker_preserves_timing_and_stops_when_aborted() {
    let (service, _, facts, delivery, _) = setup();
    let worker = tokio::spawn(crate::run_worker(Arc::new(service)));
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    tokio::task::yield_now().await;
    assert_eq!(facts.calls.load(Ordering::SeqCst), 0);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(facts.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(std::time::Duration::from_secs(300)).await;
    tokio::task::yield_now().await;
    assert_eq!(facts.calls.load(Ordering::SeqCst), 2);
    assert_eq!(delivery.calls.load(Ordering::SeqCst), 0); // Outbound channels remain opt-in.
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    tokio::time::advance(std::time::Duration::from_secs(600)).await;
    assert_eq!(facts.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn worker_future_can_be_created_by_a_synchronous_runtime_owner() {
    let (service, _, facts, _, _) = setup();
    let future = crate::run_worker(Arc::new(service));
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&future);
    drop(future);
    assert_eq!(facts.calls.load(Ordering::SeqCst), 0);
}
