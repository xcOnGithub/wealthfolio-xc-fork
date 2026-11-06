//! Provider records. Financial payloads are never Debug/logged; serialization is
//! restricted to authenticated source-history views and local profile persistence.
//! Unknown type/subtype strings remain available for explicit ingestion review.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AccessToken, LinkCredential};

/// Credentials must be persisted only through the encrypted SecretStore.
pub struct TokenExchange {
    pub item_id: String,
    pub access_token: AccessToken,
}

pub struct LinkToken {
    pub link_token: LinkCredential,
    pub expiration: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub item_id: String,
    pub institution_id: Option<String>,
    pub consent_expiration_time: Option<DateTime<Utc>>,
    pub available_products: Vec<String>,
    pub billed_products: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Account {
    pub account_id: String,
    pub name: String,
    pub mask: Option<String>,
    #[serde(rename = "type")]
    pub account_type: String,
    pub subtype: Option<String>,
    pub balances: Balances,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Balances {
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    pub current: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    pub available: Option<Decimal>,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AccountsResponse {
    pub accounts: Vec<Account>,
    pub item: Item,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct BankTransaction {
    pub transaction_id: String,
    pub account_id: String,
    pub pending_transaction_id: Option<String>,
    pub pending: bool,
    pub date: NaiveDate,
    pub authorized_date: Option<NaiveDate>,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub amount: Decimal,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
    pub name: String,
    pub personal_finance_category: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RemovedTransaction {
    pub transaction_id: String,
    pub account_id: String,
}

/// One page only. Never advance a persisted cursor until the whole batch applies.
#[derive(Clone, Serialize, Deserialize)]
pub struct TransactionsPage {
    pub added: Vec<BankTransaction>,
    pub modified: Vec<BankTransaction>,
    pub removed: Vec<RemovedTransaction>,
    pub next_cursor: String,
    pub has_more: bool,
    pub transactions_update_status: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Security {
    pub security_id: String,
    pub ticker_symbol: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub security_type: Option<String>,
    pub subtype: Option<String>,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
    pub is_cash_equivalent: Option<bool>,
    pub option_contract: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Holding {
    pub account_id: String,
    pub security_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub quantity: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub institution_price: Decimal,
    pub institution_price_as_of: Option<NaiveDate>,
    pub institution_price_datetime: Option<DateTime<Utc>>,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub institution_value: Decimal,
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    pub cost_basis: Option<Decimal>,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HoldingsResponse {
    pub accounts: Vec<Account>,
    pub item: Item,
    pub holdings: Vec<Holding>,
    pub securities: Vec<Security>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InvestmentTransaction {
    pub investment_transaction_id: String,
    pub account_id: String,
    pub security_id: Option<String>,
    pub cancel_transaction_id: Option<String>,
    pub date: NaiveDate,
    pub name: String,
    #[serde(rename = "type")]
    pub transaction_type: String,
    pub subtype: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub amount: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub quantity: Decimal,
    #[serde(default, with = "rust_decimal::serde::arbitrary_precision_option")]
    pub fees: Option<Decimal>,
    pub iso_currency_code: Option<String>,
    pub unofficial_currency_code: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InvestmentsPage {
    pub accounts: Vec<Account>,
    pub item: Item,
    pub investment_transactions: Vec<InvestmentTransaction>,
    pub securities: Vec<Security>,
    pub total_investment_transactions: u32,
}

/// Sensitive Link URL is returned only to the authenticated initiating caller.
pub struct HostedLink {
    pub token: LinkCredential,
    pub url: String,
    pub expiration: DateTime<Utc>,
}
