use crate::{delivery::FamilyDelivery, engine, model::*, sources::FamilyFactSource};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use wealthfolio_core::{secrets::SecretStore, Result};

#[async_trait]
pub trait FamilyRepository: Send + Sync {
    async fn load(&self) -> Result<FamilyState>;
    async fn save(&self, state: FamilyState) -> Result<()>;
}

/// Read the effective post-update credentials without mutating the real store.
/// This also lets the existing delivery implementation evaluate its server fallback.
struct ProposedDeliverySecrets<'a> {
    current: &'a dyn SecretStore,
    changes: [(&'static str, Option<&'a str>); 3],
}
impl SecretStore for ProposedDeliverySecrets<'_> {
    fn get_secret(&self, key: &str) -> Result<Option<String>> {
        if let Some((_, Some(value))) = self.changes.iter().find(|(changed, _)| *changed == key) {
            return Ok(Some((*value).to_owned()));
        }
        self.current.get_secret(key)
    }
    fn set_secret(&self, _: &str, _: &str) -> Result<()> {
        Err(engine::invalid("Credential preview is read-only"))
    }
    fn delete_secret(&self, _: &str) -> Result<()> {
        Err(engine::invalid("Credential preview is read-only"))
    }
}

pub struct FamilyService {
    repository: Arc<dyn FamilyRepository>,
    sources: Arc<dyn FamilyFactSource>,
    secrets: Arc<dyn SecretStore>,
    delivery: Arc<dyn FamilyDelivery>,
    // One service per profile runtime, matching the existing database-owner lifecycle.
    gate: tokio::sync::Mutex<()>,
}
impl FamilyService {
    pub fn new(
        repository: Arc<dyn FamilyRepository>,
        sources: Arc<dyn FamilyFactSource>,
        secrets: Arc<dyn SecretStore>,
        delivery: Arc<dyn FamilyDelivery>,
    ) -> Self {
        Self {
            repository,
            sources,
            secrets,
            delivery,
            gate: tokio::sync::Mutex::new(()),
        }
    }
    fn snapshot_of(&self, mut state: FamilyState) -> FamilySnapshot {
        if state.settings.email_from.is_empty() {
            state.settings.email_from = self.delivery.default_sender().unwrap_or_default();
        }
        FamilySnapshot {
            state,
            resend_configured: self.delivery.email_configured(self.secrets.as_ref()),
            webhook_configured: self
                .secrets
                .get_secret(WEBHOOK_URL_KEY)
                .ok()
                .flatten()
                .is_some_and(|s| !s.is_empty()),
            webhook_token_configured: self
                .secrets
                .get_secret(WEBHOOK_TOKEN_KEY)
                .ok()
                .flatten()
                .is_some_and(|s| !s.is_empty()),
        }
    }
    pub async fn snapshot(&self) -> Result<FamilySnapshot> {
        Ok(self.snapshot_of(self.repository.load().await?))
    }
    pub async fn save_settings(&self, input: SaveFamilySettings) -> Result<FamilySnapshot> {
        engine::validate_settings(&input.settings)?;
        for secret in [&input.resend_api_key, &input.webhook_token]
            .into_iter()
            .flatten()
        {
            if secret.len() > 4096 || secret.contains(['\r', '\n']) {
                return Err(engine::invalid("Invalid delivery credential"));
            }
        }
        if let Some(url) = &input.webhook_url {
            if url.len() > 2048 {
                return Err(engine::invalid("Webhook URL is too long"));
            }
            if !url.is_empty() {
                crate::delivery::validate_webhook_url(url)?;
            }
        }
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        let proposed = ProposedDeliverySecrets {
            current: self.secrets.as_ref(),
            changes: [
                (RESEND_KEY, input.resend_api_key.as_deref()),
                (WEBHOOK_TOKEN_KEY, input.webhook_token.as_deref()),
                (WEBHOOK_URL_KEY, input.webhook_url.as_deref()),
            ],
        };
        if (input.settings.email_alerts || input.settings.email_digest)
            && !self.delivery.email_configured(&proposed)
        {
            return Err(engine::invalid(
                "Configure an email provider key before enabling email delivery",
            ));
        }
        if (input.settings.webhook_alerts || input.settings.webhook_digest)
            && proposed
                .get_secret(WEBHOOK_URL_KEY)?
                .is_none_or(|s| s.is_empty())
        {
            return Err(engine::invalid(
                "Configure a webhook address before enabling delivery",
            ));
        }
        for (key, value) in [
            (RESEND_KEY, input.resend_api_key),
            (WEBHOOK_TOKEN_KEY, input.webhook_token),
            (WEBHOOK_URL_KEY, input.webhook_url),
        ] {
            if let Some(value) = value {
                self.secrets
                    .set_secret(key, &value)
                    .map_err(|_| engine::invalid("Unable to save delivery credential"))?;
            }
        }
        state.settings = input.settings;
        state.config_revision = state.config_revision.saturating_add(1);
        for d in &mut state.deliveries {
            if d.status == DeliveryStatus::Pending {
                d.status = DeliveryStatus::Cancelled;
            }
        }
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn action(&self, input: InboxAction, now: DateTime<Utc>) -> Result<FamilySnapshot> {
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        engine::apply_action(&mut state, input, now)?;
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn bulk_action(
        &self,
        input: BulkInboxAction,
        now: DateTime<Utc>,
    ) -> Result<FamilySnapshot> {
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        engine::apply_bulk_action(&mut state, input, now)?;
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn task(&self, input: FamilyTask, now: DateTime<Utc>) -> Result<FamilySnapshot> {
        if input.title.trim().is_empty() || input.title.len() > 160 || input.detail.len() > 1000 {
            return Err(engine::invalid("Invalid family task"));
        }
        self.sources.validate_account(&input.account_id)?;
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        let key = format!("statement:{}", input.account_id);
        if input.resolved {
            if let Some(item) = state.items.iter_mut().find(|i| i.fact.key == key) {
                item.status = InboxStatus::Resolved;
                item.active = false;
                item.updated_at = now;
            }
        } else {
            let evaluation = Evaluation {
                candidates: vec![Candidate {
                    key,
                    source: "statement_tasks".into(),
                    kind: "reconciliation".into(),
                    severity: "info".into(),
                    title: input.title,
                    detail: input.detail,
                    href: format!("/accounts/{}", crate::sources::escaped(&input.account_id)),
                    amount: None,
                    currency: None,
                    notify: false,
                }],
                ..Default::default()
            };
            let warnings = state.warnings.clone();
            let last_evaluated = state.last_evaluated;
            engine::reconcile(&mut state, evaluation, now);
            state.warnings = warnings;
            state.last_evaluated = last_evaluated;
        }
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn preview_digest(&self, month: &str, now: DateTime<Utc>) -> Result<MonthlyDigest> {
        let state = self.repository.load().await?;
        self.sources.digest(month, &state.settings, now).await
    }
    pub async fn refresh(&self, now: DateTime<Utc>) -> Result<FamilySnapshot> {
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        if !state.settings.enabled {
            return Ok(self.snapshot_of(state));
        }
        let evaluation = self.sources.evaluate(&state.settings, now).await?;
        let ids = engine::reconcile(&mut state, evaluation, now);
        if !ids.is_empty() {
            let mut text = String::from("Family money — needs attention\n\n");
            for item in state.items.iter().filter(|i| ids.contains(&i.id)).take(20) {
                text.push_str(&format!("- {}: {}\n", item.fact.title, item.fact.detail));
            }
            text.push_str("\nOpen the Family inbox to review and decide together.");
            let n = Notification {
                subject: format!("Family money: {} new alerts", ids.len()),
                text,
                digest_month: None,
                item_ids: ids,
            };
            queue(&mut state, n, now);
        }
        let timezone = self.sources.timezone()?;
        if let Some(month) = engine::due_month(now, &timezone) {
            if !state.digests.contains_key(&month) {
                let digest = self.sources.digest(&month, &state.settings, now).await?;
                let n = Notification {
                    subject: format!("Family Money Meeting — {month}"),
                    text: digest.text.clone(),
                    digest_month: Some(month.clone()),
                    item_ids: vec![],
                };
                state.digests.insert(month, digest);
                queue(&mut state, n, now);
                while state.digests.len() > 24 {
                    if let Some(key) = state.digests.keys().next().cloned() {
                        state.digests.remove(&key);
                    }
                }
            }
        }
        state.deliveries.retain(|d| {
            d.created_at > now - Duration::days(180)
                || matches!(
                    d.status,
                    DeliveryStatus::Pending | DeliveryStatus::Sending | DeliveryStatus::Failed
                )
        });
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn dispatch(&self, now: DateTime<Utc>) -> Result<()> {
        loop {
            // Release between messages so settings changes need wait for at most
            // one bounded request. Keep credentials/settings stable during that request.
            let _guard = self.gate.lock().await;
            let mut state = self.repository.load().await?;
            if !state.settings.enabled {
                return Ok(());
            }
            let mut cancelled = false;
            for index in 0..state.deliveries.len() {
                let d = &state.deliveries[index];
                if d.status == DeliveryStatus::Pending
                    && (!enabled(&state.settings, d)
                        || d.config_revision != state.config_revision
                        || d.updated_at < now - Duration::days(7))
                {
                    state.deliveries[index].status = DeliveryStatus::Cancelled;
                    cancelled = true;
                }
            }
            let Some(index) = state
                .deliveries
                .iter()
                .position(|d| d.status == DeliveryStatus::Pending)
            else {
                if cancelled {
                    self.repository.save(state).await?;
                }
                return Ok(());
            };
            state.deliveries[index].status = DeliveryStatus::Sending;
            state.deliveries[index].updated_at = now;
            self.repository.save(state.clone()).await?; // No network call unless the claim is durable.
            let result = self
                .delivery
                .send(
                    &state.deliveries[index],
                    &state.settings,
                    self.secrets.as_ref(),
                )
                .await;
            state.deliveries[index].status = if result.is_ok() {
                DeliveryStatus::Delivered
            } else {
                DeliveryStatus::Failed
            };
            state.deliveries[index].error = result.err().map(|_| {
                "Delivery failed or receipt is uncertain. Check receipt before retrying.".into()
            });
            state.deliveries[index].updated_at = Utc::now();
            self.repository.save(state).await?;
        }
    }
    pub async fn retry_delivery(&self, id: &str, now: DateTime<Utc>) -> Result<FamilySnapshot> {
        let _guard = self.gate.lock().await;
        let mut state = self.repository.load().await?;
        let index = state
            .deliveries
            .iter()
            .position(|d| d.id == id)
            .ok_or_else(|| engine::invalid("Delivery not found"))?;
        let d = &state.deliveries[index];
        if !matches!(d.status, DeliveryStatus::Failed | DeliveryStatus::Sending)
            || d.config_revision != state.config_revision
            || !state.settings.enabled
            || !enabled(&state.settings, d)
        {
            return Err(engine::invalid(
                "This delivery cannot be retried with the current settings",
            ));
        }
        state.deliveries[index].status = DeliveryStatus::Pending;
        state.deliveries[index].updated_at = now;
        self.repository.save(state.clone()).await?;
        Ok(self.snapshot_of(state))
    }
    pub async fn tick(&self) -> Result<()> {
        self.refresh(Utc::now()).await?;
        self.dispatch(Utc::now()).await
    }
}
fn enabled(s: &FamilySettings, d: &DeliveryRecord) -> bool {
    match (d.channel.as_str(), d.notification.digest_month.is_some()) {
        ("email", true) => s.email_digest,
        ("email", false) => s.email_alerts,
        ("webhook", true) => s.webhook_digest,
        ("webhook", false) => s.webhook_alerts,
        _ => false,
    }
}
fn queue(state: &mut FamilyState, n: Notification, now: DateTime<Utc>) {
    for channel in ["email", "webhook"] {
        let d = DeliveryRecord {
            id: uuid::Uuid::now_v7().to_string(),
            channel: channel.into(),
            config_revision: state.config_revision,
            notification: n.clone(),
            status: DeliveryStatus::Pending,
            created_at: now,
            updated_at: now,
            error: None,
        };
        if enabled(&state.settings, &d) {
            state.deliveries.push(d);
        }
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
