//! Web runtime wiring for direct Plaid. Credentials are mounted at runtime only.
use std::{collections::BTreeMap, sync::Arc};
use wealthfolio_connect::plaid::{
    service::PlaidService, PlaidClient, PlaidConfig, PlaidEnvironment,
};

pub struct PlaidRuntime {
    pub service: Arc<PlaidService>,
    pub origin: String,
}
pub struct Settings {
    pub client: PlaidClient,
    pub environment: PlaidEnvironment,
    pub institutions: BTreeMap<String, String>,
    pub origin: String,
}

pub fn settings(encrypted_database: bool) -> anyhow::Result<Option<Settings>> {
    settings_with(encrypted_database, |key| std::env::var(key).ok())
}

fn settings_with(
    encrypted_database: bool,
    value: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<Settings>> {
    let flag = value("WF_PLAID_ENABLED").unwrap_or_default();
    match flag.as_str() {
        "" | "false" | "0" => return Ok(None),
        "true" | "1" => {}
        _ => anyhow::bail!("Invalid WF_PLAID_ENABLED"),
    }
    anyhow::ensure!(
        encrypted_database,
        "Direct Plaid requires an encrypted profile database and credential store"
    );
    let env = value("WF_PLAID_ENV").unwrap_or_default();
    let environment = env
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid WF_PLAID_ENV"))?;
    let client_id = value("WF_PLAID_CLIENT_ID").unwrap_or_default();
    let secret_path = value("WF_PLAID_SECRET_FILE")
        .ok_or_else(|| anyhow::anyhow!("WF_PLAID_SECRET_FILE is required"))?;
    let bytes = std::fs::metadata(&secret_path)
        .map_err(|_| anyhow::anyhow!("Plaid credential file is unavailable"))?;
    anyhow::ensure!(
        bytes.is_file() && bytes.len() <= 4096,
        "Invalid Plaid credential file"
    );
    let secret = zeroize::Zeroizing::new(
        std::fs::read_to_string(secret_path)
            .map_err(|_| anyhow::anyhow!("Plaid credential file is unavailable"))?,
    );
    let redirect = value("WF_PLAID_REDIRECT_URI").unwrap_or_default();
    let config = PlaidConfig::when_enabled(true, &env, &client_id, secret.trim(), &redirect)
        .map_err(|_| anyhow::anyhow!("Invalid direct Plaid configuration"))?
        .ok_or_else(|| anyhow::anyhow!("Direct Plaid is disabled"))?;
    let uri = reqwest::Url::parse(&redirect)
        .map_err(|_| anyhow::anyhow!("Invalid Plaid redirect URI"))?;
    anyhow::ensure!(
        uri.path() == "/settings/accounts",
        "Plaid return URI must be the HTTPS /settings/accounts page"
    );
    let institutions: BTreeMap<String,String> = serde_json::from_str(&value("WF_PLAID_INSTITUTIONS").unwrap_or_default())
        .map_err(|_| anyhow::anyhow!("WF_PLAID_INSTITUTIONS must map supported institutions to their privately tested Plaid IDs"))?;
    anyhow::ensure!(
        !institutions.is_empty(),
        "At least one verified institution is required"
    );
    for (key, id) in &institutions {
        wealthfolio_connect::plaid::service::product_for(key)
            .map_err(|_| anyhow::anyhow!("Unsupported institution key"))?;
        anyhow::ensure!(
            id.strip_prefix("ins_")
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())),
            "Invalid configured institution ID"
        );
    }
    Ok(Some(Settings {
        client: PlaidClient::new(config)
            .map_err(|_| anyhow::anyhow!("Plaid HTTP client unavailable"))?,
        environment,
        institutions,
        origin: uri.origin().ascii_serialization(),
    }))
}

pub fn start_scheduler(state: Arc<crate::AppState>) {
    if state.plaid.is_none() {
        return;
    }
    let runtime = state.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(3600));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            // The existing lifecycle lock orders this work before profile deletion/restore.
            // Cancellation leaves uncommitted cursors untouched and exchange claims durable.
            let result = tokio::time::timeout(std::time::Duration::from_secs(240), async {
                let _lifecycle = runtime.profile_lifecycle.lock().await;
                let Some(_guard) =
                    wealthfolio_connect::acquire_broker_sync_guard(&runtime.broker_sync_running)
                else {
                    return;
                };
                if let Some(plaid) = &runtime.plaid {
                    let _ = plaid.service.sync(true, &|| Ok(())).await;
                }
            })
            .await;
            if result.is_err() {
                tracing::warn!("Direct Plaid scheduled sync reached its time budget");
            }
        }
    });
    state
        .workers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(task);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_settings_do_not_read_any_credentials() {
        for flag in [None, Some("false"), Some("0"), Some("")] {
            assert!(settings_with(false, |key| {
                assert_eq!(key, "WF_PLAID_ENABLED");
                flag.map(str::to_owned)
            })
            .unwrap()
            .is_none());
        }
    }
    #[test]
    fn invalid_flag_and_unencrypted_profile_fail_before_credential_access() {
        for (flag, encrypted) in [("yes", true), ("true", false), ("1", false)] {
            assert!(settings_with(encrypted, |key| {
                assert_eq!(key, "WF_PLAID_ENABLED");
                Some(flag.into())
            })
            .is_err());
        }
    }
}
