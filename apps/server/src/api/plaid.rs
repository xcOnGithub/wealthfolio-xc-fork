use crate::{
    profiles::{ProfileAccess, WebProfiles},
    AppState,
};
use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use wealthfolio_connect::plaid::{state::SyncError, AccessToken};

#[derive(Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Action {
    Status {},
    Start {
        institution: String,
        reconnect: Option<String>,
        #[serde(default)]
        duplicate: bool,
    },
    Finish {
        flow_id: String,
    },
    Accounts {
        connection_id: String,
    },
    Map {
        connection_id: String,
        external_id: String,
        account_id: String,
        #[serde(default)]
        merrill_confirmed: bool,
        cutover_date: Option<String>,
    },
    CreateAndMap {
        connection_id: String,
        external_id: String,
        name: String,
        #[serde(default)]
        merrill_confirmed: bool,
    },
    Preview {
        mapping_id: String,
        #[serde(default)]
        refresh: bool,
    },
    Decide {
        mapping_id: String,
        record_id: String,
        kind: String,
    },
    Pair {
        mapping_id: String,
        record_id: String,
        other_mapping_id: String,
        other_record_id: String,
    },
    Approve {
        mapping_id: String,
        digest: String,
        opening_balance: Option<String>,
    },
    PreviewInvestments {
        mapping_id: String,
    },
    ApproveInvestments {
        mapping_id: String,
        digest: String,
        #[serde(default)]
        opening_costs: BTreeMap<String, String>,
        #[serde(default)]
        confirmed: bool,
    },
    PreviewHoldings {
        mapping_id: String,
    },
    ApproveHoldings {
        mapping_id: String,
        digest: String,
        #[serde(default)]
        allow_opaque: bool,
    },
    Sync {},
    Disconnect {
        connection_id: String,
    },
    RemoveConnection {
        connection_id: String,
    },
    /// Owner-run migration from private tests. Never echoed or made a generic secret.
    Adopt {
        institution: String,
        access_token: String,
    },
    History {
        mapping_id: String,
        #[serde(default)]
        offset: usize,
    },
}

fn error(error: SyncError) -> (StatusCode, Json<Value>) {
    let status = match error {
        SyncError::Access => StatusCode::FORBIDDEN,
        SyncError::Disabled => StatusCode::NOT_IMPLEMENTED,
        SyncError::Changed => StatusCode::CONFLICT,
        SyncError::Storage => StatusCode::SERVICE_UNAVAILABLE,
        SyncError::Provider => StatusCode::BAD_GATEWAY,
        SyncError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"message":error.to_string()})))
}

fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    headers.get("origin").and_then(|v| v.to_str().ok()) == Some(origin)
        && headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| matches!(v, "same-origin" | "none"))
}

async fn action(
    Extension(state): Extension<Arc<AppState>>,
    Extension(profiles): Extension<Arc<WebProfiles>>,
    Extension(access): Extension<ProfileAccess>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, (StatusCode, Json<Value>)> {
    let action: Action =
        serde_json::from_value(body).map_err(|_| error(SyncError::Configuration))?;
    let Some(plaid) = &state.plaid else {
        if matches!(action, Action::Status {}) {
            return Ok(
                Json(json!({"enabled":false,"connections":[],"institutions":[]})).into_response(),
            );
        }
        return Err(error(SyncError::Disabled));
    };
    if !same_origin(&headers, &plaid.origin) {
        return Err(error(SyncError::Access));
    }
    let _lifecycle = state.profile_lifecycle.lock().await;
    let _guard = wealthfolio_connect::acquire_broker_sync_guard(&state.broker_sync_running)
        .ok_or_else(|| error(SyncError::Changed))?;
    let admit = || {
        profiles
            .registry
            .sessions
            .admit(&access.owner, access.session.scope_id)
            .map(|_| ())
            .map_err(|_| SyncError::Access)
    };
    let owner = format!("{}:{}", access.owner, access.session.scope_id);
    let svc = &plaid.service;
    let result: std::result::Result<Value,SyncError> = async {
        Ok(match action {
            Action::Status {} => svc.list(&admit).await?,
            Action::Start {institution,reconnect,duplicate} => svc.start(&owner,&institution,reconnect.as_deref(),duplicate,&admit).await?,
            Action::Finish {flow_id} => svc.finish(&owner,&flow_id,&admit).await?,
            Action::Accounts {connection_id} => svc.accounts(&connection_id,&admit).await?,
            Action::RemoveConnection {connection_id} => { svc.remove_connection(&connection_id,&admit).await?; json!({"ok":true}) },
            Action::Map {connection_id,external_id,account_id,merrill_confirmed,cutover_date} => { svc.map_account(&connection_id,&external_id,&account_id,merrill_confirmed,cutover_date,&admit).await?; Value::Null },
            Action::CreateAndMap {connection_id,external_id,name,merrill_confirmed} => serde_json::to_value(svc.create_and_map(&connection_id,&external_id,&name,merrill_confirmed,&admit).await?).map_err(|_| SyncError::Storage)?,
            Action::Preview {mapping_id,refresh} => svc.preview(&mapping_id,refresh,&admit).await?,
            Action::Decide {mapping_id,record_id,kind} => { svc.decide(&mapping_id,&record_id,&kind,&admit).await?; Value::Null },
            Action::Pair {mapping_id,record_id,other_mapping_id,other_record_id} => {svc.pair(&mapping_id,&record_id,&other_mapping_id,&other_record_id,&admit).await?; Value::Null},
            Action::Approve {mapping_id,digest,opening_balance} => {svc.approve(&mapping_id,&digest,opening_balance,&admit).await?; Value::Null},
            Action::PreviewInvestments {mapping_id} => svc.preview_investments(&mapping_id,&admit).await?,
            Action::ApproveInvestments {mapping_id,digest,opening_costs,confirmed} => {svc.approve_investments(&mapping_id,&digest,opening_costs,confirmed,&admit).await?; Value::Null},
            Action::PreviewHoldings {mapping_id} => svc.preview_holdings(&mapping_id,&admit).await?,
            Action::ApproveHoldings {mapping_id,digest,allow_opaque} => {svc.approve_holdings(&mapping_id,&digest,allow_opaque,&admit).await?; Value::Null},
            Action::Sync {} => svc.sync(false,&admit).await?,
            Action::Disconnect {connection_id} => {svc.disconnect(&connection_id,&admit).await?; Value::Null},
            Action::Adopt {institution,access_token} => svc.adopt(&owner,&institution,AccessToken::new(svc.environment,access_token).map_err(SyncError::from)?,&admit).await?,
            Action::History {mapping_id,offset} => {
                admit()?;
                let mappings = svc.repository.mappings().await?;
                let m = mappings.iter().find(|m| m.id == mapping_id).ok_or(SyncError::Access)?;
                let mut history: Vec<Value> = m.source_history.get("transactions").and_then(Value::as_array).cloned().unwrap_or_default();
                history.sort_by(|a,b| b.get("date").and_then(Value::as_str).cmp(&a.get("date").and_then(Value::as_str)));
                json!({"currency":m.currency,"total":history.len(),"records":history.into_iter().skip(offset).take(100).collect::<Vec<_>>(),"performanceHistoryComplete":false})
            }
        })
    }.await;
    let mut response = Json(result.map_err(error)?).into_response();
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

pub fn router() -> Router {
    Router::new()
        .route("/plaid/action", post(action))
        .layer(axum::extract::DefaultBodyLimit::max(65536))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_require_exact_origin_and_reject_cross_site_fetch() {
        let mut headers = HeaderMap::new();
        assert!(!same_origin(&headers, "https://finance.example"));
        headers.insert("origin", "https://finance.example.evil".parse().unwrap());
        assert!(!same_origin(&headers, "https://finance.example"));
        headers.insert("origin", "https://finance.example".parse().unwrap());
        assert!(same_origin(&headers, "https://finance.example"));
        for site in ["same-origin", "none", "cross-site", "same-site"] {
            headers.insert("sec-fetch-site", site.parse().unwrap());
            assert_eq!(
                same_origin(&headers, "https://finance.example"),
                matches!(site, "same-origin" | "none")
            );
        }
    }
    #[test]
    fn create_and_map_accepts_only_user_choices_not_provider_fields_or_cutover() {
        let input = json!({"operation":"create_and_map","connectionId":"connection","externalId":"external","name":"Synthetic"});
        assert!(matches!(
            serde_json::from_value::<Action>(input.clone()).unwrap(),
            Action::CreateAndMap {
                merrill_confirmed: false,
                ..
            }
        ));
        for field in [
            "accountId",
            "currency",
            "accountType",
            "provider",
            "cutoverDate",
            "trackingMode",
        ] {
            let mut invalid = input.clone();
            invalid[field] = json!("untrusted");
            assert!(
                serde_json::from_value::<Action>(invalid).is_err(),
                "{field}"
            );
        }
        assert!(serde_json::from_value::<Action>(json!({"operation":"map","connectionId":"connection","externalId":"external","accountId":"local"})).is_ok());
    }

    #[test]
    fn unknown_operations_and_fields_are_rejected_without_echoing_payloads() {
        for value in [
            json!({"operation":"private-credential"}),
            json!({"operation":"status","secret":"private-credential"}),
            json!({"operation":"approve","mappingId":"x"}),
        ] {
            assert!(serde_json::from_value::<Action>(value).is_err());
        }
        for (error_value, status) in [
            (SyncError::Access, StatusCode::FORBIDDEN),
            (SyncError::Changed, StatusCode::CONFLICT),
            (SyncError::Disabled, StatusCode::NOT_IMPLEMENTED),
            (SyncError::RateLimited, StatusCode::TOO_MANY_REQUESTS),
            (SyncError::Provider, StatusCode::BAD_GATEWAY),
            (SyncError::Review, StatusCode::BAD_REQUEST),
        ] {
            assert_eq!(error(error_value).0, status);
        }
    }

    #[test]
    fn connection_removal_accepts_only_the_scoped_connection_identifier() {
        let action: Action = serde_json::from_value(json!({
            "operation":"remove_connection", "connectionId":"fixture"
        }))
        .unwrap();
        assert!(
            matches!(action, Action::RemoveConnection {connection_id} if connection_id == "fixture")
        );
        for extra in ["accessToken", "accountId", "force"] {
            let mut value = json!({"operation":"remove_connection", "connectionId":"fixture"});
            value[extra] = json!("fixture");
            assert!(serde_json::from_value::<Action>(value).is_err());
        }
    }
}
