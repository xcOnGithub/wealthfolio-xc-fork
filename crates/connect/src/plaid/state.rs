//! Profile-local connection, consent and staging contracts. No token values here.
use super::{BankTransaction, PlaidEnvironment, PlaidProduct};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wealthfolio_core::{activities::ActivityUpsert, portfolio::snapshot::AccountStateSnapshot};

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("Direct Plaid is disabled")]
    Disabled,
    #[error("Plaid connection is unavailable for this profile or session")]
    Access,
    #[error("Plaid state changed; reload before continuing")]
    Changed,
    #[error("Plaid configuration is incomplete or invalid")]
    Configuration,
    #[error("Review the account data or unsupported records before importing")]
    Review,
    #[error("Plaid connection needs reconnection or operator attention")]
    Attention,
    #[error("Plaid data is not ready; try again later")]
    NotReady,
    #[error("Plaid storage is unavailable")]
    Storage,
    #[error("Plaid rate limit reached; retry later")]
    RateLimited,
    #[error("Plaid request failed; retry later")]
    Provider,
}

impl From<super::PlaidError> for SyncError {
    fn from(error: super::PlaidError) -> Self {
        use super::PlaidError as E;
        match error {
            E::NotReady => Self::NotReady,
            E::Credentials | E::ReconnectRequired => Self::Attention,
            E::Configuration => Self::Configuration,
            E::RateLimited => Self::RateLimited,
            _ => Self::Provider,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    // Never included in public DTOs. Bound to the initiating profile unlock.
    pub owner: String,
    pub environment: PlaidEnvironment,
    pub product: PlaidProduct,
    pub institution: String,
    pub institution_id: String,
    pub item_id: Option<String>,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub update_mode: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub kind: String,
    pub group: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub id: String,
    pub connection_id: String,
    pub external_id: String,
    pub account_id: String,
    pub currency: String,
    pub credit_card: bool,
    #[serde(default)]
    pub cutover_date: Option<chrono::NaiveDate>,
    pub cursor: Option<String>,
    pub last_success: Option<DateTime<Utc>>,
    pub status: String,
    #[serde(default)]
    pub last_error: Option<String>,
    pub decisions: BTreeMap<String, Decision>,
    pub staged: Option<BankStage>,
    pub source_history: serde_json::Value,
    #[serde(default)]
    pub holdings_staged: Option<HoldingsStage>,
    #[serde(default)]
    pub opaque_security_ids: BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BankStage {
    pub records: Vec<BankTransaction>,
    pub removed: Vec<String>,
    pub next_cursor: String,
    pub checkpoint_in: Option<String>,
    pub balance: String,
    pub fetched_at: DateTime<Utc>,
    pub approved: bool,
    pub opening_balance: Option<String>,
    pub digest: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HoldingsStage {
    pub response: super::HoldingsResponse,
    pub history: Vec<super::InvestmentTransaction>,
    pub fetched_at: DateTime<Utc>,
    pub digest: String,
}

pub struct BankCommit {
    pub mapping: Mapping,
    pub expected_cursor: Option<String>,
    pub activities: Vec<ActivityUpsert>,
    pub earliest: Option<DateTime<Utc>>,
    pub removed_ids: Vec<String>,
}

#[async_trait]
pub trait PlaidRepository: Send + Sync {
    async fn connections(&self) -> Result<Vec<Connection>>;
    async fn put_connection(
        &self,
        connection: Connection,
        expected_status: Option<String>,
    ) -> Result<()>;
    /// Remove only an unused, fully disconnected connection; retain all financial data.
    async fn remove_connection(&self, connection: Connection) -> Result<()>;
    async fn mappings(&self) -> Result<Vec<Mapping>>;
    /// Atomic metadata-only staging/decision writes; mapping ownership is immutable.
    async fn put_mappings(&self, mappings: Vec<Mapping>) -> Result<()>;
    /// Existing accounts must be empty, manual accounts of the appropriate tracking mode.
    async fn add_mapping(&self, mapping: Mapping, product: PlaidProduct) -> Result<()>;
    /// Apply all ready account batches + import runs + cursors in one writer transaction.
    async fn commit_banks(&self, batches: Vec<BankCommit>) -> Result<()>;
    async fn commit_holdings(
        &self,
        mapping: Mapping,
        snapshot: AccountStateSnapshot,
        quotes: Vec<wealthfolio_core::quotes::Quote>,
    ) -> Result<()>;
}

pub fn secret_key(connection: &Connection, purpose: &str) -> Result<String> {
    if !matches!(purpose, "access" | "link" | "revocation") {
        return Err(SyncError::Configuration);
    }
    let id = uuid::Uuid::parse_str(&connection.id).map_err(|_| SyncError::Configuration)?;
    let env = match connection.environment {
        PlaidEnvironment::Sandbox => "sandbox",
        PlaidEnvironment::Production => "production",
    };
    Ok(format!(
        "{}{}:{}:{}",
        wealthfolio_core::secrets::PLAID_SECRET_PREFIX,
        env,
        id,
        purpose
    ))
}
