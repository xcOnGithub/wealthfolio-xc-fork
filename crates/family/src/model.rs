use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use wealthfolio_core::secrets::{
    FAMILY_RESEND_API_KEY as RESEND_KEY, FAMILY_WEBHOOK_TOKEN_KEY as WEBHOOK_TOKEN_KEY,
    FAMILY_WEBHOOK_URL_KEY as WEBHOOK_URL_KEY,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct FamilySettings {
    pub enabled: bool,
    pub large_charge_amount: f64,
    pub unusual_multiplier: f64,
    pub unusual_minimum: f64,
    pub budget_threshold: f64,
    pub consent_notice_days: i64,
    pub stale_valuation_days: i64,
    pub bill_notice_days: i64,
    pub email_alerts: bool,
    pub email_digest: bool,
    pub webhook_alerts: bool,
    pub webhook_digest: bool,
    pub email_from: String,
    pub recipients: Vec<String>,
    pub bills: Vec<FamilyBill>,
}
impl Default for FamilySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            large_charge_amount: 1000.0,
            unusual_multiplier: 3.0,
            unusual_minimum: 100.0,
            budget_threshold: 0.8,
            consent_notice_days: 14,
            stale_valuation_days: 90,
            bill_notice_days: 30,
            email_alerts: false,
            email_digest: false,
            webhook_alerts: false,
            webhook_digest: false,
            email_from: String::new(),
            recipients: vec![],
            bills: vec![],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamilyBill {
    pub id: String,
    pub name: String,
    pub due_date: NaiveDate,
    pub amount: f64,
    pub currency: String,
    pub annual: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveFamilySettings {
    pub settings: FamilySettings,
    pub resend_api_key: Option<String>,
    pub webhook_token: Option<String>,
    pub webhook_url: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    Open,
    Acknowledged,
    Snoozed,
    Resolved,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub key: String,
    pub source: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub detail: String,
    pub href: String,
    #[serde(default, deserialize_with = "optional_finite_number")]
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub notify: bool,
}
// The fork enables serde_json arbitrary_precision. Flattened fields are first
// buffered by Serde, where decimal JSON numbers become Number's private map;
// deserialize through Number rather than treating that buffer as a direct f64.
fn optional_finite_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<f64>, D::Error> {
    Option::<serde_json::Number>::deserialize(deserializer)?
        .map(|number| {
            number
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| serde::de::Error::custom("Invalid alert amount"))
        })
        .transpose()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,
    #[serde(flatten)]
    pub fact: Candidate,
    pub status: InboxStatus,
    pub active: bool,
    pub first_seen: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub snoozed_until: Option<DateTime<Utc>>,
    pub note: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxAction {
    pub id: String,
    pub action: String,
    pub snoozed_until: Option<DateTime<Utc>>,
    pub note: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamilyTask {
    pub account_id: String,
    pub title: String,
    pub detail: String,
    pub resolved: bool,
}
#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    pub candidates: Vec<Candidate>,
    pub complete_sources: Vec<String>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendingMover {
    pub name: String,
    pub current: f64,
    pub previous: f64,
    pub change: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyGoal {
    pub title: String,
    pub progress: Option<f64>,
    pub current: Option<f64>,
    pub target: Option<f64>,
    pub currency: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthlyDigest {
    pub month: String,
    pub generated_at: DateTime<Utc>,
    pub currency: String,
    pub net_worth: Option<f64>,
    pub net_worth_change: Option<f64>,
    pub income: Option<f64>,
    pub spending: Option<f64>,
    pub savings_rate: Option<f64>,
    pub movers: Vec<SpendingMover>,
    pub goals: Vec<FamilyGoal>,
    pub upcoming_bills: Vec<FamilyBill>,
    pub warnings: Vec<String>,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub subject: String,
    pub text: String,
    pub digest_month: Option<String>,
    pub item_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
    Sending,
    Delivered,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    pub id: String,
    pub channel: String,
    pub config_revision: u64,
    pub notification: Notification,
    pub status: DeliveryStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FamilyState {
    pub settings: FamilySettings,
    pub config_revision: u64,
    pub items: Vec<InboxItem>,
    pub digests: BTreeMap<String, MonthlyDigest>,
    pub deliveries: Vec<DeliveryRecord>,
    pub last_evaluated: Option<DateTime<Utc>>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilySnapshot {
    #[serde(flatten)]
    pub state: FamilyState,
    pub resend_configured: bool,
    pub webhook_token_configured: bool,
    pub webhook_configured: bool,
}
