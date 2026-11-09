//! Direct provider orchestration. Runtime adapters own admission and serialization.
use super::{ingest, state::*, *};
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, RwLock},
};
use wealthfolio_core::{
    activities::ActivityRepositoryTrait,
    assets::{AssetServiceTrait, AssetSpec, InstrumentType},
    events::{DomainEvent, DomainEventSink},
    portfolio::snapshot::{AccountStateSnapshot, Position, SnapshotSource},
    secrets::SecretStore,
};

pub type Admission<'a> = &'a (dyn Fn() -> state::Result<()> + Send + Sync);

#[async_trait]
pub trait PlaidApi: Send + Sync {
    async fn link(
        &self,
        owner: &str,
        product: PlaidProduct,
        flow: &str,
        access: Option<&AccessToken>,
        include_transactions: bool,
    ) -> std::result::Result<HostedLink, PlaidError>;
    async fn result(
        &self,
        link: &LinkCredential,
        update: bool,
    ) -> std::result::Result<Option<PublicToken>, PlaidError>;
    async fn exchange(&self, token: &PublicToken)
        -> std::result::Result<TokenExchange, PlaidError>;
    async fn accounts(
        &self,
        token: &AccessToken,
    ) -> std::result::Result<AccountsResponse, PlaidError>;
    async fn transactions(
        &self,
        token: &AccessToken,
        account: &str,
        cursor: Option<&str>,
    ) -> std::result::Result<TransactionsPage, PlaidError>;
    async fn holdings(
        &self,
        token: &AccessToken,
        account: &str,
    ) -> std::result::Result<HoldingsResponse, PlaidError>;
    async fn investments(
        &self,
        token: &AccessToken,
        account: &str,
        start: NaiveDate,
        end: NaiveDate,
        offset: u32,
    ) -> std::result::Result<InvestmentsPage, PlaidError>;
    async fn remove(&self, token: &AccessToken) -> std::result::Result<(), PlaidError>;
}
#[async_trait]
impl PlaidApi for PlaidClient {
    async fn link(
        &self,
        owner: &str,
        product: PlaidProduct,
        flow: &str,
        access: Option<&AccessToken>,
        include_transactions: bool,
    ) -> std::result::Result<HostedLink, PlaidError> {
        self.create_hosted_link(owner, product, flow, access, include_transactions)
            .await
    }
    async fn result(
        &self,
        link: &LinkCredential,
        update: bool,
    ) -> std::result::Result<Option<PublicToken>, PlaidError> {
        self.hosted_result(link, update).await
    }
    async fn exchange(
        &self,
        token: &PublicToken,
    ) -> std::result::Result<TokenExchange, PlaidError> {
        self.exchange_public_token(token).await
    }
    async fn accounts(
        &self,
        token: &AccessToken,
    ) -> std::result::Result<AccountsResponse, PlaidError> {
        PlaidClient::accounts(self, token).await
    }
    async fn transactions(
        &self,
        token: &AccessToken,
        account: &str,
        cursor: Option<&str>,
    ) -> std::result::Result<TransactionsPage, PlaidError> {
        self.transactions_page(token, account, cursor).await
    }
    async fn holdings(
        &self,
        token: &AccessToken,
        account: &str,
    ) -> std::result::Result<HoldingsResponse, PlaidError> {
        PlaidClient::holdings(self, token, account).await
    }
    async fn investments(
        &self,
        token: &AccessToken,
        account: &str,
        start: NaiveDate,
        end: NaiveDate,
        offset: u32,
    ) -> std::result::Result<InvestmentsPage, PlaidError> {
        self.investments_page(token, account, start, end, offset)
            .await
    }
    async fn remove(&self, token: &AccessToken) -> std::result::Result<(), PlaidError> {
        self.remove_item(token).await
    }
}

pub struct PlaidService {
    pub repository: Arc<dyn PlaidRepository>,
    pub api: Arc<dyn PlaidApi>,
    pub secrets: Arc<dyn SecretStore>,
    pub environment: PlaidEnvironment,
    /// Institution keys -> verified Plaid institution IDs from operator configuration.
    pub institutions: BTreeMap<String, String>,
    pub assets: Arc<dyn AssetServiceTrait>,
    pub activities: Arc<dyn ActivityRepositoryTrait>,
    pub events: Arc<dyn DomainEventSink>,
    pub timezone: Arc<RwLock<String>>,
}

pub fn product_for(institution: &str) -> state::Result<PlaidProduct> {
    match institution {
        "chase" | "boa" | "usbank" | "amex" => Ok(PlaidProduct::Transactions),
        "merrill_edge" | "schwab" | "robinhood" => Ok(PlaidProduct::Investments),
        _ => Err(SyncError::Configuration),
    }
}

pub fn eligible(c: &Connection, a: &Account) -> bool {
    if c.institution == "merrill_edge"
        && a.account_type == "credit"
        && a.subtype.as_deref() == Some("credit card")
    {
        return true;
    }
    match c.product {
        // Plaid Investments also supports cash-management depository accounts,
        // including brokerage CMAs. Ordinary bank deposits remain excluded.
        PlaidProduct::Investments => {
            a.account_type == "investment"
                || (a.account_type == "depository"
                    && a.subtype.as_deref() == Some("cash management"))
        }
        PlaidProduct::Transactions if c.institution == "amex" => {
            a.account_type == "credit" && a.subtype.as_deref() == Some("credit card")
        }
        PlaidProduct::Transactions => {
            (a.account_type == "depository"
                && matches!(a.subtype.as_deref(), Some("checking" | "savings")))
                || (a.account_type == "credit" && a.subtype.as_deref() == Some("credit card"))
        }
    }
}

impl PlaidService {
    async fn local_connection(&self, id: &str) -> state::Result<Connection> {
        self.repository
            .connections()
            .await?
            .into_iter()
            .find(|c| c.id == id && c.environment == self.environment)
            .ok_or(SyncError::Access)
    }
    async fn connection(&self, id: &str) -> state::Result<Connection> {
        let c = self.local_connection(id).await?;
        if self.institutions.get(&c.institution) != Some(&c.institution_id) {
            return Err(SyncError::Access);
        }
        Ok(c)
    }
    fn access(&self, c: &Connection) -> state::Result<AccessToken> {
        let value = self
            .secrets
            .get_secret(&secret_key(c, "access")?)
            .map_err(|_| SyncError::Storage)?
            .ok_or(SyncError::Attention)?;
        AccessToken::new(self.environment, value).map_err(Into::into)
    }
    async fn mapping(&self, id: &str) -> state::Result<Mapping> {
        let m = self
            .repository
            .mappings()
            .await?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or(SyncError::Access)?;
        self.connection(&m.connection_id).await?;
        Ok(m)
    }
    fn verify_accounts(&self, c: &Connection, accounts: &AccountsResponse) -> state::Result<()> {
        if accounts.item.institution_id.as_deref() != Some(&c.institution_id)
            || c.item_id
                .as_ref()
                .is_some_and(|id| id != &accounts.item.item_id)
            || accounts.item.item_id.is_empty()
        {
            return Err(SyncError::Review);
        }
        let product = match c.product {
            PlaidProduct::Investments => "investments",
            PlaidProduct::Transactions => "transactions",
        };
        if !accounts
            .item
            .billed_products
            .iter()
            .chain(&accounts.item.available_products)
            .any(|p| p == product)
        {
            return Err(SyncError::Review);
        }
        Ok(())
    }

    pub async fn list(&self, admit: Admission<'_>) -> state::Result<Value> {
        admit()?;
        let mappings = self.repository.mappings().await?;
        let connections = self.repository.connections().await?.into_iter().filter(|c| c.environment == self.environment).map(|c| {
            let has_token = self.secrets.get_secret(&secret_key(&c,"access")?).map_err(|_| SyncError::Storage)?.is_some();
            Ok(json!({"id":c.id,"institution":c.institution,"product":c.product,"status":if c.status == "connected" && !has_token { "disconnected" } else { &c.status },
                "canRemove":c.status == "disconnected" && !mappings.iter().any(|m| m.connection_id == c.id),
                "accounts":mappings.iter().filter(|m| m.connection_id == c.id).map(|m| json!({"id":m.id,"accountId":m.account_id,"product":m.product(&c),"status":m.status,"lastError":m.last_error,"lastSuccess":m.last_success})).collect::<Vec<_>>() }))
        }).collect::<state::Result<Vec<_>>>()?;
        Ok(
            json!({"enabled":true,"institutions":self.institutions.keys().collect::<Vec<_>>(),"connections":connections}),
        )
    }

    pub async fn start(
        &self,
        owner: &str,
        institution: &str,
        reconnect: Option<&str>,
        duplicate: bool,
        admit: Admission<'_>,
    ) -> state::Result<Value> {
        admit()?;
        let product = product_for(institution)?;
        let institution_id = self
            .institutions
            .get(institution)
            .ok_or(SyncError::Configuration)?
            .clone();
        let mut c = if let Some(id) = reconnect {
            let c = self.connection(id).await?;
            if c.institution != institution
                || !matches!(c.status.as_str(), "connected" | "attention")
            {
                return Err(SyncError::Changed);
            }
            c
        } else {
            let existing = self.repository.connections().await?;
            let active: Vec<_> = existing
                .iter()
                .filter(|c| !matches!(c.status.as_str(), "disconnected" | "expired"))
                .collect();
            if active.len() >= 20
                || (!duplicate && active.iter().any(|c| c.institution == institution))
            {
                return Err(SyncError::Review);
            }
            Connection {
                id: uuid::Uuid::new_v4().to_string(),
                owner: owner.into(),
                environment: self.environment,
                product,
                institution: institution.into(),
                institution_id,
                item_id: None,
                status: "new".into(),
                expires_at: Utc::now(),
                update_mode: false,
            }
        };
        let old_status = c.status.clone();
        let access = if reconnect.is_some() {
            Some(self.access(&c)?)
        } else {
            None
        };
        let link = self
            .api
            .link(
                &c.id,
                product,
                &c.id,
                access.as_ref(),
                c.institution == "merrill_edge",
            )
            .await?;
        admit()?;
        c.owner = owner.into();
        c.expires_at = link.expiration;
        c.update_mode = reconnect.is_some();
        c.status = "linking".into();
        self.secrets
            .set_secret(&secret_key(&c, "link")?, link.token.expose_secret())
            .map_err(|_| SyncError::Storage)?;
        if let Err(error) = self
            .repository
            .put_connection(c.clone(), reconnect.map(|_| old_status))
            .await
        {
            self.secrets
                .delete_secret(&secret_key(&c, "link")?)
                .map_err(|_| SyncError::Storage)?;
            return Err(error);
        }
        Ok(json!({"flowId":c.id,"url":link.url}))
    }

    pub async fn finish(
        &self,
        owner: &str,
        id: &str,
        admit: Admission<'_>,
    ) -> state::Result<Value> {
        admit()?;
        let mut c = self.connection(id).await?;
        // The vault and SQLite cannot share a transaction. A saved access
        // credential proves exchange completed; validate it without exchanging again.
        if c.status == "exchanging" && self.access(&c).is_ok() {
            admit()?;
            c.status = "validating".into();
            self.repository
                .put_connection(c.clone(), Some("exchanging".into()))
                .await?;
        }
        // Once an access token is saved, an explicitly admitted session in this
        // same encrypted profile can resume validation after restart/relogin.
        // Unexchanged Link sessions remain bound to their original session.
        if c.owner != owner && c.status != "validating" {
            return Err(SyncError::Access);
        }
        if c.status == "connected" {
            return Ok(json!({"connectionId":c.id}));
        }
        if c.status != "validating" {
            if c.status != "linking" || c.expires_at <= Utc::now() {
                return Err(SyncError::Attention);
            }
            let value = self
                .secrets
                .get_secret(&secret_key(&c, "link")?)
                .map_err(|_| SyncError::Storage)?
                .ok_or(SyncError::Attention)?;
            let link = LinkCredential::new(self.environment, value)?;
            let public = self.api.result(&link, c.update_mode).await?;
            admit()?;
            c.status = "exchanging".into();
            self.repository
                .put_connection(c.clone(), Some("linking".into()))
                .await?;
            if let Some(public) = public {
                let exchanged = self.api.exchange(&public).await?;
                c.item_id = Some(exchanged.item_id);
                if admit().is_err() {
                    self.quarantine(&mut c, &exchanged.access_token).await?;
                    return Err(SyncError::Access);
                }
                if self
                    .secrets
                    .set_secret(
                        &secret_key(&c, "access")?,
                        exchanged.access_token.expose_secret(),
                    )
                    .is_err()
                {
                    self.quarantine(&mut c, &exchanged.access_token).await?;
                    return Err(SyncError::Storage);
                }
            } else if !c.update_mode {
                return Err(SyncError::Attention);
            }
            c.status = "validating".into();
            self.repository
                .put_connection(c.clone(), Some("exchanging".into()))
                .await?;
        }
        admit()?;
        let accounts = self.api.accounts(&self.access(&c)?).await?;
        if self.verify_accounts(&c, &accounts).is_err() {
            c.status = "pending_revocation".into();
            self.repository
                .put_connection(c.clone(), Some("validating".into()))
                .await?;
            return Err(SyncError::Review);
        }
        admit()?;
        c.item_id = Some(accounts.item.item_id.clone());
        c.status = "connected".into();
        self.repository
            .put_connection(c.clone(), Some("validating".into()))
            .await?;
        if c.update_mode {
            let mut maps = self.repository.mappings().await?;
            maps.retain(|m| m.connection_id == c.id && m.status == "attention");
            for m in &mut maps {
                m.status = if m.last_success.is_some() {
                    "idle"
                } else {
                    "initial_review"
                }
                .into();
                m.last_error = None;
                m.staged = None;
                m.holdings_staged = None;
            }
            self.repository.put_mappings(maps).await?;
        }
        self.secrets
            .delete_secret(&secret_key(&c, "link")?)
            .map_err(|_| SyncError::Storage)?;
        Ok(json!({"connectionId":c.id}))
    }

    /// Issued credentials from a revoked/failed exchange are cleanup-only.
    /// Persist before attempting remote revocation so cancellation cannot lose them.
    async fn quarantine(&self, c: &mut Connection, token: &AccessToken) -> state::Result<()> {
        let key = secret_key(c, "revocation")?;
        if self
            .secrets
            .set_secret(&key, token.expose_secret())
            .is_err()
        {
            let _ = self.api.remove(token).await;
            return Err(SyncError::Storage);
        }
        let previous = c.status.clone();
        c.status = "pending_revocation".into();
        self.repository
            .put_connection(c.clone(), Some(previous))
            .await?;
        if self.api.remove(token).await.is_ok() {
            for purpose in ["access", "link", "revocation"] {
                self.secrets
                    .delete_secret(&secret_key(c, purpose)?)
                    .map_err(|_| SyncError::Storage)?;
            }
            c.status = "disconnected".into();
            self.repository
                .put_connection(c.clone(), Some("pending_revocation".into()))
                .await?;
        }
        Ok(())
    }

    /// Reuse an Item from a private connectivity test without consuming another Link connection.
    pub async fn adopt(
        &self,
        owner: &str,
        institution: &str,
        token: AccessToken,
        admit: Admission<'_>,
    ) -> state::Result<Value> {
        admit()?;
        let mut c = Connection {
            id: uuid::Uuid::new_v4().to_string(),
            owner: owner.into(),
            environment: self.environment,
            product: product_for(institution)?,
            institution: institution.into(),
            institution_id: self
                .institutions
                .get(institution)
                .ok_or(SyncError::Configuration)?
                .clone(),
            item_id: None,
            status: "connected".into(),
            expires_at: Utc::now(),
            update_mode: false,
        };
        let accounts = self.api.accounts(&token).await?;
        self.verify_accounts(&c, &accounts)?;
        admit()?;
        c.item_id = Some(accounts.item.item_id);
        if self
            .repository
            .connections()
            .await?
            .iter()
            .any(|other| other.environment == c.environment && other.item_id == c.item_id)
        {
            return Err(SyncError::Changed);
        }
        self.secrets
            .set_secret(&secret_key(&c, "access")?, token.expose_secret())
            .map_err(|_| SyncError::Storage)?;
        if let Err(error) = self.repository.put_connection(c.clone(), None).await {
            self.secrets
                .delete_secret(&secret_key(&c, "access")?)
                .map_err(|_| SyncError::Storage)?;
            return Err(error);
        }
        Ok(json!({"connectionId":c.id}))
    }

    pub async fn disconnect(&self, id: &str, admit: Admission<'_>) -> state::Result<()> {
        admit()?;
        let mut c = self.local_connection(id).await?;
        let old = c.status.clone();
        c.status = "pending_revocation".into();
        self.repository
            .put_connection(c.clone(), Some(old.clone()))
            .await?;
        let token = match self.access(&c) {
            Ok(token) => Some(token),
            Err(SyncError::Attention) => self
                .secrets
                .get_secret(&secret_key(&c, "revocation")?)
                .map_err(|_| SyncError::Storage)?
                .map(|value| AccessToken::new(self.environment, value))
                .transpose()?,
            Err(error) => return Err(error),
        };
        if let Some(token) = token {
            self.api.remove(&token).await?;
        } else if c.item_id.is_some() || old == "exchanging" {
            return Err(SyncError::Attention);
        }
        admit()?;
        self.secrets
            .delete_secret(&secret_key(&c, "access")?)
            .map_err(|_| SyncError::Storage)?;
        self.secrets
            .delete_secret(&secret_key(&c, "link")?)
            .map_err(|_| SyncError::Storage)?;
        self.secrets
            .delete_secret(&secret_key(&c, "revocation")?)
            .map_err(|_| SyncError::Storage)?;
        c.status = "disconnected".into();
        self.repository
            .put_connection(c, Some("pending_revocation".into()))
            .await
    }

    pub async fn remove_connection(&self, id: &str, admit: Admission<'_>) -> state::Result<()> {
        admit()?;
        let c = self.local_connection(id).await?;
        if c.status != "disconnected" {
            return Err(SyncError::Review);
        }
        // Missing credentials do not establish that remote consent was revoked.
        // Only disconnect can clear these after successful provider revocation.
        for purpose in ["access", "link", "revocation"] {
            if self
                .secrets
                .get_secret(&secret_key(&c, purpose)?)
                .map_err(|_| SyncError::Storage)?
                .is_some()
            {
                return Err(SyncError::Review);
            }
        }
        admit()?;
        self.repository.remove_connection(c).await
    }

    pub async fn accounts(&self, id: &str, admit: Admission<'_>) -> state::Result<Value> {
        admit()?;
        let c = self.connection(id).await?;
        if c.status != "connected" {
            return Err(SyncError::Attention);
        }
        let response = self.api.accounts(&self.access(&c)?).await?;
        self.verify_accounts(&c, &response)?;
        admit()?;
        let account_types: std::collections::BTreeSet<_> = response
            .accounts
            .iter()
            .map(|account| account.account_type.as_str())
            .collect();
        let account_classifications: Vec<_> = response
            .accounts
            .iter()
            .map(|account| (account.account_type.as_str(), account.subtype.as_deref()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|(kind, subtype)| json!({"type":kind,"subtype":subtype}))
            .collect();
        Ok(
            json!({"totalAccounts":response.accounts.len(),"accountTypes":account_types,"accountClassifications":account_classifications,"accounts":response.accounts.iter().filter(|a| eligible(&c,a)).map(|a| json!({"id":a.account_id,"name":a.name,"mask":a.mask,"type":a.account_type,"product":c.account_product(a.account_type == "credit"),"currency":a.balances.iso_currency_code})).collect::<Vec<_>>()}),
        )
    }

    pub async fn map_account(
        &self,
        connection: &str,
        external: &str,
        local: &str,
        merrill_confirmed: bool,
        cutover: Option<String>,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        let c = self.connection(connection).await?;
        if c.status != "connected" {
            return Err(SyncError::Review);
        }
        let response = self.api.accounts(&self.access(&c)?).await?;
        self.verify_accounts(&c, &response)?;
        admit()?;
        let a = response
            .accounts
            .iter()
            .find(|a| a.account_id == external && eligible(&c, a))
            .ok_or(SyncError::Review)?;
        let m = Mapping {
            id: uuid::Uuid::new_v4().to_string(),
            connection_id: c.id.clone(),
            external_id: external.into(),
            account_id: local.into(),
            currency: ingest::currency(a)?,
            credit_card: a.account_type == "credit",
            cutover_date: cutover
                .map(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d"))
                .transpose()
                .map_err(|_| SyncError::Review)?,
            cursor: None,
            last_success: None,
            status: "initial_review".into(),
            last_error: None,
            decisions: BTreeMap::new(),
            staged: None,
            source_history: json!({}),
            holdings_staged: None,
            opaque_security_ids: Default::default(),
        };
        if c.institution == "merrill_edge"
            && m.product(&c) == PlaidProduct::Investments
            && !merrill_confirmed
        {
            return Err(SyncError::Review);
        }
        let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
            &self.timezone.read().map_err(|_| SyncError::Storage)?,
        );
        if m.cutover_date
            .is_some_and(|d| d > Utc::now().with_timezone(&tz).date_naive())
            || (m.product(&c) == PlaidProduct::Investments && m.cutover_date.is_some())
        {
            return Err(SyncError::Review);
        }
        let product = m.product(&c);
        self.repository.add_mapping(m, product).await
    }

    async fn fetch_bank(&self, m: &Mapping, c: &Connection) -> state::Result<BankStage> {
        let access = self.access(c)?;
        let accounts = self.api.accounts(&access).await?;
        self.verify_accounts(c, &accounts)?;
        let account = accounts
            .accounts
            .iter()
            .find(|a| a.account_id == m.external_id && eligible(c, a))
            .ok_or(SyncError::Review)?;
        if ingest::currency(account)? != m.currency
            || (account.account_type == "credit") != m.credit_card
            || m.product(c) != PlaidProduct::Transactions
        {
            return Err(SyncError::Review);
        }
        let balance = account.balances.current.ok_or(SyncError::Review)?
            * if m.credit_card {
                -Decimal::ONE
            } else {
                Decimal::ONE
            };
        // A mutation restarts the complete pagination round, never a single page.
        for _ in 0..3 {
            let mut cursor = m.cursor.clone();
            let mut seen = HashSet::new();
            let mut records = BTreeMap::new();
            let mut removed = HashSet::new();
            let mut restart = false;
            for _ in 0..100 {
                let page = match self
                    .api
                    .transactions(&access, &m.external_id, cursor.as_deref())
                    .await
                {
                    Err(PlaidError::PaginationChanged) => {
                        restart = true;
                        break;
                    }
                    other => other?,
                };
                if page.transactions_update_status.as_deref() != Some("HISTORICAL_UPDATE_COMPLETE")
                    && m.cursor.as_deref().is_none_or(str::is_empty)
                {
                    return Err(SyncError::NotReady);
                }
                // A fully fetched account with no transactions can legitimately
                // return an empty cursor. Some("") records its reviewed opening
                // balance, while still asking Plaid from the beginning next time.
                // Never discard a real checkpoint or accept uncheckpointed data.
                let completed_empty_account = m.cursor.as_deref().is_none_or(str::is_empty)
                    && cursor.as_deref().is_none_or(str::is_empty)
                    && !page.has_more
                    && page.added.is_empty()
                    && page.modified.is_empty()
                    && page.removed.is_empty();
                if page.next_cursor.is_empty() && !completed_empty_account {
                    return Err(SyncError::NotReady);
                }
                if page.has_more
                    && (Some(&page.next_cursor) == cursor.as_ref()
                        || !seen.insert(page.next_cursor.clone()))
                {
                    return Err(SyncError::Review);
                }
                for tx in page.added.into_iter().chain(page.modified) {
                    ingest::validate_transaction(&tx, m)?;
                    if records.insert(tx.transaction_id.clone(), tx).is_some() {
                        return Err(SyncError::Review);
                    }
                }
                for tx in page.removed {
                    if tx.account_id != m.external_id || tx.transaction_id.is_empty() {
                        return Err(SyncError::Review);
                    }
                    removed.insert(tx.transaction_id);
                }
                if records.len() + removed.len() > ingest::MAX_RECORDS {
                    return Err(SyncError::Review);
                }
                cursor = Some(page.next_cursor);
                if !page.has_more {
                    // Plaid removals are authoritative for the completed batch.
                    for id in &removed {
                        records.remove(id);
                    }
                    let mut stage = BankStage {
                        records: records.into_values().collect(),
                        removed: removed.into_iter().collect(),
                        next_cursor: cursor.ok_or(SyncError::Review)?,
                        checkpoint_in: m.cursor.clone(),
                        balance: balance.to_string(),
                        fetched_at: Utc::now(),
                        approved: false,
                        opening_balance: None,
                        digest: String::new(),
                    };
                    stage.removed.sort();
                    if serde_json::to_vec(&stage)
                        .map_err(|_| SyncError::Storage)?
                        .len()
                        > ingest::MAX_STAGE_BYTES
                    {
                        return Err(SyncError::Review);
                    }
                    stage.digest = ingest::digest(&stage, &m.decisions)?;
                    return Ok(stage);
                }
            }
            if !restart {
                return Err(SyncError::Review);
            }
        }
        Err(SyncError::Provider)
    }

    pub async fn preview(
        &self,
        id: &str,
        refresh: bool,
        admit: Admission<'_>,
    ) -> state::Result<Value> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let c = self.connection(&m.connection_id).await?;
        if c.status != "connected" || m.product(&c) != PlaidProduct::Transactions {
            return Err(SyncError::Attention);
        }
        if refresh
            || m.staged
                .as_ref()
                .is_none_or(|s| (Utc::now() - s.fetched_at).num_minutes() > 30)
        {
            m.staged = Some(self.fetch_bank(&m, &c).await?);
            admit()?;
            m.status = "review".into();
            self.repository.put_mappings(vec![m.clone()]).await?;
        }
        let stage = m.staged.as_ref().ok_or(SyncError::NotReady)?;
        let mut warnings = Vec::new();
        for tx in &stage.records {
            if !tx.pending
                && m.cutover_date.is_none_or(|d| tx.date >= d)
                && ingest::decision(tx, &m).is_err()
            {
                warnings.push(tx.transaction_id.clone());
            }
        }
        let mut removed_ids = stage.removed.clone();
        removed_ids.extend(ingest::cutover_removals(&m, stage)?);
        let mut seen_removed = HashSet::new();
        removed_ids.retain(|id| seen_removed.insert(id.clone()));
        let removed_records: Vec<_> = removed_ids
            .iter()
            .filter_map(|id| {
                m.source_history.get(id).map(|record| {
                    json!({"activityId":ingest::stable_id(&m.id,id),
                "accountId":m.account_id,"name":record.get("name"),"date":record.get("date"),
                "amount":record.get("amount"),"currency":m.currency})
                })
            })
            .collect();
        Ok(
            json!({"removedRecords":removed_records,"mappingId":m.id,"digest":stage.digest,"initial":m.cursor.is_none() && m.cutover_date.is_none(),"cutoverDate":m.cutover_date,"currency":m.currency,"balance":stage.balance,
            "suggestedOpening":if m.cursor.is_none() && m.cutover_date.is_none() { Some(ingest::suggested_opening(&m,stage)?.to_string()) } else { None },
            "records":stage.records.iter().filter(|t| m.cutover_date.is_none_or(|d| t.date >= d)).collect::<Vec<_>>(),"decisions":m.decisions,"reviewRequired":warnings,"removedCount":removed_ids.len(),"fetchedAt":stage.fetched_at}),
        )
    }

    pub async fn decide(
        &self,
        id: &str,
        record: &str,
        kind: &str,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let s = m.staged.as_mut().ok_or(SyncError::Review)?;
        if !matches!(kind, "income" | "expense" | "refund" | "external")
            || !s
                .records
                .iter()
                .any(|t| t.transaction_id == record && !t.pending)
        {
            return Err(SyncError::Review);
        }
        let group = m.decisions.get(record).and_then(|d| d.group.clone());
        let mut changed = Vec::new();
        if let Some(group) = group {
            for mut peer in self.repository.mappings().await? {
                let records: Vec<_> = peer
                    .decisions
                    .iter()
                    .filter(|(_, d)| d.group.as_deref() == Some(&group))
                    .map(|(id, _)| id.clone())
                    .collect();
                if records.is_empty() {
                    continue;
                }
                // Review undo is for uncommitted choices. Changing a posted
                // pair needs reconciliation of both ledger legs, not a split
                // metadata edit that could orphan an existing transfer.
                if records
                    .iter()
                    .any(|id| peer.source_history.get(id).is_some())
                {
                    return Err(SyncError::Review);
                }
                let stage = peer.staged.as_mut().ok_or(SyncError::Review)?;
                for id in records {
                    peer.decisions.remove(&id);
                }
                stage.approved = false;
                stage.digest = ingest::digest(stage, &peer.decisions)?;
                peer.status = "review".into();
                if peer.id != m.id {
                    changed.push(peer);
                }
            }
        }
        m.decisions.insert(
            record.into(),
            Decision {
                kind: kind.into(),
                group: None,
            },
        );
        s.approved = false;
        s.digest = ingest::digest(s, &m.decisions)?;
        m.status = "review".into();
        changed.push(m);
        admit()?;
        self.repository.put_mappings(changed).await
    }

    pub async fn pair(
        &self,
        id: &str,
        record: &str,
        other: &str,
        other_record: &str,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        let mut a = self.mapping(id).await?;
        let mut b = self.mapping(other).await?;
        if a.id == b.id || a.currency != b.currency {
            return Err(SyncError::Review);
        }
        if a.decisions.get(record).is_some_and(|d| d.group.is_some())
            || b.decisions
                .get(other_record)
                .is_some_and(|d| d.group.is_some())
        {
            return Err(SyncError::Review);
        }
        let tx = a
            .staged
            .as_ref()
            .and_then(|s| {
                s.records
                    .iter()
                    .find(|t| t.transaction_id == record && !t.pending)
            })
            .ok_or(SyncError::Review)?;
        let counterpart = b
            .staged
            .as_ref()
            .and_then(|s| {
                s.records
                    .iter()
                    .find(|t| t.transaction_id == other_record && !t.pending)
            })
            .ok_or(SyncError::Review)?;
        if tx.amount == Decimal::ZERO
            || tx.amount.checked_add(counterpart.amount) != Some(Decimal::ZERO)
        {
            return Err(SyncError::Review);
        }
        let group = uuid::Uuid::new_v4().to_string();
        for (m, record) in [(&mut a, record), (&mut b, other_record)] {
            m.decisions.insert(
                record.into(),
                Decision {
                    kind: "internal".into(),
                    group: Some(group.clone()),
                },
            );
            let s = m.staged.as_mut().ok_or(SyncError::Review)?;
            s.approved = false;
            s.digest = ingest::digest(s, &m.decisions)?;
        }
        self.repository.put_mappings(vec![a, b]).await
    }

    pub async fn approve(
        &self,
        id: &str,
        digest: &str,
        opening: Option<String>,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let s = m.staged.as_mut().ok_or(SyncError::Review)?;
        if s.digest != digest {
            return Err(SyncError::Changed);
        }
        s.approved = true;
        s.opening_balance = opening;
        ingest::bank_commit(m.clone())?;
        m.status = "ready".into();
        self.repository.put_mappings(vec![m]).await?;
        self.apply_ready(true, admit).await
    }

    async fn apply_ready(&self, explicit: bool, admit: Admission<'_>) -> state::Result<()> {
        let mappings = self.repository.mappings().await?;
        let mut ready = Vec::new();
        for m in mappings.iter().filter(|m| {
            m.staged.as_ref().is_some_and(|s| s.approved) && (explicit || m.status == "ready")
        }) {
            let c = match self.connection(&m.connection_id).await {
                Ok(c) => c,
                Err(SyncError::Access) => continue,
                Err(error) => return Err(error),
            };
            if c.status == "connected" {
                if let Ok(batch) = ingest::bank_commit(m.clone()) {
                    ready.push(batch);
                }
            }
        }
        // Group explicitly paired accounts into connected components; unrelated
        // accounts remain independently committable when another Item needs review.
        while let Some(first) = ready.pop() {
            let mut component = vec![first];
            loop {
                let groups: HashSet<_> = component
                    .iter()
                    .flat_map(|b| {
                        b.activities
                            .iter()
                            .filter_map(|a| a.source_group_id.clone())
                    })
                    .collect();
                let Some(index) = ready.iter().position(|b| {
                    b.activities.iter().any(|a| {
                        a.source_group_id
                            .as_ref()
                            .is_some_and(|g| groups.contains(g))
                    })
                }) else {
                    break;
                };
                component.push(ready.swap_remove(index));
            }
            let ids: Vec<_> = component
                .iter()
                .map(|b| b.mapping.account_id.clone())
                .collect();
            let currencies = component
                .iter()
                .map(|b| b.mapping.currency.clone())
                .collect();
            let earliest = component.iter().filter_map(|b| b.earliest).min();
            let mut failed_maps: Vec<_> = component.iter().map(|b| b.mapping.clone()).collect();
            // Keep the pre-commit staging/cursor when a transaction rolls back.
            for m in &mut failed_maps {
                *m = mappings
                    .iter()
                    .find(|old| old.id == m.id)
                    .expect("ready batches are derived from this immutable mapping list")
                    .clone();
            }
            admit()?;
            match self.repository.commit_banks(component).await {
                Ok(()) => self.events.emit(DomainEvent::activities_changed(
                    ids,
                    vec![],
                    currencies,
                    earliest,
                )),
                Err(SyncError::Review) => {
                    for m in &mut failed_maps {
                        m.status = "review".into();
                        m.last_error = Some(SyncError::Review.to_string());
                    }
                    self.repository.put_mappings(failed_maps).await?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn sync(&self, daily: bool, admit: Admission<'_>) -> state::Result<Value> {
        admit()?;
        let mut failures = Vec::new();
        let mut successes = 0;
        for mut m in self.repository.mappings().await? {
            let c = match self.connection(&m.connection_id).await {
                Ok(c) => c,
                Err(SyncError::Access) => continue,
                Err(error) => return Err(error),
            };
            if c.status != "connected"
                || (daily
                    && m.last_success
                        .is_some_and(|t| (Utc::now() - t).num_hours() < 24))
            {
                continue;
            }
            if m.product(&c) == PlaidProduct::Investments {
                if m.status == "initial_review"
                    || (daily && matches!(m.status.as_str(), "review" | "attention"))
                {
                    continue;
                }
                match self.sync_holdings(&mut m, &c, admit).await {
                    Ok(()) => successes += 1,
                    Err(e) => {
                        if e == SyncError::Attention {
                            m.status = "attention".into();
                        } else if matches!(e, SyncError::Review | SyncError::Changed) {
                            m.status = "review".into();
                        }
                        m.last_error = Some(e.to_string());
                        admit()?;
                        self.repository.put_mappings(vec![m.clone()]).await?;
                        failures.push(json!({"mappingId":m.id,"error":e.to_string()}));
                    }
                }
            } else {
                if matches!(m.status.as_str(), "review" | "initial_review" | "attention") {
                    continue;
                }
                match self.fetch_bank(&m, &c).await {
                    Ok(mut stage) => {
                        stage.approved = m.cursor.is_some()
                            && stage
                                .records
                                .iter()
                                .filter(|t| {
                                    !t.pending && m.cutover_date.is_none_or(|d| t.date >= d)
                                })
                                .all(|t| ingest::decision(t, &m).is_ok());
                        m.status = if stage.approved { "ready" } else { "review" }.into();
                        m.staged = Some(stage);
                        admit()?;
                        self.repository.put_mappings(vec![m]).await?;
                    }
                    Err(e) => {
                        if e == SyncError::Attention {
                            m.status = "attention".into();
                        } else if matches!(e, SyncError::Review | SyncError::Changed) {
                            m.status = "review".into();
                        }
                        m.last_error = Some(e.to_string());
                        admit()?;
                        self.repository.put_mappings(vec![m.clone()]).await?;
                        failures.push(json!({"mappingId":m.id,"error":e.to_string()}));
                    }
                }
            }
        }
        self.apply_ready(false, admit).await?;
        Ok(json!({"holdingsSynced":successes,"failures":failures}))
    }

    pub async fn preview_holdings(&self, id: &str, admit: Admission<'_>) -> state::Result<Value> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let c = self.connection(&m.connection_id).await?;
        if m.product(&c) != PlaidProduct::Investments {
            return Err(SyncError::Review);
        }
        let stage = self.prepare_holdings(&m, &c).await?;
        admit()?;
        let value = json!({"mappingId":m.id,"digest":stage.digest,"currency":m.currency,
            "holdings":stage.response.holdings,"securities":stage.response.securities,
            "transactionCount":stage.history.len(),"fetchedAt":stage.fetched_at});
        m.holdings_staged = Some(stage);
        self.repository.put_mappings(vec![m]).await?;
        Ok(value)
    }

    pub async fn approve_holdings(
        &self,
        id: &str,
        digest: &str,
        allow_opaque: bool,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        admit()?;
        let mut m = self.mapping(id).await?;
        let c = self.connection(&m.connection_id).await?;
        if m.product(&c) != PlaidProduct::Investments || c.status != "connected" {
            return Err(SyncError::Review);
        }
        let stage = m.holdings_staged.clone().ok_or(SyncError::Review)?;
        if stage.digest != digest || (Utc::now() - stage.fetched_at).num_minutes() > 30 {
            return Err(SyncError::Changed);
        }
        if allow_opaque {
            for security in &stage.response.securities {
                m.opaque_security_ids.insert(security.security_id.clone());
            }
        }
        self.apply_holdings(&mut m, stage, admit).await
    }

    async fn sync_holdings(
        &self,
        m: &mut Mapping,
        c: &Connection,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        let stage = self.prepare_holdings(m, c).await?;
        self.apply_holdings(m, stage, admit).await
    }

    async fn prepare_holdings(&self, m: &Mapping, c: &Connection) -> state::Result<HoldingsStage> {
        if c.status != "connected" || m.product(c) != PlaidProduct::Investments {
            return Err(SyncError::Attention);
        }
        let token = self.access(c)?;
        let response = self.api.holdings(&token, &m.external_id).await?;
        let account = response
            .accounts
            .iter()
            .find(|a| a.account_id == m.external_id && eligible(c, a))
            .ok_or(SyncError::Review)?;
        if response.item.item_id != c.item_id.clone().ok_or(SyncError::Review)?
            || response.item.institution_id.as_deref() != Some(&c.institution_id)
            || ingest::currency(account)? != m.currency
            || (account.account_type == "credit") != m.credit_card
        {
            return Err(SyncError::Review);
        }
        let now = Utc::now();
        let today = now.date_naive();
        let start = today
            .checked_sub_days(chrono::Days::new(730))
            .ok_or(SyncError::Review)?;
        let mut history = Vec::new();
        let mut offset = 0;
        let mut expected_total = None;
        let mut seen = HashSet::new();
        for _ in 0..100 {
            let page = self
                .api
                .investments(&token, &m.external_id, start, today, offset)
                .await?;
            if page.item.item_id != response.item.item_id
                || page.total_investment_transactions as usize > ingest::MAX_RECORDS
                || expected_total.is_some_and(|n| n != page.total_investment_transactions)
            {
                return Err(SyncError::Review);
            }
            expected_total = Some(page.total_investment_transactions);
            let count = page.investment_transactions.len();
            for tx in page.investment_transactions {
                if tx.account_id != m.external_id
                    || tx.date < start
                    || tx.date > today
                    || !seen.insert(tx.investment_transaction_id.clone())
                {
                    return Err(SyncError::Review);
                }
                history.push(tx);
            }
            offset = offset.checked_add(count as u32).ok_or(SyncError::Review)?;
            if offset == expected_total.ok_or(SyncError::Review)? {
                break;
            }
            if count == 0 || offset > expected_total.ok_or(SyncError::Review)? {
                return Err(SyncError::Review);
            }
        }
        if history.len() != expected_total.ok_or(SyncError::Review)? as usize {
            return Err(SyncError::Review);
        }
        let mut stage = HoldingsStage {
            response,
            history,
            fetched_at: now,
            digest: String::new(),
        };
        let bytes = serde_json::to_vec(&stage).map_err(|_| SyncError::Storage)?;
        if bytes.len() > ingest::MAX_STAGE_BYTES {
            return Err(SyncError::Review);
        }
        use sha2::Digest;
        stage.digest = hex::encode(sha2::Sha256::digest(bytes));
        Ok(stage)
    }

    async fn apply_holdings(
        &self,
        m: &mut Mapping,
        stage: HoldingsStage,
        admit: Admission<'_>,
    ) -> state::Result<()> {
        let response = stage.response;
        let history = stage.history;
        let now = stage.fetched_at;
        let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
            &self.timezone.read().map_err(|_| SyncError::Storage)?,
        );
        let today = now.with_timezone(&tz).date_naive();
        let account = response
            .accounts
            .iter()
            .find(|a| a.account_id == m.external_id)
            .ok_or(SyncError::Review)?;
        let securities: HashMap<_, _> = response
            .securities
            .iter()
            .map(|s| (s.security_id.as_str(), s))
            .collect();
        if securities.len() != response.securities.len() {
            return Err(SyncError::Review);
        }
        let mut specs = Vec::new();
        let mut spec_keys = HashMap::new();
        let mut total_value = Decimal::ZERO;
        let mut cash = Decimal::ZERO;
        for h in &response.holdings {
            if h.account_id != m.external_id
                || h.iso_currency_code.as_deref() != Some(&m.currency)
                || h.unofficial_currency_code.is_some()
            {
                return Err(SyncError::Review);
            }
            total_value = total_value
                .checked_add(h.institution_value)
                .ok_or(SyncError::Review)?;
            let security = securities
                .get(h.security_id.as_str())
                .ok_or(SyncError::Review)?;
            if security.security_type.as_deref() == Some("cash") {
                cash = cash
                    .checked_add(h.institution_value)
                    .ok_or(SyncError::Review)?;
                continue;
            }
            if h.quantity == Decimal::ZERO && h.institution_value != Decimal::ZERO {
                return Err(SyncError::Review);
            }
            if h.quantity == Decimal::ZERO {
                continue;
            }
            let ordinary = matches!(
                security.security_type.as_deref(),
                Some("equity" | "etf" | "mutual fund")
            ) && security
                .ticker_symbol
                .as_ref()
                .is_some_and(|s| !s.is_empty());
            let spec = if ordinary {
                let symbol = security.ticker_symbol.clone().ok_or(SyncError::Review)?;
                AssetSpec::market_instrument(
                    symbol.clone(),
                    symbol,
                    None,
                    InstrumentType::Equity,
                    m.currency.clone(),
                )
            } else {
                if !m.opaque_security_ids.contains(&security.security_id) {
                    return Err(SyncError::Review);
                }
                AssetSpec {
                    id: Some(ingest::stable_id(&m.connection_id, &security.security_id)),
                    display_code: security.ticker_symbol.clone(),
                    instrument_symbol: None,
                    instrument_exchange_mic: None,
                    instrument_type: None,
                    quote_ccy: m.currency.clone(),
                    requested_quote_ccy: None,
                    kind: wealthfolio_core::assets::AssetKind::Investment,
                    quote_mode: Some(wealthfolio_core::assets::QuoteMode::Manual),
                    name: security.name.clone(),
                    provider_config: None,
                    provider_id: None,
                    provider_symbol: None,
                    metadata: None,
                }
            };
            let key = spec
                .id
                .clone()
                .or_else(|| spec.instrument_key())
                .ok_or(SyncError::Review)?;
            spec_keys.insert(h.security_id.clone(), key);
            specs.push(spec);
        }
        // Plaid investment current balance is total account value, not cash.
        // Derive residual cash from total less every reported holding; never add
        // available buying power (which can include margin) to account value.
        let total = account.balances.current.ok_or(SyncError::Review)?;
        cash = cash
            .checked_add(total.checked_sub(total_value).ok_or(SyncError::Review)?)
            .ok_or(SyncError::Review)?;
        admit()?;
        let ensured = self
            .assets
            .ensure_assets(specs, self.activities.as_ref())
            .await
            .map_err(|_| SyncError::Storage)?;
        admit()?;
        let mut positions = HashMap::new();
        let mut quotes = Vec::new();
        let mut basis = Decimal::ZERO;
        let mut basis_by_security = serde_json::Map::new();
        for h in &response.holdings {
            let Some(key) = spec_keys.get(&h.security_id) else {
                continue;
            };
            let id = ensured
                .input_to_asset_id
                .get(key)
                .ok_or(SyncError::Review)?
                .clone();
            if positions.contains_key(&id) {
                return Err(SyncError::Review);
            }
            let mut p = Position::new(m.account_id.clone(), id.clone(), m.currency.clone(), now);
            p.quantity = h.quantity;
            let previous = m
                .source_history
                .pointer("/basisBySecurity")
                .and_then(|v| v.get(&h.security_id));
            let retained = previous
                .filter(|v| {
                    v.get("assetId").and_then(Value::as_str) == Some(id.as_str())
                        && v.get("currency").and_then(Value::as_str) == Some(m.currency.as_str())
                        && v.get("quantity")
                            .and_then(Value::as_str)
                            .and_then(|q| q.parse::<Decimal>().ok())
                            == Some(h.quantity)
                })
                .and_then(|v| v.get("basis"))
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<Decimal>().ok());
            let known_basis = h.cost_basis.or(retained);
            p.cost_basis_status = Some(if known_basis.is_some() {
                wealthfolio_core::portfolio::economic_events::BasisStatus::Complete
            } else {
                wealthfolio_core::portfolio::economic_events::BasisStatus::Unknown
            });
            p.total_cost_basis = known_basis.unwrap_or(Decimal::ZERO);
            basis_by_security.insert(h.security_id.clone(), json!({"assetId":id,"currency":m.currency,
                "quantity":h.quantity.to_string(),"basis":known_basis.map(|b|b.to_string()),
                "status":if h.cost_basis.is_some(){"provider"}else if retained.is_some(){"retained"}else{"unknown"}}));
            p.average_cost = p
                .total_cost_basis
                .checked_div(h.quantity)
                .ok_or(SyncError::Review)?;
            basis = basis
                .checked_add(p.total_cost_basis)
                .ok_or(SyncError::Review)?;
            let price = h
                .institution_value
                .checked_div(h.quantity)
                .ok_or(SyncError::Review)?;
            if price < Decimal::ZERO {
                return Err(SyncError::Review);
            }
            positions.insert(id.clone(), p);
            let timestamp = ingest::holdings_quote_timestamp(h, now, &tz)?;
            quotes.push(wealthfolio_core::quotes::Quote {
                id: format!("{}_{}_BROKER", id, timestamp.date_naive()),
                asset_id: id,
                timestamp,
                open: price,
                high: price,
                low: price,
                close: price,
                adjclose: price,
                volume: Decimal::ZERO,
                currency: m.currency.clone(),
                data_source: "BROKER".into(),
                created_at: now,
                notes: None,
            });
        }
        let snapshot = AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id(&m.account_id, today),
            account_id: m.account_id.clone(),
            snapshot_date: today,
            currency: m.currency.clone(),
            positions,
            cash_balances: HashMap::from([(m.currency.clone(), cash)]),
            cost_basis: basis,
            cash_total_account_currency: cash,
            source: SnapshotSource::BrokerImported,
            calculated_at: now.naive_utc(),
            ..Default::default()
        };
        m.source_history = json!({"transactions":history,"securities":response.securities,"holdings":response.holdings,"asOf":now,"performanceHistoryComplete":false,"basisBySecurity":basis_by_security});
        if serde_json::to_vec(&m.source_history)
            .map_err(|_| SyncError::Storage)?
            .len()
            > ingest::MAX_STAGE_BYTES
        {
            return Err(SyncError::Review);
        }
        m.holdings_staged = None;
        m.last_error = None;
        m.last_success = Some(Utc::now());
        m.status = "idle".into();
        admit()?;
        self.repository
            .commit_holdings(m.clone(), snapshot, quotes)
            .await?;
        self.events.emit(DomainEvent::holdings_changed(
            vec![m.account_id.clone()],
            ensured.assets.keys().cloned().collect(),
        ));
        Ok(())
    }
}
