use super::*;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
};
use wealthfolio_connect::plaid::{
    service::{PlaidApi, PlaidService},
    *,
};
use wealthfolio_core::{
    assets::AssetService,
    events::{DomainEvent, DomainEventSink},
    quotes::QuoteService,
    secrets::SecretStore,
};

#[derive(Default)]
struct Vault {
    values: Mutex<HashMap<String, String>>,
    fail: Mutex<Option<&'static str>>,
}
impl SecretStore for Vault {
    fn set_secret(&self, key: &str, value: &str) -> crate::Result<()> {
        if *self.fail.lock().unwrap() == Some("set") {
            return Err(Error::Secret("fixture failure".into()));
        }
        self.values.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn get_secret(&self, key: &str) -> crate::Result<Option<String>> {
        if *self.fail.lock().unwrap() == Some("get") {
            return Err(Error::Secret("fixture failure".into()));
        }
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn delete_secret(&self, key: &str) -> crate::Result<()> {
        if *self.fail.lock().unwrap() == Some("delete") {
            return Err(Error::Secret("fixture failure".into()));
        }
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}
#[derive(Default)]
struct Events(Mutex<Vec<DomainEvent>>);
impl DomainEventSink for Events {
    fn emit(&self, event: DomainEvent) {
        self.0.lock().unwrap().push(event);
    }
}

struct ApiState {
    calls: Vec<&'static str>,
    transaction_requests: Vec<(String, Option<String>)>,
    link_transactions: Vec<bool>,
    accounts: Value,
    bank: VecDeque<std::result::Result<Value, PlaidError>>,
    holdings: Option<Value>,
    investments: VecDeque<Value>,
    errors: HashMap<&'static str, VecDeque<PlaidError>>,
    revoke_admission: Option<Arc<AtomicBool>>,
    result_public: bool,
}
struct Api(Mutex<ApiState>);
impl Api {
    fn new() -> Self {
        Self(Mutex::new(ApiState {
            calls: vec![],
            transaction_requests: vec![],
            link_transactions: vec![],
            accounts: json!({"accounts":[account("depository")],"item":item()}),
            bank: VecDeque::new(),
            holdings: None,
            investments: VecDeque::new(),
            errors: HashMap::new(),
            revoke_admission: None,
            result_public: true,
        }))
    }
    fn check(s: &mut ApiState, method: &'static str) -> std::result::Result<(), PlaidError> {
        s.calls.push(method);
        if let Some(e) = s.errors.get_mut(method).and_then(VecDeque::pop_front) {
            return Err(e);
        }
        Ok(())
    }
    fn fail(&self, method: &'static str, error: PlaidError) {
        self.0
            .lock()
            .unwrap()
            .errors
            .entry(method)
            .or_default()
            .push_back(error);
    }
    fn calls(&self, method: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|m| **m == method)
            .count()
    }
    fn page(&self, records: Value) {
        self.0.lock().unwrap().bank.push_back(Ok(json!({"added":records,"modified":[],"removed":[],"next_cursor":"cursor-1","has_more":false,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"})));
    }
}
fn item() -> Value {
    json!({"item_id":"item","institution_id":"ins_1","available_products":["transactions","investments"],"billed_products":["transactions","investments"]})
}
fn account(kind: &str) -> Value {
    json!({"account_id":"external","name":"Synthetic","type":kind,"subtype":if kind=="credit"{"credit card"}else if kind=="investment"{"brokerage"}else{"checking"},"balances":{"current":250,"available":9999,"iso_currency_code":"USD"}})
}
fn transaction(id: &str, amount: &str, primary: &str) -> Value {
    json!({"account_id":"external","transaction_id":id,"pending":false,"date":"2026-11-01","amount":amount,"iso_currency_code":"USD","name":"Synthetic purchase","personal_finance_category":{"primary":primary}})
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
#[async_trait]
impl PlaidApi for Api {
    async fn link(
        &self,
        _: &str,
        _: PlaidProduct,
        _: &str,
        _: Option<&AccessToken>,
        include_transactions: bool,
    ) -> std::result::Result<HostedLink, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "link")?;
        s.link_transactions.push(include_transactions);
        Ok(HostedLink {
            token: LinkCredential::new(PlaidEnvironment::Sandbox, "link".into())?,
            url: "https://secure.plaid.com/hl/test".into(),
            expiration: Utc::now() + Duration::minutes(30),
        })
    }
    async fn result(
        &self,
        _: &LinkCredential,
        update: bool,
    ) -> std::result::Result<Option<PublicToken>, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "result")?;
        if update || !s.result_public {
            Ok(None)
        } else {
            Ok(Some(PublicToken::new(
                PlaidEnvironment::Sandbox,
                "public".into(),
            )?))
        }
    }
    async fn exchange(&self, _: &PublicToken) -> std::result::Result<TokenExchange, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "exchange")?;
        if let Some(flag) = &s.revoke_admission {
            flag.store(false, Ordering::SeqCst);
        }
        Ok(TokenExchange {
            item_id: "item".into(),
            access_token: AccessToken::new(PlaidEnvironment::Sandbox, "access".into())?,
        })
    }
    async fn accounts(&self, _: &AccessToken) -> std::result::Result<AccountsResponse, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "accounts")?;
        Ok(parse(s.accounts.clone()))
    }
    async fn transactions(
        &self,
        _: &AccessToken,
        account: &str,
        cursor: Option<&str>,
    ) -> std::result::Result<TransactionsPage, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "transactions")?;
        s.transaction_requests
            .push((account.into(), cursor.map(Into::into)));
        let page=s.bank.pop_front().unwrap_or_else(||Ok(json!({"added":[],"modified":[],"removed":[],"next_cursor":"cursor-2","has_more":false,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"})))?;
        Ok(parse(page))
    }
    async fn holdings(
        &self,
        _: &AccessToken,
        _: &str,
    ) -> std::result::Result<HoldingsResponse, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "holdings")?;
        Ok(parse(s.holdings.clone().unwrap_or_else(||json!({"accounts":[account("investment")],"item":item(),"holdings":[],"securities":[]}))))
    }
    async fn investments(
        &self,
        _: &AccessToken,
        _: &str,
        _: chrono::NaiveDate,
        _: chrono::NaiveDate,
        _: u32,
    ) -> std::result::Result<InvestmentsPage, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "investments")?;
        Ok(parse(s.investments.pop_front().unwrap_or_else(||json!({"accounts":[account("investment")],"item":item(),"investment_transactions":[],"securities":[],"total_investment_transactions":0}))))
    }
    async fn remove(&self, _: &AccessToken) -> std::result::Result<(), PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "remove")
    }
}

struct Rig {
    service: PlaidService,
    quotes: Arc<dyn wealthfolio_core::quotes::QuoteServiceTrait>,
    repo: Arc<PlaidRepositorySqlite>,
    api: Arc<Api>,
    vault: Arc<Vault>,
    events: Arc<Events>,
    pool: Arc<crate::db::DbPool>,
    writer: WriteHandle,
    _dir: tempfile::TempDir,
}
impl Rig {
    async fn new() -> Self {
        std::env::set_var("CONNECT_API_URL", "http://test.local");
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::DbAccess::plaintext(dir.path().join("app.db").to_string_lossy());
        db.prepare().unwrap();
        db.run_migrations().unwrap();
        let pool = db.create_pool().unwrap();
        let writer = crate::db::write_actor::spawn_writer(pool.as_ref().clone()).unwrap();
        let assets = Arc::new(crate::assets::AssetRepository::new(
            pool.clone(),
            writer.clone(),
        ));
        let activities = Arc::new(crate::activities::ActivityRepository::new(
            pool.clone(),
            writer.clone(),
        ));
        let market = Arc::new(crate::market_data::MarketDataRepository::new(
            pool.clone(),
            writer.clone(),
        ));
        let sync = Arc::new(crate::market_data::QuoteSyncStateRepository::new(
            pool.clone(),
            writer.clone(),
        ));
        let vault = Arc::new(Vault::default());
        let quotes = Arc::new(
            QuoteService::new_with_custom_provider(
                market.clone(),
                sync,
                market,
                assets.clone(),
                activities.clone(),
                vault.clone(),
                None,
            )
            .await
            .unwrap(),
        );
        let assets = Arc::new(AssetService::new(assets, quotes.clone()).unwrap());
        let repo = Arc::new(PlaidRepositorySqlite::new(writer.clone()));
        let api = Arc::new(Api::new());
        let events = Arc::new(Events::default());
        let service = PlaidService {
            repository: repo.clone(),
            api: api.clone(),
            secrets: vault.clone(),
            environment: PlaidEnvironment::Sandbox,
            institutions: BTreeMap::from([
                ("chase".into(), "ins_1".into()),
                ("merrill_edge".into(), "ins_1".into()),
                ("schwab".into(), "ins_1".into()),
                ("amex".into(), "ins_1".into()),
            ]),
            assets,
            activities,
            events: events.clone(),
            timezone: Arc::new(RwLock::new("America/Los_Angeles".into())),
            base_currency: Arc::new(RwLock::new("USD".into())),
            fx: Arc::new(wealthfolio_core::fx::FxService::new(Arc::new(
                crate::fx::FxRepository::new(pool.clone(), writer.clone()),
            ))),
        };
        Self {
            service,
            quotes,
            repo,
            api,
            vault,
            events,
            pool,
            writer,
            _dir: dir,
        }
    }
    fn connection(&self, product: PlaidProduct) -> super::PlaidConnection {
        super::PlaidConnection {
            id: uuid::Uuid::new_v4().to_string(),
            owner: "owner".into(),
            environment: PlaidEnvironment::Sandbox,
            product,
            institution: if product == PlaidProduct::Transactions {
                "chase"
            } else {
                "merrill_edge"
            }
            .into(),
            institution_id: "ins_1".into(),
            item_id: Some("item".into()),
            status: "connected".into(),
            expires_at: Utc::now() + Duration::minutes(30),
            consent_expires_at: None,
            update_mode: false,
        }
    }
    async fn seed(&self, product: PlaidProduct) -> (super::PlaidConnection, Mapping) {
        self.seed_kind(product, false).await
    }
    async fn seed_kind(
        &self,
        product: PlaidProduct,
        credit_card: bool,
    ) -> (super::PlaidConnection, Mapping) {
        let c = self.connection(product);
        self.repo.put_connection(c.clone(), None).await.unwrap();
        self.vault
            .set_secret(&secret_key(&c, "access").unwrap(), "access")
            .unwrap();
        let local = uuid::Uuid::new_v4().to_string();
        let l = local.clone();
        self.writer.exec(move|conn|{diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Synthetic',?,'USD',0,1,0,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(l).bind::<Text,_>(if credit_card {"CREDIT_CARD"} else if product==PlaidProduct::Investments{"SECURITIES"}else{"CASH"}).bind::<Text,_>(if product==PlaidProduct::Investments{"HOLDINGS"}else{"TRANSACTIONS"}).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
        let m = Mapping {
            id: uuid::Uuid::new_v4().to_string(),
            connection_id: c.id.clone(),
            external_id: "external".into(),
            account_id: local,
            currency: "USD".into(),
            credit_card,
            product: None,
            cutover_date: None,
            cursor: None,
            last_success: None,
            status: "initial_review".into(),
            last_error: None,
            decisions: BTreeMap::new(),
            staged: None,
            source_history: json!({}),
            holdings_staged: None,
            opaque_security_ids: Default::default(),
            cash_security_ids: Default::default(),
            investment_ledger: None,
        };
        self.repo.add_mapping(m.clone(), product).await.unwrap();
        (c, m)
    }
    fn count(&self, table: &str) -> i64 {
        #[derive(QueryableByName)]
        struct N {
            #[diesel(sql_type=diesel::sql_types::BigInt)]
            n: i64,
        }
        diesel::sql_query(format!("SELECT count(*) AS n FROM {table}"))
            .get_result::<N>(&mut self.pool.get().unwrap())
            .unwrap()
            .n
    }
    async fn preview(&self, m: &Mapping, records: Value) -> Value {
        self.api.page(records);
        self.service.preview(&m.id, true, &|| Ok(())).await.unwrap()
    }
    async fn current(&self, m: &Mapping) -> Mapping {
        self.repo
            .mappings()
            .await
            .unwrap()
            .into_iter()
            .find(|x| x.id == m.id)
            .unwrap()
    }
}

#[tokio::test]
async fn bank_import_commits_reviewed_opening_cursor_and_history_exactly_once() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    let preview = r
        .preview(&m, json!([transaction("one", "10.25", "FOOD_AND_DRINK")]))
        .await;
    assert_eq!(r.count("activities"), 0);
    assert_eq!(preview["suggestedOpening"], "260.25");
    r.service
        .approve(
            &m.id,
            preview["digest"].as_str().unwrap(),
            Some("260.25".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
    assert_eq!(r.count("import_runs"), 1);
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("cursor-1"));
    assert_eq!(r.events.0.lock().unwrap().len(), 1);
    assert!(matches!(
        r.service.approve(&m.id, "old", None, &|| Ok(())).await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("activities"), 2);
}

#[tokio::test]
async fn unknown_transfer_blocks_without_advancing_and_manual_external_decision_is_reviewed() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    let p = r
        .preview(&m, json!([transaction("one", "10", "TRANSFER_OUT")]))
        .await;
    assert_eq!(p["reviewRequired"].as_array().unwrap().len(), 1);
    assert!(r
        .service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("260".into()),
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(r.count("activities"), 0);
    r.service
        .decide(&m.id, "one", "external", &|| Ok(()))
        .await
        .unwrap();
    assert!(matches!(
        r.service
            .approve(
                &m.id,
                p["digest"].as_str().unwrap(),
                Some("260".into()),
                &|| Ok(())
            )
            .await,
        Err(SyncError::Changed)
    ));
    let p = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("260".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert!(r.current(&m).await.cursor.is_some());
}

#[tokio::test]
async fn durable_exchange_claim_prevents_retry_and_validation_resumes_after_outage() {
    let r = Rig::new().await;
    let started = r
        .service
        .start("owner", "chase", None, false, &|| Ok(()))
        .await
        .unwrap();
    let id = started["flowId"].as_str().unwrap();
    r.api.fail("accounts", PlaidError::Unavailable);
    assert!(r.service.finish("owner", id, &|| Ok(())).await.is_err());
    assert_eq!(r.repo.connections().await.unwrap()[0].status, "validating");
    r.service
        .finish("new-admitted-profile-session", id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.api.calls("exchange"), 1);
    assert_eq!(r.repo.connections().await.unwrap()[0].status, "connected");
    let r = Rig::new().await;
    let flow = r
        .service
        .start("owner", "chase", None, false, &|| Ok(()))
        .await
        .unwrap();
    let id = flow["flowId"].as_str().unwrap();
    r.api.fail("exchange", PlaidError::Unavailable);
    assert!(r.service.finish("owner", id, &|| Ok(())).await.is_err());
    assert!(matches!(
        r.service.finish("owner", id, &|| Ok(())).await,
        Err(SyncError::Attention)
    ));
    assert_eq!(r.api.calls("exchange"), 1);
}

#[tokio::test]
async fn admission_loss_after_exchange_quarantines_cleanup_only_token() {
    let r = Rig::new().await;
    let flow = r
        .service
        .start("owner", "chase", None, false, &|| Ok(()))
        .await
        .unwrap();
    let active = Arc::new(AtomicBool::new(true));
    r.api.0.lock().unwrap().revoke_admission = Some(active.clone());
    r.api.fail("remove", PlaidError::Unavailable);
    let admit = || {
        if active.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(SyncError::Access)
        }
    };
    assert!(matches!(
        r.service
            .finish("owner", flow["flowId"].as_str().unwrap(), &admit)
            .await,
        Err(SyncError::Access)
    ));
    let c = r.repo.connections().await.unwrap().remove(0);
    assert_eq!(c.status, "pending_revocation");
    assert!(r
        .vault
        .get_secret(&secret_key(&c, "access").unwrap())
        .unwrap()
        .is_none());
    assert!(r
        .vault
        .get_secret(&secret_key(&c, "revocation").unwrap())
        .unwrap()
        .is_some());
    r.service.disconnect(&c.id, &|| Ok(())).await.unwrap();
    assert_eq!(
        r.repo.connections().await.unwrap()[0].status,
        "disconnected"
    );
    assert!(r.vault.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn removed_records_win_consistently_over_added_records() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    r.api.0.lock().unwrap().bank.push_back(Ok(json!({"added":[transaction("one","10","FOOD_AND_DRINK")],"modified":[],"removed":[{"account_id":"external","transaction_id":"one"}],"has_more":false,"next_cursor":"next","transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"})));
    let p = r.service.preview(&m.id, true, &|| Ok(())).await.unwrap();
    assert!(p["records"].as_array().unwrap().is_empty());
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("250".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 1);
    let m = r.current(&m).await;
    assert_eq!(m.cursor.as_deref(), Some("next"));
    assert!(m.source_history.get("one").is_none());
}

#[tokio::test]
async fn rollback_keeps_all_financial_rows_import_runs_and_cursor_unchanged() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    let p = r
        .preview(&m, json!([transaction("one", "10", "FOOD_AND_DRINK")]))
        .await;
    r.writer.exec(|conn|{diesel::sql_query("CREATE TRIGGER fail_plaid_commit BEFORE UPDATE ON plaid_accounts WHEN NEW.body LIKE '%\"cursor\":\"cursor-1\"%' BEGIN SELECT RAISE(ABORT,'injected'); END").execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    assert!(matches!(
        r.service
            .approve(
                &m.id,
                p["digest"].as_str().unwrap(),
                Some("260".into()),
                &|| Ok(())
            )
            .await,
        Err(SyncError::Storage)
    ));
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("import_runs"), 0);
    assert!(r.current(&m).await.cursor.is_none());
    assert!(r.events.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn changed_account_mode_currency_and_type_block_atomic_commit() {
    for (column, value) in [
        ("tracking_mode", "HOLDINGS"),
        ("currency", "EUR"),
        ("account_type", "CREDIT_CARD"),
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Transactions).await;
        let p = r
            .preview(&m, json!([transaction("one", "10", "FOOD_AND_DRINK")]))
            .await;
        let id = m.account_id.clone();
        r.writer
            .exec(move |conn| {
                diesel::sql_query(format!("UPDATE accounts SET {column}=? WHERE id=?"))
                    .bind::<Text, _>(value)
                    .bind::<Text, _>(id)
                    .execute(conn)
                    .map_err(sql)?;
                Ok(())
            })
            .await
            .unwrap();
        r.service
            .approve(
                &m.id,
                p["digest"].as_str().unwrap(),
                Some("260".into()),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(r.count("activities"), 0);
        assert_eq!(r.current(&m).await.status, "review");
    }
}

#[tokio::test]
async fn saved_access_recovers_crash_between_vault_and_connection_write() {
    let r = Rig::new().await;
    let mut c = r.connection(PlaidProduct::Transactions);
    c.status = "exchanging".into();
    c.item_id = None;
    r.repo.put_connection(c.clone(), None).await.unwrap();
    r.vault
        .set_secret(&secret_key(&c, "access").unwrap(), "access")
        .unwrap();
    r.service
        .finish("new-admitted-session", &c.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.api.calls("exchange"), 0);
    let c = r.repo.connections().await.unwrap().remove(0);
    assert_eq!(c.status, "connected");
    assert_eq!(c.item_id.as_deref(), Some("item"));
}

#[tokio::test]
async fn disabled_institution_or_environment_never_receives_provider_calls() {
    let mut r = Rig::new().await;
    let (c, m) = r.seed(PlaidProduct::Transactions).await;
    r.service.institutions.clear();
    assert!(matches!(
        r.service.preview(&m.id, true, &|| Ok(())).await,
        Err(SyncError::Access)
    ));
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert!(r.api.0.lock().unwrap().calls.is_empty());
    r.service.environment = PlaidEnvironment::Production;
    assert!(matches!(
        r.service.accounts(&c.id, &|| Ok(())).await,
        Err(SyncError::Access)
    ));
    assert!(r.api.0.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn review_failure_blocks_scheduled_retries_until_owner_intervenes() {
    let r = Rig::new().await;
    let (_, mut m) = r.seed(PlaidProduct::Transactions).await;
    m.status = "idle".into();
    m.cursor = Some("old".into());
    r.repo.put_mappings(vec![m.clone()]).await.unwrap();
    r.api.0.lock().unwrap().accounts["accounts"][0]["balances"]["iso_currency_code"] = json!("EUR");
    r.service.sync(true, &|| Ok(())).await.unwrap();
    assert_eq!(r.current(&m).await.status, "review");
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("old"));
    let count = r.api.calls("accounts");
    r.service.sync(true, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("accounts"), count);
}

#[tokio::test]
async fn initial_history_must_be_complete_and_pending_does_not_become_ledger_activity() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    r.api.0.lock().unwrap().bank.push_back(Ok(json!({"added":[],"modified":[],"removed":[],"next_cursor":"n","has_more":false,"transactions_update_status":"INITIAL_UPDATE_COMPLETE"})));
    assert!(matches!(
        r.service.preview(&m.id, true, &|| Ok(())).await,
        Err(SyncError::NotReady)
    ));
    let mut pending = transaction("pending", "12", "FOOD_AND_DRINK");
    pending["pending"] = json!(true);
    let p = r.preview(&m, json!([pending])).await;
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("250".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 1); // reviewed opening only
    let mut posted = transaction("posted", "12", "FOOD_AND_DRINK");
    posted["pending_transaction_id"] = json!("pending");
    let p = r.preview(&m, json!([posted])).await;
    r.service
        .approve(&m.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
}

fn brokerage_fixture(kind: &str, symbol: Option<&str>, basis: Value) -> Value {
    json!({"accounts":[account("investment")],"item":item(),
        "securities":[{"security_id":"security","ticker_symbol":symbol,"name":"Synthetic security","type":kind,"iso_currency_code":"USD"}],
        "holdings":[{"account_id":"external","security_id":"security","quantity":"2.5","institution_price":"40","institution_value":"100","cost_basis":basis,"iso_currency_code":"USD"}]})
}

#[tokio::test]
async fn holdings_use_total_value_residual_cash_and_do_not_apply_source_trades() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    r.api.0.lock().unwrap().holdings = Some(brokerage_fixture("other", None, json!("80")));
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert_eq!(r.count("holdings_snapshots"), 0);
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), false, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("holdings_snapshots"), 1);
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("quotes"), 1);
    #[derive(QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type=Text)]
        cash_total_account_currency: String,
        #[diesel(sql_type=Text)]
        positions: String,
    }
    let row =
        diesel::sql_query("SELECT cash_total_account_currency,positions FROM holdings_snapshots")
            .get_result::<Snapshot>(&mut r.pool.get().unwrap())
            .unwrap();
    assert_eq!(
        row.cash_total_account_currency
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::from(150)
    );
    let positions: HashMap<String, wealthfolio_core::portfolio::snapshot::Position> =
        serde_json::from_str(&row.positions).unwrap();
    let position = positions.values().next().unwrap();
    assert_eq!(position.quantity.to_string(), "2.5");
    assert_eq!(position.total_cost_basis.to_string(), "80");
    assert_eq!(position.contract_multiplier, rust_decimal::Decimal::ONE);
    assert_eq!(r.current(&m).await.status, "idle");
    assert_eq!(r.events.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn stale_holdings_approval_and_invalid_quantities_cannot_write_snapshots() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    r.api.0.lock().unwrap().holdings = Some(brokerage_fixture("other", None, Value::Null));
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, "stale", true, &|| Ok(()))
            .await,
        Err(SyncError::Changed)
    ));
    let mut map = r.current(&m).await;
    map.holdings_staged.as_mut().unwrap().fetched_at -= Duration::minutes(31);
    r.repo.put_mappings(vec![map]).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await,
        Err(SyncError::Changed)
    ));
    let mut holdings = brokerage_fixture("other", None, Value::Null);
    holdings["holdings"][0]["quantity"] = json!("0");
    r.api.0.lock().unwrap().holdings = Some(holdings);
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("holdings_snapshots"), 0);
}

#[tokio::test]
async fn explicit_transfer_pair_commits_both_legs_together_and_preserves_owner_edits() {
    let r = Rig::new().await;
    let (_, a) = r.seed(PlaidProduct::Transactions).await;
    let mut b = a.clone();
    b.id = uuid::Uuid::new_v4().to_string();
    b.account_id = uuid::Uuid::new_v4().to_string();
    b.external_id = "other".into();
    let account = b.account_id.clone();
    r.writer.exec(move|conn|{diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Other','CASH','USD',0,1,0,'TRANSACTIONS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(account).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    r.repo
        .add_mapping(b.clone(), PlaidProduct::Transactions)
        .await
        .unwrap();
    r.preview(&a, json!([transaction("out", "10", "TRANSFER_OUT")]))
        .await;
    let mut other_account = account_fixture_for_other();
    r.api.0.lock().unwrap().accounts["accounts"]
        .as_array_mut()
        .unwrap()
        .push(other_account.take());
    let mut tx = transaction("in", "-10", "TRANSFER_IN");
    tx["account_id"] = json!("other");
    r.preview(&b, json!([tx])).await;
    r.service
        .pair(&a.id, "out", &b.id, "in", &|| Ok(()))
        .await
        .unwrap();
    let p = r.service.preview(&a.id, false, &|| Ok(())).await.unwrap();
    r.service
        .approve(
            &a.id,
            p["digest"].as_str().unwrap(),
            Some("260".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 0);
    assert!(r.current(&a).await.cursor.is_none());
    let p = r.service.preview(&b.id, false, &|| Ok(())).await.unwrap();
    r.service
        .approve(
            &b.id,
            p["digest"].as_str().unwrap(),
            Some("240".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 4);
    assert_eq!(r.count("import_runs"), 2);
    assert!(r.current(&a).await.cursor.is_some());
    assert!(r.current(&b).await.cursor.is_some());
    // An owner has deliberately unlinked both legs. Later source corrections
    // must preserve those edits rather than re-impose the provider grouping.
    r.writer.exec(|conn|{diesel::sql_query("UPDATE activities SET is_user_modified=1,source_group_id=NULL,activity_type=CASE WHEN activity_type='TRANSFER_IN' THEN 'DEPOSIT' ELSE 'WITHDRAWAL' END WHERE source_group_id IS NOT NULL").execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    let p = r
        .preview(&a, json!([transaction("out", "10", "TRANSFER_OUT")]))
        .await;
    r.service
        .approve(&a.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.current(&a).await.status, "idle");
    assert_eq!(r.count("activities"), 4);
}
fn account_fixture_for_other() -> Value {
    let mut value = account("depository");
    value["account_id"] = json!("other");
    value
}

#[tokio::test]
async fn initial_cutover_rechecks_late_manual_import_inside_transaction() {
    let r = Rig::new().await;
    let (_, mut m) = r.seed(PlaidProduct::Transactions).await;
    m.cutover_date = Some("2026-11-01".parse().unwrap());
    r.repo.put_mappings(vec![m.clone()]).await.unwrap();
    let p = r
        .preview(&m, json!([transaction("one", "10", "FOOD_AND_DRINK")]))
        .await;
    let account = m.account_id.clone();
    r.writer.exec(move|conn|{diesel::sql_query("INSERT INTO activities(id,account_id,activity_type,activity_date,amount,currency,created_at,updated_at) VALUES('manual',?,'WITHDRAWAL','2026-11-01T12:00:00Z','10','USD',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(account).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    r.service
        .approve(&m.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.current(&m).await.status, "review");
    assert!(r.current(&m).await.cursor.is_none());
    assert_eq!(r.count("activities"), 1);
    assert_eq!(r.count("import_runs"), 0);
}

#[tokio::test]
async fn brokerage_quote_identity_uses_price_date_and_preserves_known_basis_only_at_same_quantity()
{
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut fixture = brokerage_fixture("other", None, json!("80"));
    fixture["holdings"][0]["institution_price_as_of"] = json!("2026-11-01");
    r.api.0.lock().unwrap().holdings = Some(fixture.clone());
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();
    fixture["holdings"][0]["cost_basis"] = Value::Null;
    r.api.0.lock().unwrap().holdings = Some(fixture.clone());
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("quotes"), 1);
    let current = r.current(&m).await;
    assert_eq!(
        current.source_history["basisBySecurity"]["security"]["basis"],
        "80"
    );
    assert_eq!(
        current.source_history["basisBySecurity"]["security"]["status"],
        "retained"
    );
    #[derive(QueryableByName)]
    struct QuoteRow {
        #[diesel(sql_type=Text)]
        id: String,
        #[diesel(sql_type=Text)]
        day: String,
    }
    let row = diesel::sql_query("SELECT id,day FROM quotes")
        .get_result::<QuoteRow>(&mut r.pool.get().unwrap())
        .unwrap();
    assert_eq!(row.day, "2026-11-01");
    assert!(row.id.ends_with("_2026-11-01_BROKER"));
    fixture["holdings"][0]["quantity"] = json!("5");
    r.api.0.lock().unwrap().holdings = Some(fixture.clone());
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(
        r.current(&m).await.source_history["basisBySecurity"]["security"]["status"],
        "unknown"
    );
    fixture["holdings"][0]["institution_price_as_of"] =
        json!((Utc::now() + Duration::days(1)).date_naive().to_string());
    r.api.0.lock().unwrap().holdings = Some(fixture);
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), false, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("quotes"), 1);
}

#[tokio::test]
async fn lifecycle_checks_owner_expiry_duplicates_and_idempotent_completion() {
    let r = Rig::new().await;
    let flow = r
        .service
        .start("owner", "chase", None, false, &|| Ok(()))
        .await
        .unwrap();
    let id = flow["flowId"].as_str().unwrap();
    assert!(matches!(
        r.service
            .start("owner", "chase", None, false, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    assert!(matches!(
        r.service.finish("foreign", id, &|| Ok(())).await,
        Err(SyncError::Access)
    ));
    assert_eq!(r.api.calls("exchange"), 0);
    r.service.finish("owner", id, &|| Ok(())).await.unwrap();
    r.service.finish("owner", id, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("exchange"), 1);
    assert_eq!(
        r.service.list(&|| Ok(())).await.unwrap()["connections"][0]["status"],
        "connected"
    );
    let flow = r
        .service
        .start("new-owner", "chase", Some(id), false, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .finish("new-owner", flow["flowId"].as_str().unwrap(), &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.api.calls("exchange"), 1);
    r.service.disconnect(id, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("remove"), 1);
    assert_eq!(
        r.service.list(&|| Ok(())).await.unwrap()["connections"][0]["status"],
        "disconnected"
    );
    let flow = r
        .service
        .start("owner", "chase", None, false, &|| Ok(()))
        .await
        .unwrap();
    let id = flow["flowId"].as_str().unwrap();
    let mut c = r
        .repo
        .connections()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    c.expires_at = Utc::now() - Duration::seconds(1);
    r.repo
        .put_connection(c, Some("linking".into()))
        .await
        .unwrap();
    assert!(matches!(
        r.service.finish("owner", id, &|| Ok(())).await,
        Err(SyncError::Attention)
    ));
}

#[tokio::test]
async fn adoption_reuses_existing_item_and_never_revokes_on_invalid_ownership() {
    let r = Rig::new().await;
    let result = r
        .service
        .adopt(
            "owner",
            "chase",
            AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert!(result["connectionId"].is_string());
    assert!(matches!(
        r.service
            .adopt(
                "owner",
                "chase",
                AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
                &|| Ok(())
            )
            .await,
        Err(SyncError::Changed)
    ));
    r.api.0.lock().unwrap().accounts["item"]["institution_id"] = json!("ins_foreign");
    assert!(matches!(
        r.service
            .adopt(
                "owner",
                "chase",
                AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
                &|| Ok(())
            )
            .await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.api.calls("remove"), 0);
    assert_eq!(r.repo.connections().await.unwrap().len(), 1);
}

#[tokio::test]
async fn disconnect_retains_pending_state_until_remote_revocation_succeeds() {
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Transactions).await;
    r.api.fail("remove", PlaidError::Unavailable);
    assert!(r.service.disconnect(&c.id, &|| Ok(())).await.is_err());
    assert_eq!(
        r.repo.connections().await.unwrap()[0].status,
        "pending_revocation"
    );
    assert!(r
        .vault
        .get_secret(&secret_key(&c, "access").unwrap())
        .unwrap()
        .is_some());
    r.service.disconnect(&c.id, &|| Ok(())).await.unwrap();
    assert_eq!(
        r.repo.connections().await.unwrap()[0].status,
        "disconnected"
    );
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Transactions).await;
    r.vault.values.lock().unwrap().clear();
    assert!(matches!(
        r.service.disconnect(&c.id, &|| Ok(())).await,
        Err(SyncError::Attention)
    ));
    assert_eq!(
        r.repo.connections().await.unwrap()[0].status,
        "pending_revocation"
    );
}

#[tokio::test]
async fn pagination_restarts_on_mutation_and_rejects_nonprogress_or_foreign_removals() {
    for case in [
        "restart",
        "repeated",
        "empty-cursor",
        "foreign-removal",
        "duplicate",
        "too-many-mutations",
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Transactions).await;
        let page = json!({"added":[],"modified":[],"removed":[],"next_cursor":"same","has_more":true,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"});
        {
            let mut s = r.api.0.lock().unwrap();
            match case {
                "restart" => {
                    s.bank.push_back(Err(PlaidError::PaginationChanged));
                    let mut final_page = page.clone();
                    final_page["has_more"] = json!(false);
                    s.bank.push_back(Ok(final_page));
                }
                "repeated" => {
                    s.bank.push_back(Ok(page.clone()));
                    s.bank.push_back(Ok(page.clone()));
                }
                "empty-cursor" => {
                    let mut p = page.clone();
                    p["next_cursor"] = json!("");
                    s.bank.push_back(Ok(p));
                }
                "foreign-removal" => {
                    let mut p = page.clone();
                    p["removed"] = json!([{"account_id":"foreign","transaction_id":"x"}]);
                    s.bank.push_back(Ok(p));
                }
                "duplicate" => {
                    let mut p = page.clone();
                    let t = transaction("same", "10", "FOOD_AND_DRINK");
                    p["added"] = json!([t.clone(), t]);
                    s.bank.push_back(Ok(p));
                }
                _ => {
                    for _ in 0..3 {
                        s.bank.push_back(Err(PlaidError::PaginationChanged));
                    }
                }
            }
        }
        let result = r.service.preview(&m.id, true, &|| Ok(())).await;
        match case {
            "restart" => {
                assert!(result.is_ok());
                assert_eq!(r.api.calls("transactions"), 2);
            }
            "empty-cursor" => assert!(matches!(result, Err(SyncError::NotReady))),
            "too-many-mutations" => assert!(matches!(result, Err(SyncError::Provider))),
            _ => assert!(matches!(result, Err(SyncError::Review))),
        }
        assert!(r.current(&m).await.cursor.is_none());
        assert_eq!(r.count("activities"), 0);
    }
}

#[tokio::test]
async fn scheduled_sync_is_due_daily_and_transient_failure_does_not_require_review() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    let p = r.preview(&m, json!([])).await;
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("250".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    let before = r.api.calls("accounts");
    r.service.sync(true, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("accounts"), before);
    r.api.fail("accounts", PlaidError::Unavailable);
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.current(&m).await.status, "idle");
    assert!(r.current(&m).await.last_error.is_some());
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("cursor-2"));
    assert!(r.current(&m).await.last_error.is_none());
}

#[tokio::test]
async fn holdings_validate_owned_records_security_uniqueness_currency_and_value() {
    for case in [
        "item",
        "currency",
        "security",
        "duplicate-security",
        "quantity",
        "negative-price",
        "cash",
        "zero",
        "missing-basis",
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        let mut fixture = brokerage_fixture("other", None, Value::Null);
        match case {
            "item" => fixture["item"]["item_id"] = json!("foreign"),
            "currency" => fixture["holdings"][0]["iso_currency_code"] = json!("EUR"),
            "security" => fixture["holdings"][0]["security_id"] = json!("missing"),
            "duplicate-security" => {
                let duplicate = fixture["securities"][0].clone();
                fixture["securities"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            "quantity" => fixture["holdings"][0]["quantity"] = json!("0"),
            "negative-price" => fixture["holdings"][0]["institution_value"] = json!("-100"),
            "cash" => fixture["securities"][0]["type"] = json!("cash"),
            "zero" => {
                fixture["holdings"][0]["quantity"] = json!("0");
                fixture["holdings"][0]["institution_value"] = json!("0");
            }
            _ => {}
        }
        r.api.0.lock().unwrap().holdings = Some(fixture);
        let result = r.service.preview_holdings(&m.id, &|| Ok(())).await;
        if case == "item" {
            assert!(matches!(result, Err(SyncError::Review)));
            continue;
        }
        let p = result.unwrap();
        let result = r
            .service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await;
        if matches!(case, "cash" | "zero" | "missing-basis") {
            assert!(result.is_ok(), "{case}");
            assert_eq!(r.count("holdings_snapshots"), 1);
        } else {
            assert!(matches!(result, Err(SyncError::Review)), "{case}");
            assert_eq!(r.count("holdings_snapshots"), 0);
        }
    }
}

#[tokio::test]
async fn every_owner_operation_rechecks_admission_before_side_effects() {
    let r = Rig::new().await;
    let (c, m) = r.seed(PlaidProduct::Transactions).await;
    let denied = || Err(SyncError::Access);
    assert!(matches!(
        r.service.list(&denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service
            .start("owner", "chase", None, false, &denied)
            .await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.finish("owner", &c.id, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service
            .adopt(
                "owner",
                "chase",
                AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
                &denied
            )
            .await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.disconnect(&c.id, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.accounts(&c.id, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service
            .map_account(&c.id, "external", &m.account_id, false, None, &denied)
            .await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.preview(&m.id, true, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.decide(&m.id, "one", "income", &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.pair(&m.id, "one", "other", "two", &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.approve(&m.id, "digest", None, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.preview_holdings(&m.id, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, "digest", false, &denied)
            .await,
        Err(SyncError::Access)
    ));
    assert!(matches!(
        r.service.sync(false, &denied).await,
        Err(SyncError::Access)
    ));
    assert!(r.api.0.lock().unwrap().calls.is_empty());
    assert_eq!(r.count("activities"), 0);
}

#[tokio::test]
async fn account_mapping_requires_local_compatibility_and_merrill_attestation() {
    for product in [PlaidProduct::Transactions, PlaidProduct::Investments] {
        let r = Rig::new().await;
        let (c, _) = r.seed(product).await;
        let local = uuid::Uuid::new_v4().to_string();
        let l = local.clone();
        r.writer.exec(move|conn|{diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Mapping',?,'USD',0,1,0,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(l).bind::<Text,_>(if product==PlaidProduct::Investments{"SECURITIES"}else{"CASH"}).bind::<Text,_>(if product==PlaidProduct::Investments{"HOLDINGS"}else{"TRANSACTIONS"}).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
        let mut a = account(if product == PlaidProduct::Investments {
            "investment"
        } else {
            "depository"
        });
        a["account_id"] = json!("second");
        r.api.0.lock().unwrap().accounts["accounts"]
            .as_array_mut()
            .unwrap()
            .push(a);
        if product == PlaidProduct::Investments {
            assert!(matches!(
                r.service
                    .map_account(&c.id, "second", &local, false, None, &|| Ok(()))
                    .await,
                Err(SyncError::Review)
            ));
        }
        for date in ["invalid", "2999-01-01"] {
            assert!(matches!(
                r.service
                    .map_account(&c.id, "second", &local, true, Some(date.into()), &|| Ok(()))
                    .await,
                Err(SyncError::Review)
            ));
        }
        r.service
            .map_account(&c.id, "second", &local, true, None, &|| Ok(()))
            .await
            .unwrap();
        assert_eq!(r.repo.mappings().await.unwrap().len(), 2);
        assert!(r
            .service
            .map_account(&c.id, "second", &local, true, None, &|| Ok(()))
            .await
            .is_err());
        assert!(matches!(
            r.service
                .map_account(&c.id, "foreign", &local, true, None, &|| Ok(()))
                .await,
            Err(SyncError::Review)
        ));
    }
}

#[tokio::test]
async fn removed_owner_edited_record_blocks_until_owner_explicitly_deletes_it() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    let p = r
        .preview(&m, json!([transaction("one", "10", "FOOD_AND_DRINK")]))
        .await;
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("260".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    r.writer.exec(|conn|{diesel::sql_query("UPDATE activities SET is_user_modified=1,notes='Owner correction' WHERE source_record_id='one'").execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    r.api.0.lock().unwrap().bank.push_back(Ok(json!({"added":[],"modified":[],"removed":[{"account_id":"external","transaction_id":"one"}],"next_cursor":"removed","has_more":false})));
    let p = r.service.preview(&m.id, true, &|| Ok(())).await.unwrap();
    assert_eq!(
        p["removedRecords"][0]["activityId"],
        wealthfolio_connect::plaid::ingest::stable_id(&m.id, "one")
    );
    assert_eq!(p["removedRecords"][0]["name"], "Synthetic purchase");
    r.service
        .approve(&m.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.current(&m).await.status, "review");
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("cursor-1"));
    assert_eq!(r.count("activities"), 2);
    r.writer
        .exec(|conn| {
            diesel::sql_query("DELETE FROM activities WHERE source_record_id='one'")
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    r.service
        .approve(&m.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("removed"));
    assert_eq!(r.count("activities"), 1);
}

#[tokio::test]
async fn credit_card_opening_uses_negative_liability_balance_and_ignores_available_credit() {
    let r = Rig::new().await;
    let (_, m) = r.seed_kind(PlaidProduct::Transactions, true).await;
    r.api.0.lock().unwrap().accounts["accounts"] = json!([account_fixture_card()]);
    let p = r
        .preview(&m, json!([transaction("purchase", "20", "FOOD_AND_DRINK")]))
        .await;
    assert_eq!(p["suggestedOpening"], "-230");
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            Some("-230".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
}
fn account_fixture_card() -> Value {
    account("credit")
}

#[tokio::test]
async fn connection_storage_and_vault_failures_do_not_leave_active_credentials() {
    for operation in [
        "start",
        "adopt",
        "active-vault-failure",
        "revoked-cleanup-success",
    ] {
        let r = Rig::new().await;
        if matches!(operation, "start" | "adopt") {
            r.writer.exec(|conn|{diesel::sql_query("CREATE TRIGGER reject_connection BEFORE INSERT ON plaid_connections BEGIN SELECT RAISE(FAIL,'fixture'); END").execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
            let result = if operation == "start" {
                r.service
                    .start("owner", "chase", None, false, &|| Ok(()))
                    .await
            } else {
                r.service
                    .adopt(
                        "owner",
                        "chase",
                        AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
                        &|| Ok(()),
                    )
                    .await
            };
            assert!(matches!(result, Err(SyncError::Storage)));
            assert!(r.vault.values.lock().unwrap().is_empty());
            assert_eq!(r.api.calls("remove"), 0);
        } else {
            let flow = r
                .service
                .start("owner", "chase", None, false, &|| Ok(()))
                .await
                .unwrap();
            if operation == "active-vault-failure" {
                *r.vault.fail.lock().unwrap() = Some("set");
                assert!(matches!(
                    r.service
                        .finish("owner", flow["flowId"].as_str().unwrap(), &|| Ok(()))
                        .await,
                    Err(SyncError::Storage)
                ));
            } else {
                let active = Arc::new(AtomicBool::new(true));
                r.api.0.lock().unwrap().revoke_admission = Some(active.clone());
                let admit = || {
                    if active.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(SyncError::Access)
                    }
                };
                assert!(matches!(
                    r.service
                        .finish("owner", flow["flowId"].as_str().unwrap(), &admit)
                        .await,
                    Err(SyncError::Access)
                ));
                assert!(r.vault.values.lock().unwrap().is_empty());
                assert_eq!(
                    r.repo.connections().await.unwrap()[0].status,
                    "disconnected"
                );
            }
            assert_eq!(r.api.calls("remove"), 1);
        }
    }
}

#[tokio::test]
async fn verified_item_product_and_connection_status_gate_every_import() {
    for case in [
        "item-mismatch",
        "empty-item",
        "missing-product",
        "missing-token",
        "not-connected",
        "credit-classification",
    ] {
        let r = Rig::new().await;
        let (mut c, m) = r.seed(PlaidProduct::Transactions).await;
        match case {
            "item-mismatch" => {
                r.api.0.lock().unwrap().accounts["item"]["item_id"] = json!("foreign")
            }
            "empty-item" => r.api.0.lock().unwrap().accounts["item"]["item_id"] = json!(""),
            "missing-product" => {
                let mut state = r.api.0.lock().unwrap();
                state.accounts["item"]["available_products"] = json!([]);
                state.accounts["item"]["billed_products"] = json!([]);
            }
            "missing-token" => r.vault.values.lock().unwrap().clear(),
            "not-connected" => {
                c.status = "attention".into();
                r.repo
                    .put_connection(c, Some("connected".into()))
                    .await
                    .unwrap();
            }
            _ => r.api.0.lock().unwrap().accounts["accounts"] = json!([account("credit")]),
        }
        assert!(
            r.service.preview(&m.id, true, &|| Ok(())).await.is_err(),
            "{case}"
        );
        assert_eq!(r.count("activities"), 0);
        if case == "missing-token" {
            assert_eq!(
                r.service.list(&|| Ok(())).await.unwrap()["connections"][0]["status"],
                "disconnected"
            );
        }
        if case == "not-connected" {
            assert!(matches!(
                r.service.accounts(&m.connection_id, &|| Ok(())).await,
                Err(SyncError::Attention)
            ));
            assert!(r
                .service
                .map_account(
                    &m.connection_id,
                    "external",
                    &m.account_id,
                    false,
                    None,
                    &|| Ok(())
                )
                .await
                .is_err());
        }
    }
}

#[tokio::test]
async fn reconnect_resets_attention_without_replacing_valid_cursor() {
    for success in [false, true] {
        let r = Rig::new().await;
        let (mut c, mut m) = r.seed(PlaidProduct::Transactions).await;
        assert!(matches!(
            r.service
                .start("owner", "amex", Some(&c.id), false, &|| Ok(()))
                .await,
            Err(SyncError::Changed)
        ));
        m.status = "attention".into();
        m.cursor = Some("owned-cursor".into());
        m.last_error = Some("needs reconnect".into());
        if success {
            m.last_success = Some(Utc::now());
        }
        r.repo.put_mappings(vec![m.clone()]).await.unwrap();
        c.status = "attention".into();
        r.repo
            .put_connection(c.clone(), Some("connected".into()))
            .await
            .unwrap();
        r.service
            .start("new-owner", "chase", Some(&c.id), false, &|| Ok(()))
            .await
            .unwrap();
        assert!(matches!(
            r.service
                .start("owner", "chase", Some(&c.id), false, &|| Ok(()))
                .await,
            Err(SyncError::Changed)
        ));
        r.service
            .finish("new-owner", &c.id, &|| Ok(()))
            .await
            .unwrap();
        let m = r.current(&m).await;
        assert_eq!(m.status, if success { "idle" } else { "initial_review" });
        assert_eq!(m.cursor.as_deref(), Some("owned-cursor"));
        assert!(m.last_error.is_none());
        assert_eq!(r.api.calls("exchange"), 0);
    }
}

#[tokio::test]
async fn banking_page_resource_limits_and_empty_removal_ids_fail_without_cursor() {
    for case in [
        "empty-removal",
        "cursor-cycle",
        "page-limit",
        "record-limit",
        "stage-limit",
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Transactions).await;
        let base = json!({"added":[],"modified":[],"removed":[],"next_cursor":"a","has_more":true,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"});
        {
            let mut state = r.api.0.lock().unwrap();
            match case {
                "empty-removal" => {
                    let mut p = base.clone();
                    p["removed"] = json!([{"account_id":"external","transaction_id":""}]);
                    state.bank.push_back(Ok(p));
                }
                "cursor-cycle" => {
                    for cursor in ["a", "b", "a"] {
                        let mut p = base.clone();
                        p["next_cursor"] = json!(cursor);
                        state.bank.push_back(Ok(p));
                    }
                }
                "page-limit" => {
                    for n in 0..100 {
                        let mut p = base.clone();
                        p["next_cursor"] = json!(format!("cursor-{n}"));
                        state.bank.push_back(Ok(p));
                    }
                }
                "record-limit" => {
                    let mut p = base.clone();
                    p["added"] = Value::Array(
                        (0..10001)
                            .map(|n| transaction(&n.to_string(), "1", "FOOD_AND_DRINK"))
                            .collect(),
                    );
                    state.bank.push_back(Ok(p));
                }
                _ => {
                    let mut p = base.clone();
                    p["has_more"] = json!(false);
                    p["added"] = Value::Array(
                        (0..2100)
                            .map(|n| {
                                let mut t = transaction(&n.to_string(), "1", "FOOD_AND_DRINK");
                                t["name"] = json!("x".repeat(4096));
                                t
                            })
                            .collect(),
                    );
                    state.bank.push_back(Ok(p));
                }
            }
        }
        assert!(
            matches!(
                r.service.preview(&m.id, true, &|| Ok(())).await,
                Err(SyncError::Review)
            ),
            "{case}"
        );
        assert!(r.current(&m).await.cursor.is_none());
        assert_eq!(r.count("activities"), 0);
    }
}

fn investment_record(id: &str) -> Value {
    json!({"investment_transaction_id":id,"account_id":"external","date":"2026-11-01","name":"Synthetic investment","type":"buy","subtype":"buy","amount":"10","price":"10","quantity":"1","iso_currency_code":"USD"})
}
fn investment_page(records: Value, total: u32) -> Value {
    json!({"item":item(),"accounts":[account("investment")],"securities":[],"investment_transactions":records,"total_investment_transactions":total})
}
#[tokio::test]
async fn investment_history_pagination_is_complete_owned_and_bounded() {
    for case in [
        "valid",
        "foreign-item",
        "too-many",
        "changed-total",
        "foreign-account",
        "old-date",
        "future-date",
        "duplicate",
        "empty-page",
        "over-total",
        "page-limit",
        "stage-limit",
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        {
            let mut s = r.api.0.lock().unwrap();
            match case {
                "valid" => {
                    s.investments
                        .push_back(investment_page(json!([investment_record("one")]), 2));
                    s.investments
                        .push_back(investment_page(json!([investment_record("two")]), 2));
                }
                "foreign-item" => {
                    let mut p = investment_page(json!([]), 0);
                    p["item"]["item_id"] = json!("foreign");
                    s.investments.push_back(p);
                }
                "too-many" => s.investments.push_back(investment_page(json!([]), 10001)),
                "changed-total" => {
                    s.investments
                        .push_back(investment_page(json!([investment_record("one")]), 2));
                    s.investments
                        .push_back(investment_page(json!([investment_record("two")]), 3));
                }
                "foreign-account" | "old-date" | "future-date" => {
                    let mut t = investment_record("one");
                    match case {
                        "foreign-account" => t["account_id"] = json!("foreign"),
                        "old-date" => t["date"] = json!("2020-01-01"),
                        _ => {
                            t["date"] =
                                json!((Utc::now() + Duration::days(1)).date_naive().to_string())
                        }
                    };
                    s.investments.push_back(investment_page(json!([t]), 1));
                }
                "duplicate" => s.investments.push_back(investment_page(
                    json!([investment_record("one"), investment_record("one")]),
                    2,
                )),
                "empty-page" => s.investments.push_back(investment_page(json!([]), 1)),
                "over-total" => s
                    .investments
                    .push_back(investment_page(json!([investment_record("one")]), 0)),
                "page-limit" => {
                    for n in 0..100 {
                        s.investments.push_back(investment_page(
                            json!([investment_record(&n.to_string())]),
                            101,
                        ));
                    }
                }
                _ => {
                    let mut t = investment_record("one");
                    t["name"] = json!("x".repeat(8 * 1024 * 1024));
                    s.investments.push_back(investment_page(json!([t]), 1));
                }
            }
        }
        let result = r.service.preview_holdings(&m.id, &|| Ok(())).await;
        if case == "valid" {
            assert_eq!(result.unwrap()["transactionCount"], 2);
        } else {
            assert!(matches!(result, Err(SyncError::Review)), "{case}");
        }
        assert_eq!(r.count("holdings_snapshots"), 0);
        assert!(r.current(&m).await.last_success.is_none());
    }
}

async fn unbound_account(r: &Rig, product: PlaidProduct) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let local = id.clone();
    r.writer.exec(move|conn|{diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Unbound',?,'USD',0,1,0,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(local).bind::<Text,_>(if product==PlaidProduct::Investments{"SECURITIES"}else{"CASH"}).bind::<Text,_>(if product==PlaidProduct::Investments{"HOLDINGS"}else{"TRANSACTIONS"}).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
    id
}
async fn manual_activity(r: &Rig, account: String, date: &str) {
    let date = date.to_string();
    r.writer.exec(move|conn|{diesel::sql_query("INSERT INTO activities(id,account_id,activity_type,activity_date,amount,currency,created_at,updated_at) VALUES('manual',?,'WITHDRAWAL',?,'10','USD',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(account).bind::<Text,_>(date).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
}
#[tokio::test]
async fn mapping_storage_enforces_all_account_and_cutover_preconditions() {
    for case in [
        "archived",
        "provider",
        "currency",
        "mode",
        "kind",
        "card-kind",
        "investment-kind",
        "existing-activity",
        "existing-snapshot",
        "cutover-empty",
        "cutover-overlap",
        "cutover-investment",
        "cutover-valid",
        "connection-status",
        "connection-product",
    ] {
        let product = if matches!(case, "investment-kind" | "cutover-investment") {
            PlaidProduct::Investments
        } else {
            PlaidProduct::Transactions
        };
        let r = Rig::new().await;
        let (mut c, mut m) = r.seed(product).await;
        m.id = uuid::Uuid::new_v4().to_string();
        m.external_id = "second".into();
        m.account_id = unbound_account(&r, product).await;
        let update = match case {
            "archived" => Some("is_archived=1"),
            "provider" => Some("provider='OTHER'"),
            "currency" => Some("currency='EUR'"),
            "mode" => Some("tracking_mode='HOLDINGS'"),
            "kind" | "card-kind" => Some("account_type='SECURITIES'"),
            "investment-kind" => Some("account_type='CASH'"),
            _ => None,
        };
        if let Some(set) = update {
            let account = m.account_id.clone();
            r.writer
                .exec(move |conn| {
                    diesel::sql_query(format!("UPDATE accounts SET {set} WHERE id=?"))
                        .bind::<Text, _>(account)
                        .execute(conn)
                        .map_err(sql)?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        if case == "card-kind" {
            m.credit_card = true;
        }
        if case.starts_with("cutover") {
            m.cutover_date = Some("2026-11-01".parse().unwrap());
        }
        if matches!(case, "existing-activity" | "cutover-overlap") {
            manual_activity(&r, m.account_id.clone(), "2026-11-01T12:00:00Z").await;
        }
        if matches!(case, "cutover-valid" | "cutover-investment") {
            manual_activity(&r, m.account_id.clone(), "2026-10-31T12:00:00Z").await;
        }
        if case == "existing-snapshot" {
            let account = m.account_id.clone();
            r.writer
                .exec_tx(move |tx| {
                    let s = AccountStateSnapshot {
                        account_id: account.clone(),
                        id: AccountStateSnapshot::stable_id(&account, Utc::now().date_naive()),
                        snapshot_date: Utc::now().date_naive(),
                        currency: "USD".into(),
                        source: wealthfolio_core::portfolio::snapshot::SnapshotSource::ManualEntry,
                        ..Default::default()
                    };
                    crate::portfolio::snapshot::SnapshotRepository::save_snapshot_tx(tx, &s)
                })
                .await
                .unwrap();
        }
        if case == "connection-status" {
            c.status = "attention".into();
            r.repo
                .put_connection(c, Some("connected".into()))
                .await
                .unwrap();
        }
        let result = r
            .repo
            .add_mapping(
                m,
                if case == "connection-product" {
                    PlaidProduct::Investments
                } else {
                    product
                },
            )
            .await;
        if case == "cutover-valid" {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err(), "{case}");
        }
    }
}

#[tokio::test]
async fn immutable_mapping_ownership_connection_identity_and_cas_are_enforced() {
    let r = Rig::new().await;
    let (c, m) = r.seed(PlaidProduct::Transactions).await;
    for field in ["account", "external", "connection", "credit-card"] {
        let mut altered = m.clone();
        match field {
            "account" => altered.account_id = "foreign".into(),
            "external" => altered.external_id = "foreign".into(),
            "credit-card" => altered.credit_card = true,
            _ => altered.connection_id = "foreign".into(),
        };
        assert!(matches!(
            r.repo.put_mappings(vec![altered]).await,
            Err(SyncError::Changed)
        ));
    }
    for field in ["environment", "owner", "institution", "cas"] {
        let mut altered = c.clone();
        match field {
            "environment" => altered.environment = PlaidEnvironment::Production,
            "owner" => altered.owner = "foreign".into(),
            "institution" => altered.institution = "amex".into(),
            _ => {}
        };
        assert!(
            matches!(
                r.repo
                    .put_connection(
                        altered,
                        Some(if field == "cas" { "wrong" } else { "connected" }.into())
                    )
                    .await,
                Err(SyncError::Changed)
            ),
            "{field}"
        );
    }
    assert_eq!(r.repo.connections().await.unwrap()[0].owner, "owner");
    assert_eq!(r.current(&m).await.account_id, m.account_id);
}

#[tokio::test]
async fn commit_rechecks_binding_archival_cursor_and_initial_emptiness() {
    for case in [
        "provider",
        "external",
        "archived",
        "status",
        "cursor",
        "late-activity",
    ] {
        let r = Rig::new().await;
        let (mut c, m) = r.seed(PlaidProduct::Transactions).await;
        r.preview(&m, json!([transaction("one", "10", "FOOD_AND_DRINK")]))
            .await;
        let mut prepared = r.current(&m).await;
        prepared.staged.as_mut().unwrap().approved = true;
        prepared.staged.as_mut().unwrap().opening_balance = Some("260".into());
        let batch = wealthfolio_connect::plaid::ingest::bank_commit(prepared).unwrap();
        if case == "status" {
            c.status = "attention".into();
            r.repo
                .put_connection(c, Some("connected".into()))
                .await
                .unwrap();
        } else if case == "cursor" {
            let mut changed = r.current(&m).await;
            changed.cursor = Some("newer".into());
            r.repo.put_mappings(vec![changed]).await.unwrap();
        } else if case == "late-activity" {
            manual_activity(&r, m.account_id.clone(), "2026-11-01T12:00:00Z").await;
        } else {
            let account = m.account_id.clone();
            let set = match case {
                "provider" => "provider=NULL",
                "external" => "provider_account_id='foreign'",
                _ => "is_archived=1",
            };
            r.writer
                .exec(move |conn| {
                    diesel::sql_query(format!("UPDATE accounts SET {set} WHERE id=?"))
                        .bind::<Text, _>(account)
                        .execute(conn)
                        .map_err(sql)?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        assert!(r.repo.commit_banks(vec![batch]).await.is_err(), "{case}");
        assert_eq!(r.count("import_runs"), 0);
        assert_eq!(
            r.count("activities"),
            if case == "late-activity" { 1 } else { 0 }
        );
    }
}

async fn transfer_batch(
    r: &Rig,
    m: &Mapping,
    record: &str,
    amount: &str,
    group: &str,
) -> BankCommit {
    let mut t = transaction(record, amount, "TRANSFER_OUT");
    t["account_id"] = json!(m.external_id);
    let mut prepared = m.clone();
    prepared.cursor = Some("before".into());
    prepared.decisions.insert(
        record.into(),
        Decision {
            kind: "internal".into(),
            group: Some(group.into()),
        },
    );
    prepared.staged = Some(BankStage {
        records: vec![parse(t)],
        removed: vec![],
        next_cursor: "after".into(),
        checkpoint_in: Some("before".into()),
        balance: "0".into(),
        fetched_at: Utc::now(),
        approved: true,
        opening_balance: None,
        digest: String::new(),
    });
    r.repo.put_mappings(vec![prepared.clone()]).await.unwrap();
    wealthfolio_connect::plaid::ingest::bank_commit(prepared).unwrap()
}
#[tokio::test]
async fn transfer_integrity_rejects_orphans_unposted_mismatched_and_invalid_legs_atomically() {
    for case in [
        "valid",
        "reverse",
        "orphan",
        "first-status",
        "second-status",
        "same-account",
        "currency",
        "amount",
        "missing-amount",
        "zero",
        "wrong-type",
        "old-orphan",
        "old-empty",
        "three-legs",
    ] {
        let r = Rig::new().await;
        let (_, a) = r.seed(PlaidProduct::Transactions).await;
        let mut b = a.clone();
        b.id = uuid::Uuid::new_v4().to_string();
        b.external_id = "second".into();
        b.account_id = unbound_account(&r, PlaidProduct::Transactions).await;
        r.repo
            .add_mapping(b.clone(), PlaidProduct::Transactions)
            .await
            .unwrap();
        let group = uuid::Uuid::new_v4().to_string();
        let mut first = transfer_batch(&r, &a, "first", "10", &group).await;
        let mut second = transfer_batch(&r, &b, "second", "-10", &group).await;
        if matches!(case, "old-orphan" | "old-empty") {
            r.repo.commit_banks(vec![first, second]).await.unwrap();
            let mut ma = r.current(&a).await;
            let mut mb = r.current(&b).await;
            ma.cursor = Some("before".into());
            mb.cursor = Some("before".into());
            r.repo.put_mappings(vec![ma, mb]).await.unwrap();
            first = transfer_batch(&r, &a, "first", "10", &group).await;
            second = transfer_batch(&r, &b, "second", "-10", &group).await;
            first.activities[0].source_group_id = None;
            first.activities[0].activity_type = "WITHDRAWAL".into();
            if case == "old-empty" {
                second.activities[0].source_group_id = None;
                second.activities[0].activity_type = "DEPOSIT".into();
            }
        }
        match case {
            "first-status" => {
                first.activities[0].status =
                    Some(wealthfolio_core::activities::ActivityStatus::Draft)
            }
            "second-status" => {
                second.activities[0].status =
                    Some(wealthfolio_core::activities::ActivityStatus::Draft)
            }
            "same-account" => second.activities[0].account_id = a.account_id.clone(),
            "currency" => second.activities[0].currency = "EUR".into(),
            "amount" => second.activities[0].amount = Some(rust_decimal::Decimal::from(20)),
            "missing-amount" => second.activities[0].amount = None,
            "zero" => second.activities[0].amount = Some(rust_decimal::Decimal::ZERO),
            "wrong-type" => second.activities[0].activity_type = "CREDIT".into(),
            "three-legs" => {
                let mut extra = second.activities[0].clone();
                extra.id = "extra".into();
                extra.source_record_id = Some("extra".into());
                extra.idempotency_key = Some("extra".into());
                second.activities.push(extra);
            }
            _ => {}
        }
        let mut batches = if case == "orphan" {
            vec![first]
        } else {
            vec![first, second]
        };
        if case == "reverse" {
            batches.reverse();
        }
        let result = r.repo.commit_banks(batches).await;
        if matches!(case, "valid" | "reverse" | "old-empty") {
            assert!(result.is_ok(), "{case}: {result:?}");
        } else {
            assert!(
                matches!(result, Err(SyncError::Review))
                    || (case == "missing-amount" && matches!(result, Err(SyncError::Storage))),
                "{case}: {result:?}"
            );
            assert_eq!(r.current(&a).await.cursor.as_deref(), Some("before"));
        }
        assert_eq!(
            r.count("activities"),
            if matches!(case, "valid" | "reverse" | "old-empty" | "old-orphan") {
                2
            } else {
                0
            }
        );
    }
}

#[tokio::test]
async fn broker_snapshot_commit_preserves_manual_entries_and_rolls_back_quotes_and_metadata() {
    for case in ["manual", "rollback"] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        r.api.0.lock().unwrap().holdings = Some(brokerage_fixture("other", None, json!("80")));
        let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
        if case == "manual" {
            let account = m.account_id.clone();
            let date = Utc::now()
                .with_timezone(
                    &wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
                        "America/Los_Angeles",
                    ),
                )
                .date_naive();
            r.writer
                .exec_tx(move |tx| {
                    let s = AccountStateSnapshot {
                        account_id: account.clone(),
                        id: AccountStateSnapshot::stable_id(&account, date),
                        snapshot_date: date,
                        currency: "USD".into(),
                        source: wealthfolio_core::portfolio::snapshot::SnapshotSource::ManualEntry,
                        ..Default::default()
                    };
                    crate::portfolio::snapshot::SnapshotRepository::save_snapshot_tx(tx, &s)
                })
                .await
                .unwrap();
        } else {
            r.writer.exec(|conn|{diesel::sql_query("CREATE TRIGGER reject_quotes BEFORE INSERT ON quotes BEGIN SELECT RAISE(FAIL,'fixture'); END").execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
        }
        assert!(r
            .service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await
            .is_err());
        assert!(r.current(&m).await.last_success.is_none());
        assert_eq!(r.count("quotes"), 0);
        assert_eq!(
            r.count("holdings_snapshots"),
            if case == "manual" { 1 } else { 0 }
        );
        assert!(r.events.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unexpected_link_result_and_unverified_item_are_never_connected() {
    for case in ["missing-public", "wrong-item", "empty-item"] {
        let r = Rig::new().await;
        let flow = r
            .service
            .start("owner", "chase", None, false, &|| Ok(()))
            .await
            .unwrap();
        let id = flow["flowId"].as_str().unwrap();
        match case {
            "missing-public" => r.api.0.lock().unwrap().result_public = false,
            "wrong-item" => r.api.0.lock().unwrap().accounts["item"]["item_id"] = json!("foreign"),
            _ => {
                let mut c = r.repo.connections().await.unwrap().remove(0);
                c.status = "validating".into();
                c.item_id = None;
                r.vault
                    .set_secret(&secret_key(&c, "access").unwrap(), "access")
                    .unwrap();
                r.repo
                    .put_connection(c, Some("linking".into()))
                    .await
                    .unwrap();
                r.api.0.lock().unwrap().accounts["item"]["item_id"] = json!("");
            }
        }
        assert!(r.service.finish("owner", id, &|| Ok(())).await.is_err());
        assert_ne!(r.repo.connections().await.unwrap()[0].status, "connected");
        assert_eq!(r.count("activities"), 0);
    }
}
#[tokio::test]
async fn connection_cap_duplicate_login_and_environment_are_explicit() {
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Transactions).await;
    r.service
        .start("owner", "chase", None, true, &|| Ok(()))
        .await
        .unwrap();
    for _ in 0..18 {
        let mut extra = c.clone();
        extra.id = uuid::Uuid::new_v4().to_string();
        extra.item_id = None;
        r.repo.put_connection(extra, None).await.unwrap();
    }
    assert!(matches!(
        r.service
            .start("owner", "chase", None, true, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    let r = Rig::new().await;
    let mut other = r.connection(PlaidProduct::Transactions);
    other.environment = PlaidEnvironment::Production;
    r.repo.put_connection(other, None).await.unwrap();
    r.service
        .adopt(
            "owner",
            "chase",
            AccessToken::new(PlaidEnvironment::Sandbox, "access".into()).unwrap(),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.repo.connections().await.unwrap().len(), 2);
}
#[tokio::test]
async fn cancellation_distinguishes_unexchanged_flow_from_ambiguous_exchange() {
    for ambiguous in [false, true] {
        let r = Rig::new().await;
        let mut c = r.connection(PlaidProduct::Transactions);
        c.status = if ambiguous { "exchanging" } else { "linking" }.into();
        c.item_id = None;
        r.repo.put_connection(c.clone(), None).await.unwrap();
        let result = r.service.disconnect(&c.id, &|| Ok(())).await;
        if ambiguous {
            assert!(matches!(result, Err(SyncError::Attention)));
        } else {
            assert!(result.is_ok());
        }
        assert_eq!(r.api.calls("remove"), 0);
    }
}
#[tokio::test]
async fn review_kind_connection_state_cutover_and_cache_freshness_are_enforced() {
    let r = Rig::new().await;
    let (mut c, m) = r.seed(PlaidProduct::Investments).await;
    assert!(matches!(
        r.service.preview(&m.id, true, &|| Ok(())).await,
        Err(SyncError::Attention)
    ));
    r.api.0.lock().unwrap().accounts["accounts"] = json!([account("investment")]);
    assert!(matches!(
        r.service
            .map_account(
                &c.id,
                "external",
                "local",
                true,
                Some("2026-10-01".into()),
                &|| Ok(())
            )
            .await,
        Err(SyncError::Review)
    ));
    c.status = "attention".into();
    r.repo
        .put_connection(c, Some("connected".into()))
        .await
        .unwrap();
    assert!(matches!(
        r.service.preview_holdings(&m.id, &|| Ok(())).await,
        Err(SyncError::Attention)
    ));
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, "digest", true, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Transactions).await;
    assert!(matches!(
        r.service.preview_holdings(&m.id, &|| Ok(())).await,
        Err(SyncError::Review)
    ));
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, "digest", true, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    r.preview(&m, json!([])).await;
    let before = r.api.calls("accounts");
    r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("accounts"), before);
    let mut current = r.current(&m).await;
    current.staged.as_mut().unwrap().fetched_at -= Duration::minutes(31);
    r.repo.put_mappings(vec![current]).await.unwrap();
    r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("accounts"), before + 1);
    for (record, kind) in [("missing", "expense"), ("missing", "invalid")] {
        assert!(matches!(
            r.service.decide(&m.id, record, kind, &|| Ok(())).await,
            Err(SyncError::Review)
        ));
    }
}
fn staged_transfer(m: &mut Mapping, id: &str, amount: &str, pending: bool) {
    let mut t = transaction(id, amount, "TRANSFER_OUT");
    t["account_id"] = json!(m.external_id);
    t["pending"] = json!(pending);
    m.staged = Some(BankStage {
        records: vec![parse(t)],
        removed: vec![],
        next_cursor: "next".into(),
        checkpoint_in: m.cursor.clone(),
        balance: "0".into(),
        fetched_at: Utc::now(),
        approved: false,
        opening_balance: None,
        digest: String::new(),
    });
}
#[tokio::test]
async fn transfer_pairing_rejects_same_account_currency_missing_pending_zero_and_mismatched_legs() {
    let r = Rig::new().await;
    let (_, mut a) = r.seed(PlaidProduct::Transactions).await;
    let mut b = a.clone();
    b.id = uuid::Uuid::new_v4().to_string();
    b.account_id = unbound_account(&r, PlaidProduct::Transactions).await;
    b.external_id = "second".into();
    r.repo
        .add_mapping(b.clone(), PlaidProduct::Transactions)
        .await
        .unwrap();
    for case in [
        "same",
        "currency",
        "missing-first",
        "pending-first",
        "missing-second",
        "pending-second",
        "zero",
        "amount",
    ] {
        staged_transfer(
            &mut a,
            "one",
            if case == "zero" { "0" } else { "10" },
            case == "pending-first",
        );
        staged_transfer(
            &mut b,
            "two",
            if case == "amount" { "-20" } else { "-10" },
            case == "pending-second",
        );
        b.currency = if case == "currency" { "EUR" } else { "USD" }.into();
        r.repo
            .put_mappings(vec![a.clone(), b.clone()])
            .await
            .unwrap();
        assert!(
            matches!(
                r.service
                    .pair(
                        &a.id,
                        if case == "missing-first" {
                            "missing"
                        } else {
                            "one"
                        },
                        if case == "same" { &a.id } else { &b.id },
                        if case == "missing-second" {
                            "missing"
                        } else {
                            "two"
                        },
                        &|| Ok(())
                    )
                    .await,
                Err(SyncError::Review)
            ),
            "{case}"
        );
        if case == "pending-first" {
            assert!(matches!(
                r.service.decide(&a.id, "one", "expense", &|| Ok(())).await,
                Err(SyncError::Review)
            ));
        }
    }
}

#[tokio::test]
async fn automatic_broker_sync_is_due_isolated_and_pauses_only_for_attention_or_review() {
    for failure in [
        None,
        Some(PlaidError::Unavailable),
        Some(PlaidError::ReconnectRequired),
        Some(PlaidError::NotReady),
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        r.service.sync(true, &|| Ok(())).await.unwrap();
        assert_eq!(r.api.calls("holdings"), 0);
        r.api.0.lock().unwrap().holdings = Some(brokerage_fixture("other", None, Value::Null));
        let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await
            .unwrap();
        let before = r.api.calls("holdings");
        r.service.sync(true, &|| Ok(())).await.unwrap();
        assert_eq!(r.api.calls("holdings"), before);
        if let Some(error) = failure {
            r.api.fail("holdings", error);
        }
        r.service.sync(false, &|| Ok(())).await.unwrap();
        let current = r.current(&m).await;
        assert_eq!(
            current.status,
            if failure == Some(PlaidError::ReconnectRequired) {
                "attention"
            } else {
                "idle"
            }
        );
        if failure == Some(PlaidError::ReconnectRequired) {
            let before = r.api.calls("holdings");
            let mut current = current;
            current.last_success = None;
            r.repo.put_mappings(vec![current]).await.unwrap();
            r.service.sync(true, &|| Ok(())).await.unwrap();
            assert_eq!(r.api.calls("holdings"), before);
        }
    }
    let r = Rig::new().await;
    let (_, mut m) = r.seed(PlaidProduct::Investments).await;
    m.status = "idle".into();
    r.repo.put_mappings(vec![m.clone()]).await.unwrap();
    r.api.0.lock().unwrap().holdings = Some(brokerage_fixture("other", None, Value::Null));
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.current(&m).await.status, "review");
    r.service.sync(true, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("holdings"), 1);
}

#[tokio::test]
async fn automatic_bank_sync_applies_known_records_and_pauses_unknown_or_reconnection() {
    for case in [
        "known",
        "unknown",
        "pending",
        "cutover",
        "attention",
        "disconnected",
        "expired-stage",
        "disabled-staged",
    ] {
        let mut r = Rig::new().await;
        let (mut c, mut m) = r.seed(PlaidProduct::Transactions).await;
        m.cursor = Some("before".into());
        m.status = "idle".into();
        if case == "cutover" {
            m.cutover_date = Some("2026-11-02".parse().unwrap());
        }
        if matches!(case, "expired-stage" | "disabled-staged") {
            staged_transfer(&mut m, "one", "10", false);
            let stage = m.staged.as_mut().unwrap();
            stage.approved = true;
            stage.fetched_at -= Duration::minutes(if case == "expired-stage" { 31 } else { 0 });
            m.decisions.insert(
                "one".into(),
                Decision {
                    kind: "expense".into(),
                    group: None,
                },
            );
            m.status = "ready".into();
            m.last_success = Some(Utc::now());
        }
        r.repo.put_mappings(vec![m.clone()]).await.unwrap();
        if case == "disconnected" {
            c.status = "disconnected".into();
            r.repo
                .put_connection(c, Some("connected".into()))
                .await
                .unwrap();
        }
        if case == "attention" {
            r.api.fail("accounts", PlaidError::ReconnectRequired);
        }
        if case == "disabled-staged" {
            r.service.institutions.clear();
        }
        let mut record = transaction(
            "one",
            "10",
            if matches!(case, "unknown" | "cutover") {
                "TRANSFER_OUT"
            } else {
                "FOOD_AND_DRINK"
            },
        );
        if case == "pending" {
            record["pending"] = json!(true);
        }
        r.api.page(json!([record]));
        r.service.sync(true, &|| Ok(())).await.unwrap();
        let current = r.current(&m).await;
        match case {
            "known" => {
                assert_eq!(current.status, "idle");
                assert_eq!(r.count("activities"), 1);
            }
            "unknown" => {
                assert_eq!(current.status, "review");
                assert_eq!(current.cursor.as_deref(), Some("before"));
            }
            "attention" => assert_eq!(current.status, "attention"),
            "pending" | "cutover" => {
                assert_eq!(current.status, "idle");
                assert_eq!(r.count("activities"), 0);
            }
            _ => {
                assert_eq!(current.cursor.as_deref(), Some("before"));
                assert_eq!(r.count("activities"), 0);
            }
        }
    }
}

#[tokio::test]
async fn holdings_resolve_equities_locally_and_reject_foreign_duplicate_or_future_prices() {
    for case in [
        "equity",
        "etf",
        "fund",
        "foreign-account",
        "foreign-institution",
        "account-currency",
        "holding-unofficial",
        "duplicate-position",
        "datetime",
        "future-datetime",
        "basis-asset",
        "basis-currency",
    ] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        let mut fixture = brokerage_fixture(
            if case == "etf" {
                "etf"
            } else if case == "fund" {
                "mutual fund"
            } else if case == "equity" {
                "equity"
            } else {
                "other"
            },
            if matches!(case, "equity" | "etf" | "fund") {
                Some("SYNTHETIC")
            } else {
                None
            },
            json!("80"),
        );
        match case {
            "foreign-account" => fixture["holdings"][0]["account_id"] = json!("foreign"),
            "foreign-institution" => fixture["item"]["institution_id"] = json!("ins_2"),
            "account-currency" => {
                fixture["accounts"][0]["balances"]["iso_currency_code"] = json!("EUR")
            }
            "holding-unofficial" => {
                fixture["holdings"][0]["unofficial_currency_code"] = json!("USD")
            }
            "duplicate-position" => {
                let h = fixture["holdings"][0].clone();
                fixture["holdings"].as_array_mut().unwrap().push(h);
            }
            "datetime" => {
                fixture["holdings"][0]["institution_price_datetime"] =
                    json!((Utc::now() - Duration::hours(1)).to_rfc3339())
            }
            "future-datetime" => {
                fixture["holdings"][0]["institution_price_datetime"] =
                    json!((Utc::now() + Duration::hours(1)).to_rfc3339())
            }
            _ => {}
        }
        r.api.0.lock().unwrap().holdings = Some(fixture.clone());
        let preview = r.service.preview_holdings(&m.id, &|| Ok(())).await;
        if matches!(case, "foreign-institution" | "account-currency") {
            assert!(matches!(preview, Err(SyncError::Review)));
            continue;
        }
        let p = preview.unwrap();
        let result = r
            .service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await;
        if matches!(
            case,
            "foreign-account" | "holding-unofficial" | "duplicate-position" | "future-datetime"
        ) {
            assert!(matches!(result, Err(SyncError::Review)), "{case}");
            assert_eq!(r.count("holdings_snapshots"), 0);
            continue;
        }
        assert!(result.is_ok(), "{case}: {result:?}");
        assert_eq!(r.count("holdings_snapshots"), 1);
        if matches!(case, "basis-asset" | "basis-currency") {
            let mut current = r.current(&m).await;
            current.source_history["basisBySecurity"]["security"][if case == "basis-asset" {
                "assetId"
            } else {
                "currency"
            }] = json!("foreign");
            r.repo.put_mappings(vec![current]).await.unwrap();
            fixture["holdings"][0]["cost_basis"] = Value::Null;
            r.api.0.lock().unwrap().holdings = Some(fixture);
            let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
            r.service
                .approve_holdings(&m.id, p["digest"].as_str().unwrap(), false, &|| Ok(()))
                .await
                .unwrap();
            assert_eq!(
                r.current(&m).await.source_history["basisBySecurity"]["security"]["status"],
                "unknown"
            );
        }
    }
}
#[tokio::test]
async fn combined_broker_source_metadata_limit_rejects_before_financial_commit() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut fixture = brokerage_fixture("other", None, json!("80"));
    for n in 1..3 {
        let mut sec = fixture["securities"][0].clone();
        sec["security_id"] = json!(format!("security-{n}"));
        fixture["securities"].as_array_mut().unwrap().push(sec);
        let mut h = fixture["holdings"][0].clone();
        h["security_id"] = json!(format!("security-{n}"));
        fixture["holdings"].as_array_mut().unwrap().push(h);
    }
    let mut tx = investment_record("one");
    tx["name"] = json!("");
    let stage = HoldingsStage {
        response: parse(fixture.clone()),
        history: vec![parse(tx.clone())],
        fetched_at: Utc::now(),
        digest: String::new(),
    };
    let overhead = serde_json::to_vec(&stage).unwrap().len();
    tx["name"] =
        json!("x".repeat(wealthfolio_connect::plaid::ingest::MAX_STAGE_BYTES - overhead - 100));
    {
        let mut state = r.api.0.lock().unwrap();
        state.holdings = Some(fixture);
        state.investments.push_back(investment_page(json!([tx]), 1));
    }
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("holdings_snapshots"), 0);
    assert_eq!(r.count("quotes"), 0);
    assert!(r.current(&m).await.last_success.is_none());
}

#[test]
fn storage_error_payloads_are_never_exposed_as_domain_messages() {
    for value in ["PLAID_CHANGED", "PLAID_REVIEW", "private-source-payload"] {
        let result = storage(Error::ConstraintViolation(value.into()));
        assert_eq!(
            result,
            match value {
                "PLAID_CHANGED" => SyncError::Changed,
                "PLAID_REVIEW" => SyncError::Review,
                _ => SyncError::Storage,
            }
        );
        assert!(!result.to_string().contains(value));
    }
    assert!(decode::<Mapping>(Body { body: "{".into() }).is_err());
}

#[tokio::test]
async fn late_us_broker_quote_is_available_to_snapshot_day_valuation() {
    use wealthfolio_core::{
        assets::{AssetKind, AssetSpec, QuoteMode},
        portfolio::snapshot::{Position, SnapshotSource},
        quotes::{store::QuoteStore, Quote},
    };
    let r = Rig::new().await;
    let (_, mut m) = r.seed(PlaidProduct::Investments).await;
    let fetched = chrono::DateTime::parse_from_rfc3339("2026-11-06T02:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let timezone = chrono::FixedOffset::west_opt(8 * 3600).unwrap();
    let day = fetched.with_timezone(&timezone).date_naive();
    let ensured = r
        .service
        .assets
        .ensure_assets(
            vec![AssetSpec {
                id: Some(uuid::Uuid::new_v4().to_string()),
                display_code: Some("SYNTHETIC".into()),
                instrument_symbol: None,
                instrument_exchange_mic: None,
                instrument_type: None,
                quote_ccy: "USD".into(),
                requested_quote_ccy: None,
                kind: AssetKind::Investment,
                quote_mode: Some(QuoteMode::Manual),
                name: Some("Synthetic".into()),
                provider_config: None,
                provider_id: None,
                provider_symbol: None,
                metadata: None,
            }],
            r.service.activities.as_ref(),
        )
        .await
        .unwrap();
    let asset = ensured.assets.keys().next().unwrap().clone();
    let holding =
        parse::<Holding>(brokerage_fixture("other", None, Value::Null)["holdings"][0].clone());
    let timestamp =
        wealthfolio_connect::plaid::ingest::holdings_quote_timestamp(&holding, fetched, &timezone)
            .unwrap();
    let price = "40".parse::<rust_decimal::Decimal>().unwrap();
    let mut position = Position::new(m.account_id.clone(), asset.clone(), "USD".into(), fetched);
    position.quantity = "2.5".parse().unwrap();
    let snapshot = AccountStateSnapshot {
        id: AccountStateSnapshot::stable_id(&m.account_id, day),
        account_id: m.account_id.clone(),
        snapshot_date: day,
        currency: "USD".into(),
        positions: HashMap::from([(asset.clone(), position.clone())]),
        cash_balances: HashMap::from([("USD".into(), rust_decimal::Decimal::from(150))]),
        cash_total_account_currency: rust_decimal::Decimal::from(150),
        source: SnapshotSource::BrokerImported,
        calculated_at: fetched.naive_utc(),
        ..Default::default()
    };
    let quote = Quote {
        id: format!("{asset}_{}_BROKER", timestamp.date_naive()),
        asset_id: asset.clone(),
        timestamp,
        open: price,
        high: price,
        low: price,
        close: price,
        adjclose: price,
        volume: rust_decimal::Decimal::ZERO,
        currency: "USD".into(),
        data_source: "BROKER".into(),
        created_at: fetched,
        notes: None,
    };
    m.source_history = json!({"holdings":[holding],"asOf":fetched});
    r.repo
        .commit_holdings(m.clone(), snapshot, vec![quote], Default::default())
        .await
        .unwrap();
    let store = crate::market_data::MarketDataRepository::new(r.pool.clone(), r.writer.clone());
    let quotes = store
        .get_latest_quotes_as_of(std::slice::from_ref(&asset), day)
        .unwrap();
    let quote = quotes
        .get(&asset)
        .expect("current valuation must find the broker quote on its snapshot day");
    assert_eq!(
        position.quantity * quote.close + rust_decimal::Decimal::from(150),
        rust_decimal::Decimal::from(250)
    );
    assert_eq!(quote.timestamp.date_naive(), day);
    assert_eq!(r.current(&m).await.source_history["asOf"], json!(fetched));
}

#[tokio::test]
async fn plaid_cutover_crossing_correction_reconciles_or_pauses_until_owner_resolves() {
    for protection in ["none", "edited", "paired"] {
        let r = Rig::new().await;
        let (_, mut m) = r.seed(PlaidProduct::Transactions).await;
        m.cutover_date = Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        r.repo.put_mappings(vec![m.clone()]).await.unwrap();
        let mut tx = transaction("one", "10", "FOOD_AND_DRINK");
        tx["date"] = json!("2026-11-03");
        let p = r.preview(&m, json!([tx.clone()])).await;
        r.service
            .approve(&m.id, p["digest"].as_str().unwrap(), None, &|| Ok(()))
            .await
            .unwrap();
        let before = r.current(&m).await;
        assert_eq!(r.count("activities"), 1);
        if protection != "none" {
            diesel::sql_query(if protection == "edited" {
                "UPDATE activities SET is_user_modified=1"
            } else {
                "UPDATE activities SET source_group_id='paired-group'"
            })
            .execute(&mut r.pool.get().unwrap())
            .unwrap();
        }
        tx["date"] = json!("2026-11-01");
        tx["amount"] = json!("12");
        r.api.0.lock().unwrap().bank.push_back(Ok(json!({"added":[], "modified":[tx], "removed":[], "next_cursor":"cursor-2", "has_more":false, "transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"})));
        r.service.sync(false, &|| Ok(())).await.unwrap();
        if protection != "none" {
            let blocked = r.current(&m).await;
            assert_eq!(blocked.status, "review");
            assert_eq!(blocked.cursor, before.cursor);
            assert_eq!(blocked.source_history, before.source_history);
            assert_eq!(r.count("activities"), 1);
            assert_eq!(r.count("import_runs"), 1);
            let preview = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
            assert_eq!(preview["removedCount"], 1);
            assert_eq!(
                preview["removedRecords"][0]["activityId"],
                wealthfolio_connect::plaid::ingest::stable_id(&m.id, "one")
            );
            // Existing activity editor explicitly resolves the protected old row.
            diesel::sql_query("DELETE FROM activities")
                .execute(&mut r.pool.get().unwrap())
                .unwrap();
            r.service
                .approve(&m.id, preview["digest"].as_str().unwrap(), None, &|| Ok(()))
                .await
                .unwrap();
        }
        let after = r.current(&m).await;
        assert_eq!(after.status, "idle");
        assert_eq!(after.cursor.as_deref(), Some("cursor-2"));
        assert_eq!(after.source_history["one"]["date"], "2026-11-01");
        assert_eq!(
            parse::<BankTransaction>(after.source_history["one"].clone()).amount,
            "12".parse().unwrap()
        );
        assert_eq!(r.count("activities"), 0);
        assert_eq!(r.count("import_runs"), 2);
    }
}

#[tokio::test]
async fn plaid_pair_can_be_undone_atomically_then_both_accounts_can_complete_review() {
    let r = Rig::new().await;
    let (_, a) = r.seed(PlaidProduct::Transactions).await;
    let mut b = a.clone();
    b.id = uuid::Uuid::new_v4().to_string();
    b.account_id = uuid::Uuid::new_v4().to_string();
    b.external_id = "other".into();
    let account_id = b.account_id.clone();
    r.writer.exec(move |conn| {
        diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Other','CASH','USD',0,1,0,'TRANSACTIONS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
            .bind::<Text,_>(account_id).execute(conn).map_err(sql)?;
        Ok(())
    }).await.unwrap();
    r.repo
        .add_mapping(b.clone(), PlaidProduct::Transactions)
        .await
        .unwrap();
    r.api.0.lock().unwrap().accounts["accounts"]
        .as_array_mut()
        .unwrap()
        .push(account_fixture_for_other());
    r.preview(&a, json!([transaction("out", "10", "TRANSFER_OUT")]))
        .await;
    let mut tx = transaction("in", "-10", "TRANSFER_IN");
    tx["account_id"] = json!("other");
    r.preview(&b, json!([tx])).await;
    r.service
        .pair(&a.id, "out", &b.id, "in", &|| Ok(()))
        .await
        .unwrap();
    assert!(matches!(
        r.service.pair(&a.id, "out", &b.id, "in", &|| Ok(())).await,
        Err(SyncError::Review)
    ));
    let stale = r.service.preview(&b.id, false, &|| Ok(())).await.unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    r.service
        .decide(&a.id, "out", "external", &|| Ok(()))
        .await
        .unwrap();
    let a_now = r.current(&a).await;
    let b_now = r.current(&b).await;
    assert!(a_now.decisions["out"].group.is_none());
    assert!(!b_now.decisions.contains_key("in"));
    assert!(!b_now.staged.as_ref().unwrap().approved);
    assert_ne!(b_now.staged.as_ref().unwrap().digest, stale);
    assert_eq!(b_now.status, "review");
    r.service
        .decide(&b.id, "in", "external", &|| Ok(()))
        .await
        .unwrap();
    for (m, opening) in [(&a, "260"), (&b, "240")] {
        let p = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
        r.service
            .approve(
                &m.id,
                p["digest"].as_str().unwrap(),
                Some(opening.into()),
                &|| Ok(()),
            )
            .await
            .unwrap();
    }
    assert_eq!(r.count("activities"), 4);
    assert!(r.current(&a).await.cursor.is_some());
    assert!(r.current(&b).await.cursor.is_some());
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type=diesel::sql_types::BigInt)]
        n: i64,
    }
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS n FROM activities WHERE source_group_id IS NOT NULL")
            .get_result::<Count>(&mut r.pool.get().unwrap())
            .unwrap()
            .n,
        0
    );
}

#[tokio::test]
async fn plaid_basis_knowledge_survives_normalized_batch_and_json_snapshot_reads() {
    use wealthfolio_core::lots::LotRepositoryTrait;
    use wealthfolio_core::portfolio::{
        economic_events::BasisStatus, snapshot::SnapshotRepositoryTrait,
    };
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let repo =
        crate::portfolio::snapshot::SnapshotRepository::new(r.pool.clone(), r.writer.clone());
    let lots = crate::lots::LotsRepository::new(r.pool.clone(), r.writer.clone());
    let mut fixture = brokerage_fixture("other", None, Value::Null);
    for (basis, quantity, expected) in [
        (Value::Null, "2.5", BasisStatus::Unknown),
        (json!("0"), "2.5", BasisStatus::Complete),
        (json!("80"), "2.5", BasisStatus::Complete),
        (Value::Null, "2.5", BasisStatus::Complete),
        (Value::Null, "5", BasisStatus::Unknown),
    ] {
        fixture["holdings"][0]["cost_basis"] = basis;
        fixture["holdings"][0]["quantity"] = json!(quantity);
        r.api.0.lock().unwrap().holdings = Some(fixture.clone());
        let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
        r.service
            .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
            .await
            .unwrap();
        let snapshot = repo
            .get_all_latest_snapshots(std::slice::from_ref(&m.account_id))
            .unwrap()
            .remove(&m.account_id)
            .unwrap();
        let id = snapshot.id.clone();
        let asset_id = snapshot.positions.keys().next().unwrap();
        let lot_view = lots.get_asset_lot_view(asset_id, true).await.unwrap();
        assert_eq!(lot_view.len(), 1);
        assert_eq!(lot_view[0].cost_basis_status, Some(expected));
        assert_eq!(lot_view[0].quantity, quantity.parse().unwrap());
        assert_eq!(
            snapshot
                .positions
                .values()
                .next()
                .unwrap()
                .cost_basis_status,
            Some(expected)
        );
        for positions in [
            repo.get_snapshot_positions(&id).unwrap(),
            repo.get_snapshot_positions_batch(std::slice::from_ref(&id))
                .unwrap()
                .remove(&id)
                .unwrap(),
        ] {
            let pos = positions.values().next().unwrap();
            assert_eq!(pos.basis_status(), expected);
            assert_eq!(
                pos.known_cost_basis().is_some(),
                expected == BasisStatus::Complete
            );
        }
        diesel::sql_query("DELETE FROM snapshot_positions WHERE snapshot_id=?")
            .bind::<Text, _>(&id)
            .execute(&mut r.pool.get().unwrap())
            .unwrap();
        let fallback = repo.get_snapshot_positions(&id).unwrap();
        let lot_fallback = lots.get_asset_lot_view(asset_id, true).await.unwrap();
        assert_eq!(lot_fallback[0].cost_basis_status, Some(expected));
        assert_eq!(lot_fallback[0].quantity, quantity.parse().unwrap());
        assert_eq!(
            fallback.values().next().unwrap().cost_basis_status,
            Some(expected)
        );
        assert_eq!(
            repo.get_snapshot_positions_batch(std::slice::from_ref(&id))
                .unwrap()[&id]
                .values()
                .next()
                .unwrap()
                .cost_basis_status,
            Some(expected)
        );
    }
}

#[tokio::test]
async fn plaid_account_picker_distinguishes_empty_provider_results_from_unsupported_accounts() {
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Investments).await;
    for accounts in [
        json!([]),
        json!([account("depository")]),
        json!([account("investment"), account("credit")]),
    ] {
        r.api.0.lock().unwrap().accounts["accounts"] = accounts.clone();
        let result = r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
        assert_eq!(result["totalAccounts"], accounts.as_array().unwrap().len());
        let eligible = result["accounts"].as_array().unwrap();
        if accounts.as_array().unwrap().len() == 2 {
            assert_eq!(result["accountTypes"], json!(["credit", "investment"]));
            assert_eq!(eligible.len(), 2);
            assert_eq!(eligible[1]["product"], "transactions");
            assert_eq!(eligible[0]["id"], "external");
            assert_eq!(eligible[0]["type"], "investment");
            assert_eq!(eligible[0]["currency"], "USD");
        } else {
            assert!(eligible.is_empty());
            assert_eq!(
                result,
                if accounts.as_array().unwrap().is_empty() {
                    json!({"accounts":[], "totalAccounts":0, "accountTypes":[], "accountClassifications":[]})
                } else {
                    json!({"accounts":[], "totalAccounts":1, "accountTypes":["depository"], "accountClassifications":[{"type":"depository","subtype":"checking"}]})
                }
            );
        }
    }
    assert_eq!(r.count("plaid_accounts"), 1);
    assert_eq!(r.count("activities"), 0);
}

#[tokio::test]
async fn plaid_unused_disconnected_connection_removal_preserves_local_accounts_and_history() {
    let r = Rig::new().await;
    let (active, mapping) = r.seed(PlaidProduct::Transactions).await;
    let preview = r
        .preview(
            &mapping,
            json!([transaction("retained", "10", "FOOD_AND_DRINK")]),
        )
        .await;
    r.service
        .approve(
            &mapping.id,
            preview["digest"].as_str().unwrap(),
            Some("260".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    let activity_count = r.count("activities");
    assert!(activity_count > 0);
    let mut unused = r.connection(PlaidProduct::Investments);
    unused.status = "disconnected".into();
    unused.item_id = Some("unused-item".into());
    r.repo.put_connection(unused.clone(), None).await.unwrap();
    let status = r.service.list(&|| Ok(())).await.unwrap();
    let connections = status["connections"].as_array().unwrap();
    assert_eq!(
        connections.iter().find(|c| c["id"] == unused.id).unwrap()["canRemove"],
        true
    );
    assert_eq!(
        connections.iter().find(|c| c["id"] == active.id).unwrap()["canRemove"],
        false
    );
    let calls = r.api.0.lock().unwrap().calls.len();
    r.service
        .remove_connection(&unused.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.api.0.lock().unwrap().calls.len(), calls);
    assert_eq!(r.count("plaid_connections"), 1);
    assert_eq!(r.count("accounts"), 1);
    assert_eq!(r.count("activities"), activity_count);
    assert_eq!(r.repo.mappings().await.unwrap()[0].id, mapping.id);
    assert!(matches!(
        r.service.remove_connection(&unused.id, &|| Ok(())).await,
        Err(SyncError::Access)
    ));
}

#[tokio::test]
async fn plaid_connection_removal_cannot_bypass_revocation_ownership_or_mapping_guards() {
    for case in [
        "connected",
        "pending_revocation",
        "mapped",
        "access",
        "link",
        "revocation",
        "vault",
        "foreign",
        "admission",
        "expired-admission",
    ] {
        let r = Rig::new().await;
        let (mut c, _) = r.seed(PlaidProduct::Transactions).await;
        if case != "mapped" {
            // Isolate an unused record without changing the retained seeded account.
            c.id = uuid::Uuid::new_v4().to_string();
            c.item_id = Some("unused-item".into());
        }
        c.status = if matches!(case, "connected" | "pending_revocation") {
            case
        } else {
            "disconnected"
        }
        .into();
        if case == "foreign" {
            c.environment = PlaidEnvironment::Production;
        }
        r.repo
            .put_connection(
                c.clone(),
                if case == "mapped" {
                    Some("connected".into())
                } else {
                    None
                },
            )
            .await
            .unwrap();
        if matches!(case, "access" | "link" | "revocation") {
            r.vault
                .set_secret(&secret_key(&c, case).unwrap(), "fixture")
                .unwrap();
        }
        if case == "vault" {
            *r.vault.fail.lock().unwrap() = Some("get");
        }
        if case == "mapped" {
            r.vault
                .delete_secret(&secret_key(&c, "access").unwrap())
                .unwrap();
        }
        if case == "connected" {
            let status = r.service.list(&|| Ok(())).await.unwrap();
            let connection = status["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == c.id)
                .unwrap();
            assert_eq!(connection["status"], "disconnected");
            assert_eq!(connection["canRemove"], false);
        }
        let calls = r.api.0.lock().unwrap().calls.len();
        let admission_calls = std::sync::atomic::AtomicUsize::new(0);
        let result = r
            .service
            .remove_connection(&c.id, &|| {
                let n = admission_calls.fetch_add(1, Ordering::SeqCst);
                if case == "admission" || (case == "expired-admission" && n > 0) {
                    Err(SyncError::Access)
                } else {
                    Ok(())
                }
            })
            .await;
        assert!(result.is_err(), "{case}");
        assert_eq!(r.api.0.lock().unwrap().calls.len(), calls, "{case}");
        assert!(
            r.repo
                .connections()
                .await
                .unwrap()
                .iter()
                .any(|row| row.id == c.id),
            "{case}"
        );
        assert_eq!(r.count("plaid_accounts"), 1, "{case}");
        assert_eq!(r.count("accounts"), 1, "{case}");
    }
}

#[tokio::test]
async fn plaid_connection_removal_rechecks_persisted_identity_and_status() {
    let r = Rig::new().await;
    let mut c = r.connection(PlaidProduct::Investments);
    c.status = "disconnected".into();
    r.repo.put_connection(c.clone(), None).await.unwrap();
    for case in ["owner", "environment", "item", "status"] {
        let mut stale = c.clone();
        match case {
            "owner" => stale.owner = "other".into(),
            "environment" => stale.environment = PlaidEnvironment::Production,
            "item" => stale.item_id = Some("other".into()),
            "status" => {
                let mut reconnected = c.clone();
                reconnected.status = "connected".into();
                r.repo
                    .put_connection(reconnected, Some("disconnected".into()))
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                r.repo.remove_connection(stale).await,
                Err(SyncError::Changed)
            ),
            "{case}"
        );
        assert_eq!(r.count("plaid_connections"), 1);
    }
}

#[tokio::test]
async fn plaid_merrill_cash_management_maps_and_imports_as_reviewed_brokerage_holdings() {
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Investments).await;
    let mut external = account("depository");
    external["account_id"] = json!("cma");
    external["subtype"] = json!("cash management");
    r.api.0.lock().unwrap().accounts["accounts"] = json!([external.clone()]);
    let list = r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
    assert_eq!(list["accounts"][0]["id"], "cma");
    assert_eq!(
        list["accountClassifications"],
        json!([{"type":"depository","subtype":"cash management"}])
    );
    let local = uuid::Uuid::new_v4().to_string();
    let id = local.clone();
    r.writer.exec(move |conn| {
        diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Synthetic CMA','SECURITIES','USD',0,1,0,'HOLDINGS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
            .bind::<Text,_>(id).execute(conn).map_err(sql)?;
        Ok(())
    }).await.unwrap();
    assert!(matches!(
        r.service
            .map_account(&c.id, "cma", &local, false, None, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));
    r.service
        .map_account(&c.id, "cma", &local, true, None, &|| Ok(()))
        .await
        .unwrap();
    let m = r
        .repo
        .mappings()
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.external_id == "cma")
        .unwrap();
    assert!(!m.credit_card);
    let holding_calls = r.api.calls("holdings");
    r.service.sync(true, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("holdings"), holding_calls);
    assert_eq!(r.count("holdings_snapshots"), 0);
    let mut holdings = brokerage_fixture("other", None, json!("80"));
    holdings["accounts"] = json!([external.clone()]);
    holdings["holdings"][0]["account_id"] = json!("cma");
    r.api.0.lock().unwrap().holdings = Some(holdings);
    r.api.0.lock().unwrap().investments.push_back(json!({"accounts":[external],"item":item(),"investment_transactions":[],"securities":[],"total_investment_transactions":0}));
    let preview = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert_eq!(preview["holdings"][0]["quantity"], json!(2.5));
    assert_eq!(r.count("holdings_snapshots"), 0);
    r.service
        .approve_holdings(&m.id, preview["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("holdings_snapshots"), 1);
    assert_eq!(r.count("activities"), 0);
    #[derive(QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type=Text)]
        cash: String,
        #[diesel(sql_type=Text)]
        positions: String,
    }
    let snapshot = diesel::sql_query(
        "SELECT cash_total_account_currency AS cash,positions FROM holdings_snapshots",
    )
    .get_result::<Snapshot>(&mut r.pool.get().unwrap())
    .unwrap();
    // Preserve the existing Investments value reconciliation: source total 250,
    // securities 100, residual cash 150; buying power (9999) is never cash.
    assert_eq!(
        snapshot.cash.parse::<rust_decimal::Decimal>().unwrap(),
        rust_decimal::Decimal::from(150)
    );
    let positions: HashMap<String, wealthfolio_core::portfolio::snapshot::Position> =
        serde_json::from_str(&snapshot.positions).unwrap();
    assert_eq!(
        positions.values().next().unwrap().quantity.to_string(),
        "2.5"
    );
    assert_eq!(r.current(&m).await.status, "idle");
}

#[tokio::test]
async fn plaid_merrill_shared_item_cards_use_reviewed_bank_import_and_keep_brokerage_mapping() {
    for amount in ["20", "-15"] {
        let r = Rig::new().await;
        let (c, brokerage) = r.seed(PlaidProduct::Investments).await;
        let mut card = account("credit");
        card["account_id"] = json!("card");
        r.api.0.lock().unwrap().accounts["accounts"] = json!([account("investment"), card]);
        let discovered = r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
        assert_eq!(discovered["accounts"][0]["product"], "investments");
        assert_eq!(discovered["accounts"][1]["product"], "transactions");
        let local = uuid::Uuid::new_v4().to_string();
        let id = local.clone();
        r.writer.exec(move |conn| {
            diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Synthetic card','CREDIT_CARD','USD',0,1,0,'TRANSACTIONS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)")
                .bind::<Text,_>(id).execute(conn).map_err(sql)?;
            Ok(())
        }).await.unwrap();
        r.service
            .map_account(&c.id, "card", &local, false, None, &|| Ok(()))
            .await
            .unwrap();
        let mappings = r.repo.mappings().await.unwrap();
        let m = mappings.iter().find(|m| m.external_id == "card").unwrap();
        assert!(m.credit_card);
        assert_eq!(m.product(&c), PlaidProduct::Transactions);
        assert_eq!(r.count("plaid_connections"), 1);
        assert_eq!(r.api.calls("exchange"), 0);
        assert_eq!(r.current(&brokerage).await.status, "initial_review");
        assert!(r.service.preview_holdings(&m.id, &|| Ok(())).await.is_err());
        assert!(r
            .service
            .preview(&brokerage.id, false, &|| Ok(()))
            .await
            .is_err());
        let mut tx = transaction("purchase", amount, "GENERAL_MERCHANDISE");
        tx["account_id"] = json!("card");
        r.api.page(json!([tx]));
        let preview = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
        assert_eq!(preview["balance"], "-250");
        assert_eq!(r.count("activities"), 0);
        // Any ambiguous classification remains an explicit owner decision.
        if !preview["reviewRequired"].as_array().unwrap().is_empty() {
            r.service
                .decide(
                    &m.id,
                    "purchase",
                    if amount == "20" { "expense" } else { "refund" },
                    &|| Ok(()),
                )
                .await
                .unwrap();
        }
        let preview = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
        r.service
            .approve(
                &m.id,
                preview["digest"].as_str().unwrap(),
                preview["suggestedOpening"].as_str().map(str::to_owned),
                &|| Ok(()),
            )
            .await
            .unwrap();
        assert!(r.count("activities") > 0);
        assert_eq!(r.current(m).await.cursor.as_deref(), Some("cursor-1"));
        r.service.sync(false, &|| Ok(())).await.unwrap();
        let requests = r.api.0.lock().unwrap().transaction_requests.clone();
        assert_eq!(requests[0], ("card".into(), None));
        assert_eq!(requests[1], ("card".into(), Some("cursor-1".into())));
        assert_eq!(r.api.calls("holdings"), 0);
        assert_eq!(r.count("holdings_snapshots"), 0);
        assert_eq!(r.current(&brokerage).await.cursor, None);
        assert_eq!(r.current(&brokerage).await.status, "initial_review");
        let status = r.service.list(&|| Ok(())).await.unwrap();
        let accounts = status["connections"][0]["accounts"].as_array().unwrap();
        assert_eq!(
            accounts.iter().find(|a| a["id"] == m.id).unwrap()["product"],
            "transactions"
        );
        // Once the brokerage's own review is approved, one sync updates both
        // products without routing a card through Holdings or sharing cursors.
        let holdings = r
            .service
            .preview_holdings(&brokerage.id, &|| Ok(()))
            .await
            .unwrap();
        r.service
            .approve_holdings(
                &brokerage.id,
                holdings["digest"].as_str().unwrap(),
                false,
                &|| Ok(()),
            )
            .await
            .unwrap();
        let holding_calls = r.api.calls("holdings");
        let transaction_calls = r.api.calls("transactions");
        r.service.sync(false, &|| Ok(())).await.unwrap();
        assert_eq!(r.api.calls("holdings"), holding_calls + 1);
        assert_eq!(r.api.calls("transactions"), transaction_calls + 1);
        assert_eq!(r.current(&brokerage).await.cursor, None);
        assert_eq!(r.current(&brokerage).await.status, "idle");
        assert_eq!(r.count("plaid_connections"), 1);
        assert_eq!(r.api.calls("exchange"), 0);
    }
}

#[tokio::test]
async fn plaid_merrill_update_requests_transactions_consent_without_exchanging_item() {
    let r = Rig::new().await;
    let (c, _) = r.seed(PlaidProduct::Investments).await;
    let result = r
        .service
        .start("owner", "merrill_edge", Some(&c.id), false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(result["flowId"], c.id);
    assert_eq!(r.api.0.lock().unwrap().link_transactions, vec![true]);
    r.service.finish("owner", &c.id, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("exchange"), 0);
    assert_eq!(r.repo.connections().await.unwrap()[0].item_id, c.item_id);
    assert_eq!(r.count("plaid_connections"), 1);
}

#[tokio::test]
async fn plaid_completed_empty_card_history_imports_opening_once_then_advances_cursor() {
    let r = Rig::new().await;
    let (_, m) = r.seed_kind(PlaidProduct::Transactions, true).await;
    r.api.0.lock().unwrap().accounts["accounts"] = json!([account("credit")]);
    let empty = json!({"added":[],"modified":[],"removed":[],"next_cursor":"","has_more":false,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"});
    r.api.0.lock().unwrap().bank.push_back(Ok(empty.clone()));
    let preview = r.service.preview(&m.id, false, &|| Ok(())).await.unwrap();
    assert_eq!(preview["initial"], true);
    assert_eq!(preview["suggestedOpening"], "-250");
    r.service
        .approve(
            &m.id,
            preview["digest"].as_str().unwrap(),
            Some("-250".into()),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 1);
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some(""));
    r.api.0.lock().unwrap().bank.push_back(Ok(empty));
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.count("activities"), 1);
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some(""));
    assert_eq!(
        r.api
            .0
            .lock()
            .unwrap()
            .transaction_requests
            .last()
            .unwrap()
            .1
            .as_deref(),
        Some("")
    );
    let mut tx = transaction("new-purchase", "20", "FOOD_AND_DRINK");
    tx["date"] = json!(Utc::now().date_naive().to_string());
    r.api.page(json!([tx]));
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.current(&m).await.cursor.as_deref(), Some("cursor-1"));
    assert_eq!(r.count("activities"), 2);
    assert_eq!(r.current(&m).await.status, "idle");
}

#[tokio::test]
async fn plaid_empty_cursor_does_not_accept_incomplete_or_uncheckpointed_changes() {
    for case in [
        "missing-status",
        "partial",
        "more",
        "added",
        "modified",
        "removed",
        "after-cursor",
        "after-page",
    ] {
        let r = Rig::new().await;
        let (_, mut m) = r.seed(PlaidProduct::Transactions).await;
        let mut page = json!({"added":[],"modified":[],"removed":[],"next_cursor":"","has_more":false,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"});
        match case {
            "missing-status" => {
                page.as_object_mut()
                    .unwrap()
                    .remove("transactions_update_status");
            }
            "partial" => page["transactions_update_status"] = json!("INITIAL_UPDATE_COMPLETE"),
            "more" => page["has_more"] = json!(true),
            "added" => page["added"] = json!([transaction("one", "20", "FOOD_AND_DRINK")]),
            "modified" => page["modified"] = json!([transaction("one", "20", "FOOD_AND_DRINK")]),
            "removed" => {
                page["removed"] = json!([{"account_id":"external","transaction_id":"one"}])
            }
            "after-cursor" => {
                m.cursor = Some("saved".into());
                r.repo.put_mappings(vec![m.clone()]).await.unwrap();
            }
            _ => {
                r.api.0.lock().unwrap().bank.push_back(Ok(json!({"added":[],"modified":[],"removed":[],"next_cursor":"page-1","has_more":true,"transactions_update_status":"HISTORICAL_UPDATE_COMPLETE"})));
            }
        }
        r.api.0.lock().unwrap().bank.push_back(Ok(page));
        assert!(
            matches!(
                r.service.preview(&m.id, false, &|| Ok(())).await,
                Err(SyncError::NotReady)
            ),
            "{case}"
        );
        assert_eq!(r.current(&m).await.cursor, m.cursor);
        assert_eq!(r.count("activities"), 0);
    }
}

fn brokerage_trade(
    id: &str,
    kind: &str,
    quantity: &str,
    price: &str,
    amount: &str,
    fees: &str,
) -> Value {
    json!({"investment_transaction_id":id,"account_id":"external","security_id":"security","date":"2026-11-01","name":"Synthetic trade","type":kind,"subtype":kind,"quantity":quantity,"price":price,"amount":amount,"fees":fees,"iso_currency_code":"USD"})
}
fn brokerage_ledger_page(records: Value) -> Value {
    json!({"accounts":[account("investment")],"item":item(),"securities":[{"security_id":"security","ticker_symbol":"TEST","name":"Synthetic security","type":"equity","iso_currency_code":"USD"}],"total_investment_transactions":records.as_array().unwrap().len(),"investment_transactions":records})
}
fn set_brokerage(r: &Rig, quantity: &str, value: &str, basis: Value, records: Value) {
    let mut holdings = brokerage_fixture("equity", Some("TEST"), basis);
    holdings["holdings"][0]["quantity"] = json!(quantity);
    holdings["holdings"][0]["institution_value"] = json!(value);
    holdings["holdings"][0]["institution_price"] = if quantity == "0" {
        json!("10")
    } else {
        json!((value.parse::<rust_decimal::Decimal>().unwrap()
            / quantity.parse::<rust_decimal::Decimal>().unwrap())
        .to_string())
    };
    r.api.0.lock().unwrap().holdings = Some(holdings);
    r.api
        .0
        .lock()
        .unwrap()
        .investments
        .push_back(brokerage_ledger_page(records));
}
#[tokio::test]
async fn plaid_brokerage_import_is_reviewed_reconciled_atomic_and_idempotent() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let records = json!([brokerage_trade("buy", "buy", "2", "10", "21", "1")]);
    set_brokerage(&r, "2", "20", json!("21"), records.clone());
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(p["issues"], json!([]));
    assert_eq!(p["openingCash"], "251");
    assert_eq!(r.count("activities"), 0);
    assert!(matches!(
        r.service
            .approve_investments(
                &m.id,
                p["digest"].as_str().unwrap(),
                BTreeMap::new(),
                false,
                &|| Ok(())
            )
            .await,
        Err(SyncError::Review)
    ));
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
    assert!(r.current(&m).await.investment_ledger.is_some());
    #[derive(QueryableByName)]
    struct Mode {
        #[diesel(sql_type=Text)]
        tracking_mode: String,
    }
    assert_eq!(
        diesel::sql_query("SELECT tracking_mode FROM accounts WHERE id=?")
            .bind::<Text, _>(&m.account_id)
            .get_result::<Mode>(&mut r.pool.get().unwrap())
            .unwrap()
            .tracking_mode,
        "TRANSACTIONS"
    );
    assert!(r.service.preview_holdings(&m.id, &|| Ok(())).await.is_err());
    set_brokerage(&r, "2", "20", json!("21"), records);
    let result = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(result["failures"], json!([]));
    assert_eq!(r.count("activities"), 2);
    assert_eq!(r.current(&m).await.status, "idle");
}
#[tokio::test]
async fn plaid_reviewed_brokerage_history_drives_daily_net_worth_without_snapshot_double_counting()
{
    assert_plaid_calculated_history(false).await;
}

#[tokio::test]
async fn plaid_statement_prefix_cuts_over_to_reviewed_transfers_without_inferred_cost_or_double_counting(
) {
    assert_plaid_calculated_history(true).await;
}

async fn assert_plaid_calculated_history(statement_transfers: bool) {
    use rust_decimal::Decimal;
    use wealthfolio_core::{
        fx::{FxService, FxServiceTrait},
        portfolio::{
            coordinator::{
                CoordinatorDeps, FactSources, PortfolioCoordinator, PortfolioJobRequest,
                SilentObserver, WindowCadence,
            },
            net_worth::{NetWorthService, NetWorthServiceTrait},
            snapshot::{SnapshotService, SnapshotServiceTrait, SnapshotSource},
            statements::{
                StatementPosition, StatementRepository, StatementService, StatementSnapshot,
                StatementWrite,
            },
            valuation::{ValuationRepositoryTrait, ValuationStatus},
        },
        quotes::{store::QuoteStore, MarketSyncMode, Quote},
    };

    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let timezone = r.service.timezone.clone();
    let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
        &timezone.read().unwrap(),
    );
    // Relative dates keep the provider's available-history checks valid on future runs.
    let today = Utc::now().with_timezone(&tz).date_naive();
    let days: Vec<_> = (0..5)
        .map(|offset| today - Duration::days(4 - offset))
        .collect();
    let mut buy = brokerage_trade("buy", "buy", "2", "20", "41", "1");
    buy["date"] = json!(days[1]);
    let mut sell = brokerage_trade("sell", "sell", "-1", "30", "-29", "1");
    sell["date"] = json!(days[3]);
    let dividend = json!({"investment_transaction_id":"dividend","account_id":"external",
        "security_id":"security","date":days[4],"name":"Synthetic dividend",
        "type":"cash","subtype":"dividend","quantity":"0","price":"0",
        "amount":"-2","fees":"0","iso_currency_code":"USD"});
    let records = if statement_transfers {
        let mut incoming = brokerage_trade("in", "transfer", "5", "1000", "-5000", "1");
        incoming["subtype"] = json!("transfer");
        incoming["date"] = json!(days[1]);
        let mut outgoing = brokerage_trade("out", "transfer", "-2", "1000", "2000", "1");
        outgoing["subtype"] = json!("transfer");
        outgoing["date"] = json!(days[3]);
        json!([incoming, outgoing])
    } else {
        json!([buy, sell, dividend])
    };
    let stage_broker = || {
        let (quantity, value, basis, total) = if statement_transfers {
            ("3", "84", "12", "182")
        } else {
            ("2", "56", "41", "146")
        };
        set_brokerage(&r, quantity, value, json!(basis), records.clone());
        r.api.0.lock().unwrap().holdings.as_mut().unwrap()["accounts"][0]["balances"]["current"] =
            json!(total);
    };
    let statements = Arc::new(crate::portfolio::statements::StatementStore::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let prefix_start = days[0] - Duration::days(40);
    if statement_transfers {
        let statement_service = StatementService::new(
            statements.clone(),
            Arc::new(crate::accounts::AccountRepository::new(
                r.pool.clone(),
                r.writer.clone(),
            )),
            Arc::new(crate::portfolio::valuation::ValuationRepository::new(
                r.pool.clone(),
                r.writer.clone(),
            )),
            timezone.clone(),
        );
        statement_service
            .save(
                m.account_id.clone(),
                [
                    (prefix_start, "80"),
                    (days[0].pred_opt().unwrap(), "90"),
                    (days[1], "9999"),
                ]
                .into_iter()
                .map(|(date, total)| StatementWrite {
                    snapshot: StatementSnapshot {
                        date,
                        currency: "USD".into(),
                        total_value: total.into(),
                        cash_value: Some("10".into()),
                        positions: vec![StatementPosition {
                            label: "Synthetic aggregate basis".into(),
                            quantity: "5".into(),
                            market_value: None,
                            cost_basis: Some("777".into()),
                        }],
                        positions_complete: false,
                        format: "merrill-cma-monthly-v1".into(),
                        source_page: 1,
                    },
                    expected_revision: None,
                })
                .collect(),
                true,
            )
            .await
            .unwrap();
        assert_eq!(r.count("activities"), 0);
        assert!(r.current(&m).await.investment_ledger.is_none());
    }
    let original_statements =
        serde_json::to_value(statements.list(&m.account_id).unwrap()).unwrap();
    stage_broker();

    // Start with a real imported broker snapshot, then opt into ledger tracking.
    let holdings = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, holdings["digest"].as_str().unwrap(), false, &|| {
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("holdings_snapshots"), 1);
    r.events.0.lock().unwrap().clear();
    stage_broker();
    let preview = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(preview["issues"], json!([]));
    assert_eq!(preview["openingCash"], "100");
    if statement_transfers {
        assert_eq!(preview["openingPositions"], json!([]));
        // Neither the statement's reported aggregate basis nor current broker
        // basis may approve a specific incoming transfer automatically.
        assert!(r
            .service
            .approve_investments(
                &m.id,
                preview["digest"].as_str().unwrap(),
                BTreeMap::new(),
                true,
                &|| Ok(())
            )
            .await
            .is_err());
        assert_eq!(r.count("activities"), 0);
        assert!(r.current(&m).await.investment_ledger.is_none());
    } else {
        assert_eq!(preview["openingPositions"][0]["quantity"], "1");
    }
    assert_eq!(r.count("activities"), 0, "preview must not import a ledger");
    r.service
        .approve_investments_with_transfer_costs(
            &m.id,
            preview["digest"].as_str().unwrap(),
            if statement_transfers {
                BTreeMap::new()
            } else {
                BTreeMap::from([("security".into(), "10".into())])
            },
            if statement_transfers {
                BTreeMap::from([("investment:in".into(), "4".into())])
            } else {
                BTreeMap::new()
            },
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 5);
    assert!(r.events.0.lock().unwrap().iter().any(|event| matches!(
        event,
        DomainEvent::ActivitiesChanged { account_ids, .. } if account_ids.contains(&m.account_id)
    )));
    let ledger_ids = || {
        crate::schema::activities::table
            .select(crate::schema::activities::id)
            .order(crate::schema::activities::id)
            .load::<String>(&mut r.pool.get().unwrap())
            .unwrap()
    };
    let original_ids = ledger_ids();
    let accounts = Arc::new(crate::accounts::AccountRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let assets = Arc::new(crate::assets::AssetRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let projections = Arc::new(crate::portfolio::projection::ProjectionStore::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let fx_rates = Arc::new(crate::fx::FxRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let fx = Arc::new(FxService::new(fx_rates.clone()));
    fx.initialize().unwrap();
    let snapshot_service = Arc::new(SnapshotService::new(
        timezone.clone(),
        accounts.clone(),
        snapshots.clone(),
        r.service.activities.clone(),
    ));
    let observed = snapshots
        .get_snapshots_by_account(&m.account_id, None, None)
        .unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].source, SnapshotSource::BrokerImported);
    let asset_id = observed[0].positions.keys().next().unwrap().clone();
    let market = crate::market_data::MarketDataRepository::new(r.pool.clone(), r.writer.clone());
    // Historical prices are independent inputs; transaction prices cannot value
    // a day without trades. No live market or Plaid requests are made in this test.
    let quotes: Vec<_> = days
        .iter()
        .zip([15, 20, 25, 30, 28])
        .map(|(day, price)| {
            let price = Decimal::from(price);
            Quote {
                id: format!("{asset_id}_{day}_MANUAL"),
                asset_id: asset_id.clone(),
                timestamp: day.and_hms_opt(20, 0, 0).unwrap().and_utc(),
                open: price,
                high: price,
                low: price,
                close: price,
                adjclose: price,
                volume: Decimal::ZERO,
                currency: "USD".into(),
                data_source: "MANUAL".into(),
                created_at: Utc::now(),
                notes: None,
            }
        })
        .collect();
    market.upsert_quotes(&quotes).await.unwrap();
    let base_currency = Arc::new(RwLock::new("USD".into()));
    let coordinator = PortfolioCoordinator::new(CoordinatorDeps {
        base_currency: base_currency.clone(),
        timezone,
        sources: FactSources {
            accounts: accounts.clone(),
            activities: r.service.activities.clone(),
            assets: assets.clone(),
            quotes: r.quotes.clone(),
            fx_rates,
            snapshots: snapshots.clone(),
            projections: projections.clone(),
        },
        fx_service: fx.clone(),
        snapshot_service: snapshot_service.clone(),
        projections,
        lots: Arc::new(crate::lots::LotsRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        window_cadence: WindowCadence::Days(2),
    });
    let net_worth = NetWorthService::new(
        base_currency,
        accounts,
        assets,
        snapshot_service.clone(),
        r.quotes.clone(),
        valuations.clone(),
        fx,
    )
    .with_statement_history(statements.clone());
    // Expected values are hand-calculated, rather than derived from the imported rows:
    // opening 1 share + $100; buy 2 for $41; no trade; sell 1 for $29; $2 dividend.
    let expected = if statement_transfers {
        // $100 opening cash; five incoming shares at verified $4 book cost;
        // two outgoing shares; two $1 cash fees. Transfer market values are
        // deliberately much larger than either book cost or actual cash.
        [
            (100, 0, 100),
            (99, 100, 199),
            (99, 125, 224),
            (98, 90, 188),
            (98, 84, 182),
        ]
    } else {
        [
            (100, 15, 115),
            (59, 60, 119),
            (59, 75, 134),
            (88, 60, 148),
            (90, 56, 146),
        ]
    };
    let mut first_history = None;
    for repeat in [false, true] {
        if repeat {
            stage_broker();
            assert_eq!(
                r.service.sync(false, &|| Ok(())).await.unwrap()["failures"],
                json!([])
            );
            assert_eq!(ledger_ids(), original_ids);
        }
        let report = coordinator
            .run_job(
                PortfolioJobRequest {
                    account_ids: Some(vec![m.account_id.clone()]),
                    market_sync: MarketSyncMode::None,
                    force_full: false,
                },
                &SilentObserver,
            )
            .await
            .unwrap();
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        let daily = valuations
            .get_historical_valuations(&m.account_id, Some(days[0]), Some(days[4]))
            .unwrap();
        assert_eq!(daily.len(), days.len());
        for (index, (day, (cash, investments, total))) in days.iter().zip(expected).enumerate() {
            let value = daily.iter().find(|v| v.valuation_date == *day).unwrap();
            assert_eq!(value.value_status, ValuationStatus::Complete, "{day}");
            assert_eq!(value.cash_balance, Decimal::from(cash), "{day}");
            assert_eq!(
                value.investment_market_value,
                Decimal::from(investments),
                "{day}"
            );
            assert_eq!(value.total_value_base, Decimal::from(total), "{day}");
            if statement_transfers {
                assert_eq!(
                    value.cost_basis,
                    Decimal::from([0, 20, 20, 12, 12][index]),
                    "cost {day}"
                );
            }
            let point = net_worth.get_net_worth(*day).await.unwrap();
            assert_eq!(point.net_worth, Decimal::from(total), "{day}");
        }
        let history = net_worth
            .get_net_worth_history(
                if statement_transfers {
                    prefix_start.pred_opt().unwrap()
                } else {
                    days[0]
                },
                days[4],
            )
            .await
            .unwrap();
        if statement_transfers {
            assert_eq!(history.first().unwrap().date, prefix_start);
            for (date, total) in [
                (prefix_start, 80),
                (prefix_start.succ_opt().unwrap(), 80),
                (days[0].pred_opt().unwrap(), 90),
            ] {
                let point = history.iter().find(|point| point.date == date).unwrap();
                assert_eq!(point.net_worth, Decimal::from(total));
                assert_eq!(point.breakdown["cash"], Decimal::from(10));
                assert_eq!(point.breakdown["investments"], Decimal::from(total - 10));
                assert_eq!(point.statement_sources.len(), 1);
                assert_eq!(point.statement_sources[0].account_id, m.account_id);
                let historical = net_worth.get_net_worth(date).await.unwrap();
                assert_eq!(historical.net_worth, Decimal::from(total));
            }
            assert_eq!(
                serde_json::to_value(statements.list(&m.account_id).unwrap()).unwrap(),
                original_statements
            );
            assert_eq!(
                r.current(&m)
                    .await
                    .investment_ledger
                    .unwrap()
                    .transfer_unit_costs["investment:in"],
                "4"
            );
        } else {
            assert_eq!(history.len(), days.len());
        }
        assert_eq!(
            history.iter().filter(|point| point.date >= days[0]).count(),
            days.len()
        );
        for ((point, day), (cash, investments, total)) in history
            .iter()
            .filter(|point| point.date >= days[0])
            .zip(&days)
            .zip(expected)
        {
            assert!(
                point.statement_sources.is_empty(),
                "calculated history owns {day}"
            );
            assert_eq!(point.date, *day);
            assert_eq!(point.currency, "USD");
            assert_eq!(point.net_worth, Decimal::from(total), "{day}");
            assert_eq!(point.portfolio_value, Decimal::from(total), "{day}");
            assert_eq!(point.total_liabilities, Decimal::ZERO, "{day}");
            assert_eq!(
                point.breakdown.get("cash"),
                Some(&Decimal::from(cash)),
                "{day}"
            );
            assert_eq!(
                point
                    .breakdown
                    .get("investments")
                    .copied()
                    .unwrap_or_default(),
                Decimal::from(investments),
                "{day}"
            );
        }
        if let Some(first) = &first_history {
            assert_eq!(
                &history, first,
                "repeat sync must preserve historical net worth"
            );
        } else {
            first_history = Some(history);
        }
        // A day without an activity may have no calculated keyframe. Read the
        // authoritative transactions timeline, as the application does; a raw
        // imported broker snapshot can remain stored for switching back modes.
        let latest = snapshot_service
            .get_latest_snapshots_as_of(std::slice::from_ref(&m.account_id), today)
            .unwrap()
            .remove(&m.account_id)
            .unwrap();
        assert_eq!(latest.source, SnapshotSource::Calculated);
        assert_eq!(
            latest.positions[&asset_id].quantity,
            Decimal::from(if statement_transfers { 3 } else { 2 })
        );
        assert_eq!(
            latest.cash_total_account_currency,
            Decimal::from(if statement_transfers { 98 } else { 90 })
        );
        if statement_transfers {
            assert_eq!(
                latest.positions[&asset_id].total_cost_basis,
                Decimal::from(12)
            );
        }
    }
}

#[tokio::test]
async fn plaid_brokerage_missing_opening_basis_and_wrong_basis_cannot_switch_tracking() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    set_brokerage(
        &r,
        "3",
        "30",
        json!("31"),
        json!([brokerage_trade("buy", "buy", "2", "10", "21", "1")]),
    );
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(p["openingPositions"][0]["quantity"], "1");
    assert!(r
        .service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert!(r
        .service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::from([("security".into(), "999".into())]),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("import_runs"), 0);
    assert!(r.current(&m).await.investment_ledger.is_none());
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::from([("security".into(), "10".into())]),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 3);
}
#[tokio::test]
async fn plaid_brokerage_review_rejects_zero_quantity_with_nonzero_value() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    set_brokerage(&r, "0", "100", json!("0"), json!([]));
    assert!(matches!(
        r.service.preview_investments(&m.id, &|| Ok(())).await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("import_runs"), 0);
    let unchanged = r.current(&m).await;
    assert!(unchanged.investment_ledger.is_none());
    assert!(unchanged.holdings_staged.is_none());
    assert!(r
        .service
        .approve_investments(&m.id, "review", BTreeMap::new(), true, &|| Ok(()))
        .await
        .is_err());
    #[derive(QueryableByName)]
    struct Mode {
        #[diesel(sql_type=Text)]
        tracking_mode: String,
    }
    assert_eq!(
        diesel::sql_query("SELECT tracking_mode FROM accounts WHERE id=?")
            .bind::<Text, _>(&m.account_id)
            .get_result::<Mode>(&mut r.pool.get().unwrap())
            .unwrap()
            .tracking_mode,
        "HOLDINGS"
    );
}

#[tokio::test]
async fn plaid_brokerage_cash_sweep_dividend_imports_as_cash_income_once() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut dividend = brokerage_trade("cash-dividend", "cash", "0", "0", "-2", "0");
    dividend["subtype"] = json!("dividend");
    let mut holdings = brokerage_fixture("cash", None, json!("250"));
    holdings["holdings"][0]["quantity"] = json!("250");
    holdings["holdings"][0]["institution_value"] = json!("250");
    holdings["holdings"][0]["institution_price"] = json!("1");
    let mut page = brokerage_ledger_page(json!([dividend]));
    page["securities"] = holdings["securities"].clone();
    r.api.0.lock().unwrap().holdings = Some(holdings);
    r.api.0.lock().unwrap().investments.push_back(page.clone());
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(p["issues"], json!([]));
    assert_eq!(p["openingCash"], "248");
    assert_eq!(p["openingPositions"], json!([]));
    assert_eq!(p["records"][0]["type"], "DIVIDEND");
    assert_eq!(p["records"][0]["securityId"], Value::Null);
    assert_eq!(r.count("activities"), 0);
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
    #[derive(QueryableByName)]
    struct Income {
        #[diesel(sql_type=Text)]
        amount: String,
        #[diesel(sql_type=diesel::sql_types::Nullable<Text>)]
        asset_id: Option<String>,
    }
    let income =
        diesel::sql_query("SELECT amount,asset_id FROM activities WHERE activity_type='DIVIDEND'")
            .get_result::<Income>(&mut r.pool.get().unwrap())
            .unwrap();
    assert_eq!(
        income.amount.parse::<rust_decimal::Decimal>().unwrap(),
        rust_decimal::Decimal::from(2)
    );
    assert!(income.asset_id.is_none());
    assert!(r.current(&m).await.investment_ledger.is_some());
    r.api.0.lock().unwrap().investments.push_back(page);
    assert_eq!(
        r.service.sync(false, &|| Ok(())).await.unwrap()["failures"],
        json!([])
    );
    assert_eq!(r.count("activities"), 2);
}

#[tokio::test]
async fn plaid_brokerage_unsupported_instrument_is_visible_and_never_partially_imports() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    r.api.0.lock().unwrap().holdings =
        Some(brokerage_fixture("derivative", Some("TEST"), json!("80")));
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert!(!p["issues"].as_array().unwrap().is_empty());
    assert!(r
        .service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(r.count("activities"), 0);
    assert!(r.current(&m).await.investment_ledger.is_none());
}
#[tokio::test]
async fn plaid_brokerage_provider_correction_preserves_owner_edits_and_rolls_back_inconsistent_ledger(
) {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let records = json!([brokerage_trade("buy", "buy", "2", "10", "20", "0")]);
    set_brokerage(&r, "2", "20", json!("20"), records);
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let id = wealthfolio_connect::plaid::ingest::stable_id(&m.id, "investment:buy");
    let target = id.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE activities SET quantity='3',is_user_modified=1 WHERE id=?")
                .bind::<Text, _>(target)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    set_brokerage(
        &r,
        "2",
        "20",
        json!("20"),
        json!([brokerage_trade("buy", "buy", "2", "10", "20", "0")]),
    );
    let out = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(out["failures"].as_array().unwrap().len(), 1);
    assert_eq!(r.current(&m).await.status, "review");
    assert_eq!(r.count("activities"), 2);
    assert_eq!(r.count("import_runs"), 1);
}

#[tokio::test]
async fn plaid_brokerage_corrections_and_explicit_cancellations_update_existing_source_once() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    set_brokerage(
        &r,
        "2",
        "20",
        json!("20"),
        json!([brokerage_trade("buy", "buy", "2", "10", "20", "0")]),
    );
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let baseline = r.current(&m).await.investment_ledger;
    // A correction replaces the row with the same source ID, without moving the baseline.
    let corrected = brokerage_trade("buy", "buy", "3", "10", "30", "0");
    set_brokerage(&r, "3", "30", json!("30"), json!([corrected.clone()]));
    assert_eq!(
        r.service.sync(false, &|| Ok(())).await.unwrap()["failures"],
        json!([])
    );
    assert_eq!(r.count("activities"), 2);
    assert!(r.current(&m).await.investment_ledger == baseline);
    let mut cancel = brokerage_trade("cancel", "sell", "3", "10", "-30", "0");
    cancel["cancel_transaction_id"] = json!("buy");
    set_brokerage(&r, "0", "0", json!("0"), json!([corrected, cancel]));
    assert_eq!(
        r.service.sync(false, &|| Ok(())).await.unwrap()["failures"],
        json!([])
    );
    assert_eq!(r.count("activities"), 1);
    assert!(r.current(&m).await.investment_ledger == baseline);
}

#[tokio::test]
async fn plaid_brokerage_kernel_reconciles_sales_dividends_interest_fees_and_tax() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let buy = brokerage_trade("buy", "buy", "2", "10", "20", "0");
    set_brokerage(&r, "2", "20", json!("20"), json!([buy.clone()]));
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let mut records = vec![buy, brokerage_trade("sell", "sell", "-1", "15", "-15", "0")];
    for (id, kind, subtype, amount) in [
        ("div", "cash", "dividend", "-2"),
        ("int", "cash", "interest", "-1"),
        ("fee", "fee", "account fee", "1"),
        ("tax", "cash", "tax", "1"),
    ] {
        let mut row = brokerage_trade(id, kind, "0", "0", amount, "0");
        row["subtype"] = json!(subtype);
        if id != "div" {
            row["security_id"] = Value::Null;
        }
        records.push(row);
    }
    set_brokerage(&r, "1", "15", json!("10"), json!(records));
    r.api.0.lock().unwrap().holdings.as_mut().unwrap()["accounts"][0]["balances"]["current"] =
        json!("261");
    let result = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(result["failures"], json!([]));
    assert_eq!(r.count("activities"), 7);
}

#[tokio::test]
async fn plaid_brokerage_review_rejects_missing_stale_changed_and_invalid_cost_inputs() {
    let r = Rig::new().await;
    let (c, m) = r.seed(PlaidProduct::Investments).await;
    assert!(r
        .service
        .approve_investments(&m.id, "missing", BTreeMap::new(), true, &|| Ok(()))
        .await
        .is_err());
    set_brokerage(
        &r,
        "3",
        "30",
        json!("30"),
        json!([brokerage_trade("buy", "buy", "2", "10", "20", "0")]),
    );
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    for cost in ["-1", "invalid"] {
        assert!(r
            .service
            .approve_investments(
                &m.id,
                p["digest"].as_str().unwrap(),
                BTreeMap::from([("security".into(), cost.into())]),
                true,
                &|| Ok(())
            )
            .await
            .is_err());
    }
    assert!(r
        .service
        .approve_investments(&m.id, "changed", BTreeMap::new(), true, &|| Ok(()))
        .await
        .is_err());
    let mut staged = r.current(&m).await;
    staged.holdings_staged.as_mut().unwrap().fetched_at = Utc::now() - Duration::minutes(31);
    r.repo.put_mappings(vec![staged]).await.unwrap();
    assert!(r
        .service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    r.service.disconnect(&c.id, &|| Ok(())).await.unwrap();
    assert!(r
        .service
        .approve_investments(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(r.count("activities"), 0);
}

#[tokio::test]
async fn plaid_brokerage_invalid_quotes_and_ambiguous_security_identity_block_conversion() {
    for case in 0..3 {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        set_brokerage(
            &r,
            "2",
            if case == 0 { "-20" } else { "20" },
            Value::Null,
            json!([brokerage_trade("buy", "buy", "2", "10", "20", "0")]),
        );
        if case == 1 {
            let mut api = r.api.0.lock().unwrap();
            let holdings = api.holdings.as_mut().unwrap();
            let mut security = holdings["securities"][0].clone();
            security["security_id"] = json!("second");
            holdings["securities"]
                .as_array_mut()
                .unwrap()
                .push(security);
            let mut holding = holdings["holdings"][0].clone();
            holding["security_id"] = json!("second");
            holdings["holdings"].as_array_mut().unwrap().push(holding);
        }
        let p = r
            .service
            .preview_investments(&m.id, &|| Ok(()))
            .await
            .unwrap();
        let costs = BTreeMap::from([("second".into(), "10".into())]);
        let result = r
            .service
            .approve_investments(
                &m.id,
                p["digest"].as_str().unwrap(),
                costs,
                true,
                &|| Ok(()),
            )
            .await;
        if case < 2 {
            assert!(result.is_err());
            assert_eq!(r.count("activities"), 0);
        } else {
            result.unwrap();
            assert_eq!(r.count("activities"), 2);
        }
    }
}

#[tokio::test]
async fn plaid_brokerage_empty_ledger_requires_zero_reported_basis() {
    for basis in ["0", "20"] {
        let r = Rig::new().await;
        let (_, m) = r.seed(PlaidProduct::Investments).await;
        set_brokerage(&r, "0", "0", json!(basis), json!([]));
        r.api.0.lock().unwrap().holdings.as_mut().unwrap()["accounts"][0]["balances"]["current"] =
            json!("0");
        let p = r
            .service
            .preview_investments(&m.id, &|| Ok(()))
            .await
            .unwrap();
        let result = r
            .service
            .approve_investments(
                &m.id,
                p["digest"].as_str().unwrap(),
                BTreeMap::new(),
                true,
                &|| Ok(()),
            )
            .await;
        assert_eq!(result.is_ok(), basis == "0");
        assert_eq!(r.count("activities"), 0);
    }
}

async fn unmapped_plaid_connection(r: &Rig, product: PlaidProduct) -> super::PlaidConnection {
    let c = r.connection(product);
    r.repo.put_connection(c.clone(), None).await.unwrap();
    r.vault
        .set_secret(&secret_key(&c, "access").unwrap(), "access")
        .unwrap();
    c
}

#[tokio::test]
async fn plaid_create_and_map_derives_types_defaults_and_discovery_without_importing() {
    use wealthfolio_core::accounts::{AccountRepositoryTrait, TrackingMode};
    for (product, kind, subtype, expected) in [
        (PlaidProduct::Transactions, "depository", "checking", "CASH"),
        (PlaidProduct::Transactions, "depository", "savings", "CASH"),
        (
            PlaidProduct::Transactions,
            "credit",
            "credit card",
            "CREDIT_CARD",
        ),
        (
            PlaidProduct::Investments,
            "investment",
            "brokerage",
            "SECURITIES",
        ),
        (
            PlaidProduct::Investments,
            "depository",
            "cash management",
            "SECURITIES",
        ),
        (
            PlaidProduct::Investments,
            "credit",
            "credit card",
            "CREDIT_CARD",
        ),
    ] {
        let r = Rig::new().await;
        let c = unmapped_plaid_connection(&r, product).await;
        let mut a = account(kind);
        a["subtype"] = json!(subtype);
        r.api.0.lock().unwrap().accounts["accounts"] = json!([a]);
        let before = r.count("sync_outbox");
        let result = r
            .service
            .create_and_map(&c.id, "external", "  Synthetic account  ", true, &|| Ok(()))
            .await
            .unwrap();
        assert!(result.created);
        let local = crate::accounts::AccountRepository::new(r.pool.clone(), r.writer.clone())
            .get_by_id(&result.account_id)
            .unwrap();
        assert_eq!(local.name, "Synthetic account");
        assert_eq!(local.account_type, expected);
        assert_eq!(local.currency, "USD");
        assert_eq!(
            local.tracking_mode,
            if expected == "SECURITIES" {
                TrackingMode::Holdings
            } else {
                TrackingMode::Transactions
            }
        );
        assert_eq!(local.provider.as_deref(), Some(DIRECT_PLAID_PROVIDER));
        assert_eq!(
            local.provider_account_id.as_deref(),
            Some(result.mapping_id.as_str())
        );
        assert!(local.is_active && !local.is_default && !local.is_archived);
        assert!(local.account_number.is_none());
        local.accounting_settings().unwrap();
        assert_eq!(r.count("sync_outbox"), before);
        assert_eq!(r.count("activities"), 0);
        assert_eq!(r.count("import_runs"), 0);
        let maps = r.repo.mappings().await.unwrap();
        assert_eq!(maps[0].status, "initial_review");
        assert!(maps[0].cursor.is_none());
        assert!(maps[0].cutover_date.is_none());
        assert_eq!(r.api.calls("exchange"), 0);
        let discovered = r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
        assert_eq!(discovered["accounts"][0]["mappingId"], result.mapping_id);
        r.service.sync(true, &|| Ok(())).await.unwrap();
        assert_eq!(r.api.calls("transactions"), 0);
        assert_eq!(r.api.calls("holdings"), 0);
        assert_eq!(
            r.events
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|e| matches!(e, DomainEvent::AccountsChanged { .. }))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn plaid_create_and_map_concurrent_retries_preserve_existing_binding_and_edits() {
    use wealthfolio_core::accounts::AccountRepositoryTrait;
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    let (one, two) = tokio::join!(
        r.service
            .create_and_map(&c.id, "external", "First", false, &|| Ok(())),
        r.service
            .create_and_map(&c.id, "external", "Second", false, &|| Ok(())),
    );
    let one = one.unwrap();
    let two = two.unwrap();
    assert_ne!(one.created, two.created);
    assert_eq!(one.account_id, two.account_id);
    assert_eq!(one.mapping_id, two.mapping_id);
    assert_eq!(r.count("accounts"), 1);
    assert_eq!(r.count("plaid_accounts"), 1);
    let id = one.account_id.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE accounts SET name='Owner edit' WHERE id=?")
                .bind::<Text, _>(id)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let mut m = r.repo.mappings().await.unwrap().remove(0);
    m.cursor = Some("reviewed-cursor".into());
    m.status = "idle".into();
    r.repo.put_mappings(vec![m.clone()]).await.unwrap();
    let retry = r
        .service
        .create_and_map(&c.id, "external", "Replacement", false, &|| Ok(()))
        .await
        .unwrap();
    assert!(!retry.created);
    assert_eq!(retry.account_id, one.account_id);
    assert_eq!(r.current(&m).await.cursor, m.cursor);
    assert_eq!(r.current(&m).await.status, "idle");
    assert_eq!(
        crate::accounts::AccountRepository::new(r.pool.clone(), r.writer.clone())
            .get_by_id(&one.account_id)
            .unwrap()
            .name,
        "Owner edit"
    );
    assert_eq!(
        r.events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, DomainEvent::AccountsChanged { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn plaid_create_and_map_rolls_back_account_mapping_and_outbox_on_failure() {
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    r.writer.exec(|conn| { diesel::sql_query("CREATE TRIGGER reject_mapping BEFORE INSERT ON plaid_accounts BEGIN SELECT RAISE(ABORT,'fixture'); END").execute(conn).map_err(sql)?; Ok(()) }).await.unwrap();
    let before = r.count("sync_outbox");
    assert!(r
        .service
        .create_and_map(&c.id, "external", "Synthetic", false, &|| Ok(()))
        .await
        .is_err());
    assert_eq!(r.count("accounts"), 0);
    assert_eq!(r.count("plaid_accounts"), 0);
    assert_eq!(r.count("sync_outbox"), before);
    assert!(r.events.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn plaid_create_and_map_rejects_invalid_inputs_and_lost_admission_without_accounts() {
    for case in [
        "empty",
        "short",
        "long",
        "unsupported",
        "currency",
        "foreign",
        "disconnected",
        "attestation",
        "admission",
        "expired",
    ] {
        let r = Rig::new().await;
        let mut c = unmapped_plaid_connection(
            &r,
            if case == "attestation" {
                PlaidProduct::Investments
            } else {
                PlaidProduct::Transactions
            },
        )
        .await;
        let mut name = "Synthetic".to_string();
        match case {
            "empty" => name = "  ".into(),
            "short" => name = "x".into(),
            "long" => name = "x".repeat(51),
            "unsupported" => {
                r.api.0.lock().unwrap().accounts["accounts"][0]["subtype"] = json!("other")
            }
            "currency" => {
                r.api.0.lock().unwrap().accounts["accounts"][0]["balances"]["iso_currency_code"] =
                    Value::Null
            }
            "foreign" => r.api.0.lock().unwrap().accounts["item"]["item_id"] = json!("foreign"),
            "disconnected" => {
                c.status = "disconnected".into();
                r.repo
                    .put_connection(c.clone(), Some("connected".into()))
                    .await
                    .unwrap();
            }
            "attestation" => {
                r.api.0.lock().unwrap().accounts["accounts"] = json!([account("investment")])
            }
            _ => (),
        }
        let active = Arc::new(AtomicBool::new(case != "admission"));
        let admissions = std::sync::atomic::AtomicUsize::new(0);
        let admit = || {
            if active.load(Ordering::SeqCst)
                && !(case == "expired" && admissions.fetch_add(1, Ordering::SeqCst) >= 2)
            {
                Ok(())
            } else {
                Err(SyncError::Access)
            }
        };
        assert!(
            r.service
                .create_and_map(&c.id, "external", &name, false, &admit)
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(r.count("accounts"), 0, "{case}");
        assert_eq!(r.count("plaid_accounts"), 0, "{case}");
    }
}

#[tokio::test]
async fn plaid_new_account_keeps_mapping_when_not_ready_then_imports_reviewed_history() {
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    let result = r
        .service
        .create_and_map(&c.id, "external", "Synthetic", false, &|| Ok(()))
        .await
        .unwrap();
    let m = r.repo.mappings().await.unwrap().remove(0);
    r.api.fail("transactions", PlaidError::NotReady);
    assert!(matches!(
        r.service
            .preview(&result.mapping_id, false, &|| Ok(()))
            .await,
        Err(SyncError::NotReady)
    ));
    assert_eq!(r.count("accounts"), 1);
    assert_eq!(r.count("plaid_accounts"), 1);
    assert_eq!(r.count("activities"), 0);
    let p = r
        .preview(&m, json!([transaction("purchase", "10", "FOOD_AND_DRINK")]))
        .await;
    r.service
        .approve(
            &m.id,
            p["digest"].as_str().unwrap(),
            p["suggestedOpening"].as_str().map(str::to_owned),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 2);
    assert!(r.current(&m).await.cursor.is_some());
}

#[tokio::test]
async fn plaid_create_and_map_prepares_foreign_currency_without_fake_rates() {
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    r.api.0.lock().unwrap().accounts["accounts"][0]["balances"]["iso_currency_code"] = json!("EUR");
    let result = r
        .service
        .create_and_map(&c.id, "external", "Euro account", false, &|| Ok(()))
        .await
        .unwrap();
    assert!(result.created);
    use crate::schema::assets::dsl as a;
    assert_eq!(
        a::assets
            .filter(a::kind.eq("FX"))
            .count()
            .get_result::<i64>(&mut r.pool.get().unwrap())
            .unwrap(),
        1
    );
    assert_eq!(r.count("quotes"), 0);
    assert!(r.events.0.lock().unwrap().iter().any(|e| matches!(e, DomainEvent::AccountsChanged { currency_changes, .. } if currency_changes[0].new_currency == "EUR")));
}

#[tokio::test]
async fn plaid_auto_account_writer_rechecks_identity_and_account_invariants() {
    use wealthfolio_core::accounts::{NewAccount, TrackingMode};
    for case in [
        "currency",
        "kind",
        "mode",
        "provider",
        "binding",
        "archived",
        "inactive",
        "cutover",
        "status",
        "cursor",
        "ledger",
        "connection",
        "product",
        "existing-currency",
        "existing-card",
        "existing-binding",
    ] {
        let r = Rig::new().await;
        let (mut c, mut m) = r.seed(PlaidProduct::Transactions).await;
        let original = m.clone();
        m.id = uuid::Uuid::new_v4().to_string();
        m.account_id = uuid::Uuid::new_v4().to_string();
        if !case.starts_with("existing") {
            m.external_id = "another".into();
        }
        let mut account = NewAccount {
            id: None,
            name: "Synthetic".into(),
            account_type: "CASH".into(),
            group: None,
            currency: "USD".into(),
            is_default: false,
            is_active: true,
            platform_id: None,
            account_number: None,
            meta: None,
            provider: Some(DIRECT_PLAID_PROVIDER.into()),
            provider_account_id: Some(m.id.clone()),
            is_archived: false,
            tracking_mode: TrackingMode::Transactions,
        };
        match case {
            "currency" => account.currency = "EUR".into(),
            "kind" => account.account_type = "SECURITIES".into(),
            "mode" => account.tracking_mode = TrackingMode::Holdings,
            "provider" => account.provider = None,
            "binding" => account.provider_account_id = Some("foreign".into()),
            "archived" => account.is_archived = true,
            "inactive" => account.is_active = false,
            "cutover" => m.cutover_date = Some(Utc::now().date_naive()),
            "status" => m.status = "idle".into(),
            "cursor" => m.cursor = Some("checkpoint".into()),
            "ledger" => {
                m.investment_ledger =
                    Some(wealthfolio_connect::plaid::investments::InvestmentLedger {
                        transfer_unit_costs: std::collections::BTreeMap::new(),
                        start_date: Utc::now().date_naive(),
                        opening_cash: "0".into(),
                        opening_positions: BTreeMap::new(),
                    })
            }
            "connection" => {
                c.status = "disconnected".into();
                r.repo
                    .put_connection(c, Some("connected".into()))
                    .await
                    .unwrap();
            }
            "existing-currency" => m.currency = "EUR".into(),
            "existing-card" => m.credit_card = true,
            "existing-binding" => {
                let id = original.account_id.clone();
                r.writer
                    .exec(move |conn| {
                        diesel::sql_query(
                            "UPDATE accounts SET provider_account_id='foreign' WHERE id=?",
                        )
                        .bind::<Text, _>(id)
                        .execute(conn)
                        .map_err(sql)?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            _ => (),
        }
        assert!(
            r.repo
                .create_mapped_account(
                    m,
                    if case == "product" {
                        PlaidProduct::Investments
                    } else {
                        PlaidProduct::Transactions
                    },
                    account
                )
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(r.count("accounts"), 1, "{case}");
        assert_eq!(r.count("plaid_accounts"), 1, "{case}");
    }
}

#[tokio::test]
async fn plaid_auto_account_avoids_outbox_while_manual_account_creation_still_projects() {
    use wealthfolio_core::accounts::{AccountRepositoryTrait, NewAccount, TrackingMode};
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    let before = r.count("sync_outbox");
    r.service
        .create_and_map(&c.id, "external", "Automatic", false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("sync_outbox"), before);
    let repo = crate::accounts::AccountRepository::new(r.pool.clone(), r.writer.clone());
    repo.create(NewAccount {
        id: None,
        name: "Manual".into(),
        account_type: "CASH".into(),
        group: None,
        currency: "USD".into(),
        is_default: false,
        is_active: true,
        platform_id: None,
        account_number: None,
        meta: None,
        provider: None,
        provider_account_id: None,
        is_archived: false,
        tracking_mode: TrackingMode::Transactions,
    })
    .await
    .unwrap();
    assert_eq!(r.count("sync_outbox"), before + 1);
}

#[tokio::test]
async fn plaid_auto_account_fx_preparation_failure_does_not_create_account() {
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    r.api.0.lock().unwrap().accounts["accounts"][0]["balances"]["iso_currency_code"] = json!("EUR");
    r.writer.exec(|conn| { diesel::sql_query("CREATE TRIGGER reject_fx BEFORE INSERT ON assets BEGIN SELECT RAISE(ABORT,'fixture'); END").execute(conn).map_err(sql)?; Ok(()) }).await.unwrap();
    assert!(matches!(
        r.service
            .create_and_map(&c.id, "external", "Synthetic", false, &|| Ok(()))
            .await,
        Err(SyncError::Storage)
    ));
    assert_eq!(r.count("accounts"), 0);
    assert_eq!(r.count("plaid_accounts"), 0);
}

#[tokio::test]
async fn plaid_auto_brokerage_retry_preserves_transaction_history_activation() {
    use wealthfolio_core::accounts::{AccountRepositoryTrait, TrackingMode};
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Investments).await;
    r.api.0.lock().unwrap().accounts["accounts"] = json!([account("investment")]);
    let first = r
        .service
        .create_and_map(&c.id, "external", "Brokerage", true, &|| Ok(()))
        .await
        .unwrap();
    set_brokerage(&r, "0", "0", json!("0"), json!([]));
    let p = r
        .service
        .preview_investments(&first.mapping_id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_investments(
            &first.mapping_id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let retry = r
        .service
        .create_and_map(&c.id, "external", "Replacement", true, &|| Ok(()))
        .await
        .unwrap();
    assert!(!retry.created);
    assert_eq!(retry.account_id, first.account_id);
    let account = crate::accounts::AccountRepository::new(r.pool.clone(), r.writer.clone())
        .get_by_id(&first.account_id)
        .unwrap();
    assert_eq!(account.tracking_mode, TrackingMode::Transactions);
    assert_eq!(account.name, "Brokerage");
    assert_eq!(r.count("activities"), 1);
    assert!(r.repo.mappings().await.unwrap()[0]
        .investment_ledger
        .is_some());
}

#[tokio::test]
async fn plaid_brokerage_security_transfer_cost_review_is_atomic_persisted_and_idempotent() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut incoming = brokerage_trade("in", "transfer", "5", "100", "-500", "0");
    incoming["subtype"] = json!("transfer");
    let mut outgoing = brokerage_trade("out", "transfer", "-2", "100", "200", "0");
    outgoing["subtype"] = json!("transfer");
    let records = json!([incoming, outgoing]);
    set_brokerage(&r, "3", "30", json!("12"), records.clone());
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(p["issues"], json!([]));
    assert_eq!(p["openingPositions"], json!([]));
    assert!(p["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["requiresTransferCost"] == true));
    let digest = p["digest"].as_str().unwrap();
    assert!(r
        .service
        .approve_investments(&m.id, digest, BTreeMap::new(), true, &|| Ok(()))
        .await
        .is_err());
    assert_eq!(r.count("activities"), 0);
    assert!(r.current(&m).await.investment_ledger.is_none());
    for costs in [
        BTreeMap::from([("investment:in".into(), "-1".into())]),
        BTreeMap::from([("investment:unknown".into(), "4".into())]),
    ] {
        assert!(r
            .service
            .approve_investments_with_transfer_costs(
                &m.id,
                digest,
                BTreeMap::new(),
                costs,
                true,
                &|| Ok(())
            )
            .await
            .is_err());
        assert_eq!(r.count("activities"), 0);
    }
    r.service
        .approve_investments_with_transfer_costs(
            &m.id,
            digest,
            BTreeMap::new(),
            BTreeMap::from([("investment:in".into(), "4".into())]),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let count = r.count("activities");
    assert_eq!(count, 3);
    assert_eq!(
        r.current(&m)
            .await
            .investment_ledger
            .unwrap()
            .transfer_unit_costs["investment:in"],
        "4"
    );
    set_brokerage(&r, "3", "30", json!("12"), records.clone());
    let sync = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(sync["failures"], json!([]));
    assert_eq!(r.count("activities"), count);
    set_brokerage(&r, "3", "30", json!("12"), records.clone());
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert!(r
        .service
        .approve_investments_with_transfer_costs(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            BTreeMap::from([("investment:in".into(), "5".into())]),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(
        r.current(&m)
            .await
            .investment_ledger
            .unwrap()
            .transfer_unit_costs["investment:in"],
        "4"
    );
    assert_eq!(r.count("activities"), count);
    let mut corrected = records.clone();
    corrected[0]["quantity"] = json!("6");
    set_brokerage(&r, "4", "40", json!("16"), corrected);
    let changed = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert!(changed["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == "reviewedTransferChanged"));
    assert!(r
        .service
        .approve_investments_with_transfer_costs(
            &m.id,
            changed["digest"].as_str().unwrap(),
            BTreeMap::new(),
            BTreeMap::new(),
            true,
            &|| Ok(())
        )
        .await
        .is_err());
    assert_eq!(r.count("activities"), count);
}

#[tokio::test]
async fn plaid_brokerage_security_transfer_fee_is_cash_not_market_value_or_book_cost() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut incoming = brokerage_trade("in-fee", "transfer", "5", "100", "-500", "1");
    incoming["subtype"] = json!("transfer");
    set_brokerage(&r, "5", "50", json!("20"), json!([incoming]));
    let p = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(p["issues"], json!([]));
    assert_eq!(p["openingCash"], "201");
    r.service
        .approve_investments_with_transfer_costs(
            &m.id,
            p["digest"].as_str().unwrap(),
            BTreeMap::new(),
            BTreeMap::from([("investment:in-fee".into(), "4".into())]),
            true,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("activities"), 3);
}

#[tokio::test]
async fn statement_history_import_drives_net_worth_across_months_and_calculated_cutover() {
    use rust_decimal::Decimal;
    use wealthfolio_core::{
        fx::{FxService, FxServiceTrait},
        portfolio::{
            economic_events::BasisStatus,
            net_worth::{NetWorthService, NetWorthServiceTrait},
            snapshot::SnapshotService,
            statements::{
                StatementRepository, StatementService, StatementSnapshot, StatementWrite,
            },
            valuation::{
                DailyAccountValuation, ExternalFlowSource, ValuationRepositoryTrait,
                ValuationStatus,
            },
        },
    };
    let r = Rig::new().await;
    let (_, first) = r.seed(PlaidProduct::Investments).await;
    let second_id = uuid::Uuid::new_v4().to_string();
    let local = second_id.clone();
    r.writer.exec(move |conn| {
        diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode) VALUES (?,'Synthetic second','SECURITIES','USD',0,1,0,'HOLDINGS')")
            .bind::<Text,_>(local).execute(conn).map_err(sql)?;
        Ok(())
    }).await.unwrap();
    let accounts = Arc::new(crate::accounts::AccountRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let statements = Arc::new(crate::portfolio::statements::StatementStore::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let service = StatementService::new(
        statements.clone(),
        accounts.clone(),
        valuations.clone(),
        r.service.timezone.clone(),
    );
    let write = |day: &str, total: &str| StatementWrite {
        snapshot: StatementSnapshot {
            date: day.parse().unwrap(),
            currency: "USD".into(),
            total_value: total.into(),
            cash_value: Some("10".into()),
            positions: vec![],
            positions_complete: false,
            format: "merrill-cma-monthly-v1".into(),
            source_page: 1,
        },
        expected_revision: None,
    };
    service
        .save(
            first.account_id.clone(),
            vec![
                write("2025-01-31", "100"),
                write("2025-02-28", "110"),
                write("2025-03-15", "999"),
            ],
            true,
        )
        .await
        .unwrap();
    service
        .save(second_id.clone(), vec![write("2025-02-10", "50")], true)
        .await
        .unwrap();
    let row = |day: &str, total: i64| DailyAccountValuation {
        id: format!("{}_{}", first.account_id, day),
        account_id: first.account_id.clone(),
        valuation_date: day.parse().unwrap(),
        account_currency: "USD".into(),
        base_currency: "USD".into(),
        fx_rate_to_base: Decimal::ONE,
        cash_balance: Decimal::ZERO,
        investment_market_value: Decimal::from(total),
        total_value: Decimal::from(total),
        cost_basis: Decimal::ZERO,
        book_basis: Decimal::ZERO,
        net_contribution: Decimal::ZERO,
        cash_balance_base: Decimal::ZERO,
        investment_market_value_base: Decimal::from(total),
        total_value_base: Decimal::from(total),
        cost_basis_base: Decimal::ZERO,
        book_basis_base: Decimal::ZERO,
        net_contribution_base: Decimal::ZERO,
        external_inflow_base: Decimal::ZERO,
        external_outflow_base: Decimal::ZERO,
        external_flow_source: ExternalFlowSource::NoFlow,
        performance_eligible_value_base: Decimal::from(total),
        value_status: ValuationStatus::Complete,
        basis_status: BasisStatus::Unknown,
        calculated_at: Utc::now(),
    };
    let mut partial = row("2025-03-01", 9999);
    partial.value_status = ValuationStatus::PartialUnpriced;
    valuations
        .replace_valuations_for_account(
            &first.account_id,
            None,
            &[partial, row("2025-03-10", 200), row("2025-03-12", 210)],
        )
        .await
        .unwrap();
    let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let snapshot_service = Arc::new(SnapshotService::new(
        r.service.timezone.clone(),
        accounts.clone(),
        snapshots,
        r.service.activities.clone(),
    ));
    let fx = Arc::new(FxService::new(Arc::new(crate::fx::FxRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ))));
    fx.initialize().unwrap();
    let net_worth = NetWorthService::new(
        Arc::new(RwLock::new("USD".into())),
        accounts,
        Arc::new(crate::assets::AssetRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        snapshot_service,
        r.quotes.clone(),
        valuations.clone(),
        fx,
    )
    .with_statement_history(statements.clone());
    let history = net_worth
        .get_net_worth_history("2025-01-01".parse().unwrap(), "2025-03-16".parse().unwrap())
        .await
        .unwrap();
    for (day, total) in [
        ("2025-01-31", 100),
        ("2025-02-09", 100),
        ("2025-02-10", 150),
        ("2025-02-15", 150),
        ("2025-02-28", 160),
        ("2025-03-01", 160),
        ("2025-03-09", 160),
        ("2025-03-10", 250),
        ("2025-03-11", 250),
        ("2025-03-12", 260),
        ("2025-03-16", 260),
    ] {
        let point = history.iter().find(|p| p.date.to_string() == day).unwrap();
        assert_eq!(point.net_worth, Decimal::from(total), "{day}");
    }
    for (day, cash, investments) in [
        ("2025-01-31", 10, 90),
        ("2025-02-15", 20, 130),
        ("2025-03-11", 10, 240),
    ] {
        let point = history.iter().find(|p| p.date.to_string() == day).unwrap();
        assert_eq!(point.breakdown["cash"], Decimal::from(cash));
        assert_eq!(point.breakdown["investments"], Decimal::from(investments));
    }
    assert_eq!(history.first().unwrap().date.to_string(), "2025-01-31");
    assert_eq!(history[0].incomplete_accounts, vec![second_id.clone()]);
    let after = history
        .iter()
        .find(|p| p.date.to_string() == "2025-03-11")
        .unwrap();
    assert_eq!(after.statement_sources.len(), 1);
    assert_eq!(after.statement_sources[0].account_id, second_id);
    let subset = net_worth
        .get_net_worth_history("2025-03-11".parse().unwrap(), "2025-03-11".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(subset[0].net_worth, Decimal::from(250));
    assert_eq!(r.count("activities"), 0);
    assert_eq!(r.count("holdings_snapshots"), 0);
    assert_eq!(
        valuations
            .get_historical_valuations(&first.account_id, None, None)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(statements.list(&first.account_id).unwrap().len(), 3);
    assert!(
        r.api.0.lock().unwrap().calls.is_empty(),
        "statement history must not call Plaid"
    );
}

#[tokio::test]
async fn plaid_provider_cash_equivalent_holdings_snapshot_counts_cash_once_without_opaque_asset() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    let mut fixture = brokerage_fixture("other", None, Value::Null);
    fixture["securities"][0]["is_cash_equivalent"] = json!(true);
    r.api.0.lock().unwrap().holdings = Some(fixture);
    let preview = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(
            &m.id,
            preview["digest"].as_str().unwrap(),
            false,
            &|| Ok(()),
        )
        .await
        .unwrap();
    #[derive(QueryableByName)]
    struct CashSnapshot {
        #[diesel(sql_type = Text)]
        cash: String,
        #[diesel(sql_type = Text)]
        positions: String,
    }
    let row = diesel::sql_query(
        "SELECT cash_total_account_currency AS cash, positions FROM holdings_snapshots",
    )
    .get_result::<CashSnapshot>(&mut r.pool.get().unwrap())
    .unwrap();
    assert_eq!(
        row.cash.parse::<rust_decimal::Decimal>().unwrap(),
        rust_decimal::Decimal::from(250)
    );
    let positions: HashMap<String, Value> = serde_json::from_str(&row.positions).unwrap();
    assert!(positions.is_empty());
    assert_eq!(r.count("activities"), 0);
    assert!(r.current(&m).await.opaque_security_ids.is_empty());
}

#[tokio::test]
async fn plaid_preview_issue_security_context_keeps_bad_records_blocking_final_import() {
    let r = Rig::new().await;
    let (_, m) = r.seed(PlaidProduct::Investments).await;
    set_brokerage(
        &r,
        "2",
        "20",
        Value::Null,
        json!([brokerage_trade("bad", "buy", "2", "10", "100", "0")]),
    );
    {
        let mut api = r.api.0.lock().unwrap();
        let holdings = api.holdings.as_mut().unwrap();
        holdings["securities"].as_array_mut().unwrap().push(json!({"security_id":"unknown","type":"other","name":"Synthetic unsupported","iso_currency_code":"USD"}));
        holdings["holdings"].as_array_mut().unwrap().push(json!({"account_id":"external","security_id":"unknown","quantity":"1","institution_price":"10","institution_value":"10","iso_currency_code":"USD"}));
    }
    let preview = r
        .service
        .preview_investments(&m.id, &|| Ok(()))
        .await
        .unwrap();
    let issues = preview["issues"].as_array().unwrap();
    assert!(issues
        .iter()
        .any(|i| i["reason"] == "tradeValues" && i["securityId"] == "security"));
    assert!(issues
        .iter()
        .any(|i| i["reason"] == "unsupportedInstrument" && i["securityId"] == "unknown"));
    assert!(matches!(
        r.service
            .approve_investments(
                &m.id,
                preview["digest"].as_str().unwrap(),
                BTreeMap::from([("unknown".into(), "1".into())]),
                true,
                &|| Ok(())
            )
            .await,
        Err(SyncError::Review)
    ));
    assert_eq!(r.count("activities"), 0);
    assert!(r.current(&m).await.investment_ledger.is_none());
}

#[tokio::test]
async fn family_consent_metadata_uses_existing_account_request_and_admission_guard() {
    let r = Rig::new().await;
    let c = unmapped_plaid_connection(&r, PlaidProduct::Transactions).await;
    // Old persisted connections had only a Link expiration; that is not consent.
    let mut legacy = serde_json::to_value(&c).unwrap();
    legacy.as_object_mut().unwrap().remove("consent_expires_at");
    legacy.as_object_mut().unwrap().remove("consentExpiresAt");
    let decoded: super::PlaidConnection = serde_json::from_value(legacy).unwrap();
    assert!(decoded.consent_expires_at.is_none());
    let expiry = Utc::now() + Duration::days(10);
    r.api.0.lock().unwrap().accounts["item"]["consent_expiration_time"] = json!(expiry);
    r.api.0.lock().unwrap().calls.clear();
    r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
    let stored = r
        .repo
        .connections()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == c.id)
        .unwrap();
    assert_eq!(stored.consent_expires_at, Some(expiry));
    assert_eq!(stored.expires_at, c.expires_at);
    assert_eq!(r.api.0.lock().unwrap().calls, vec!["accounts"]);
    r.api.0.lock().unwrap().calls.clear();
    assert!(r
        .service
        .accounts(&c.id, &|| Err(SyncError::Changed))
        .await
        .is_err());
    assert!(r.api.0.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn plaid_reviewed_cash_sweep_repairs_historical_snapshots_completes_valuations_and_cleans_unreferenced_opaque_asset(
) {
    use rust_decimal::Decimal;
    use std::collections::BTreeSet;
    use wealthfolio_core::{
        assets::AssetRepositoryTrait,
        fx::{FxService, FxServiceTrait},
        portfolio::{
            coordinator::{
                CoordinatorDeps, FactSources, PortfolioCoordinator, PortfolioJobRequest,
                SilentObserver, WindowCadence,
            },
            economic_events::BasisStatus,
            snapshot::{Position, SnapshotService, SnapshotSource},
            valuation::{ValuationRepositoryTrait, ValuationStatus},
        },
        quotes::{store::QuoteStore, MarketSyncMode, Quote},
    };

    let r = Rig::new().await;
    let (c, m) = r.seed(PlaidProduct::Investments).await;

    // Second local account under the same Plaid connection to verify shared-connection safety.
    let local2 = uuid::Uuid::new_v4().to_string();
    let l2 = local2.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query(
                "INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at)
                 VALUES(?,'Second Merrill','SECURITIES','USD',0,1,0,'HOLDINGS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            )
            .bind::<Text, _>(l2)
            .execute(conn)
            .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let m2 = Mapping {
        id: uuid::Uuid::new_v4().to_string(),
        connection_id: c.id.clone(),
        external_id: "external-2".into(),
        account_id: local2,
        currency: "USD".into(),
        credit_card: false,
        cutover_date: None,
        cursor: None,
        last_success: None,
        status: "initial_review".into(),
        last_error: None,
        decisions: BTreeMap::new(),
        staged: None,
        source_history: json!({}),
        holdings_staged: None,
        opaque_security_ids: Default::default(),
        cash_security_ids: Default::default(),
        investment_ledger: None,
        product: None,
    };
    r.repo
        .add_mapping(m2.clone(), PlaidProduct::Investments)
        .await
        .unwrap();

    let merrill_fixture = |ext_id: &str, sweep_qty: i64| {
        json!({
            "accounts": [{
                "account_id": ext_id,
                "name": "Merrill CMA",
                "type": "investment",
                "subtype": "brokerage",
                "balances": { "current": 1000 + sweep_qty, "iso_currency_code": "USD" }
            }],
            "item": item(),
            "securities": [
                {
                    "security_id": "sec-voo",
                    "ticker_symbol": "VOO",
                    "name": "Vanguard S&P 500 ETF",
                    "type": "etf",
                    "iso_currency_code": "USD"
                },
                {
                    "security_id": "sec-deposit",
                    "ticker_symbol": "DEPOSIT",
                    "name": "Ml Direct Deposit Progrm",
                    "type": "other",
                    "iso_currency_code": "USD"
                }
            ],
            "holdings": [
                {
                    "account_id": ext_id,
                    "security_id": "sec-voo",
                    "quantity": "2",
                    "institution_price": "500",
                    "institution_value": "1000",
                    "cost_basis": "900",
                    "iso_currency_code": "USD"
                },
                {
                    "account_id": ext_id,
                    "security_id": "sec-deposit",
                    "quantity": sweep_qty.to_string(),
                    "institution_price": "1",
                    "institution_value": sweep_qty.to_string(),
                    "cost_basis": null,
                    "iso_currency_code": "USD"
                }
            ]
        })
    };

    // 1. Initial import on both accounts treats DEPOSIT as an opaque manual-quote asset.
    r.api.0.lock().unwrap().holdings = Some(merrill_fixture("external", 250));
    let p1 = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert_eq!(p1["eligibleCashSecurityIds"], json!(["sec-deposit"]));
    r.service
        .approve_holdings(&m.id, p1["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();

    r.api.0.lock().unwrap().holdings = Some(merrill_fixture("external-2", 125));
    let p2 = r
        .service
        .preview_holdings(&m2.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_holdings(&m2.id, p2["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();

    let timezone = r.service.timezone.clone();
    let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
        &timezone.read().unwrap(),
    );
    let today = Utc::now().with_timezone(&tz).date_naive();
    let day1 = today - chrono::Days::new(4);
    let day2 = today - chrono::Days::new(2);

    let accounts = Arc::new(crate::accounts::AccountRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let assets = Arc::new(crate::assets::AssetRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let snapshots = Arc::new(crate::portfolio::snapshot::SnapshotRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let valuations = Arc::new(crate::portfolio::valuation::ValuationRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let projections = Arc::new(crate::portfolio::projection::ProjectionStore::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let fx_rates = Arc::new(crate::fx::FxRepository::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let fx = Arc::new(FxService::new(fx_rates.clone()));
    fx.initialize().unwrap();
    let snapshot_service = Arc::new(SnapshotService::new(
        timezone.clone(),
        accounts.clone(),
        snapshots.clone(),
        r.service.activities.clone(),
    ));

    let current_snaps = snapshots
        .get_snapshots_by_account(&m.account_id, None, None)
        .unwrap();
    assert_eq!(current_snaps.len(), 1);
    let opaque_id = current_snaps[0]
        .positions
        .keys()
        .find(|id| id.starts_with("plaid-"))
        .unwrap()
        .clone();
    let voo_id = current_snaps[0]
        .positions
        .keys()
        .find(|id| *id != &opaque_id)
        .unwrap()
        .clone();

    // Seed two earlier BROKER_IMPORTED snapshots where DEPOSIT has no historical quotes:
    // - day1 has both relational snapshot_positions and JSON
    // - day2 simulates pre-PR #49 where snapshot_positions lost the opaque row via cascade
    let mk_hist_snap = |date: chrono::NaiveDate, sweep_qty: i64| {
        let now = Utc::now();
        let mut voo = Position::new(m.account_id.clone(), voo_id.clone(), "USD".into(), now);
        voo.quantity = Decimal::from(2);
        voo.total_cost_basis = Decimal::from(900);
        voo.average_cost = Decimal::from(450);
        voo.cost_basis_status = Some(BasisStatus::Complete);

        let mut sweep = Position::new(m.account_id.clone(), opaque_id.clone(), "USD".into(), now);
        sweep.quantity = Decimal::from(sweep_qty);
        sweep.total_cost_basis = Decimal::ZERO;
        sweep.average_cost = Decimal::ZERO;
        sweep.cost_basis_status = Some(BasisStatus::Unknown);

        AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id(&m.account_id, date),
            account_id: m.account_id.clone(),
            snapshot_date: date,
            currency: "USD".into(),
            positions: HashMap::from([(voo_id.clone(), voo), (opaque_id.clone(), sweep)]),
            cash_balances: HashMap::from([("USD".into(), Decimal::ZERO)]),
            cost_basis: Decimal::from(900),
            cash_total_account_currency: Decimal::ZERO,
            source: SnapshotSource::BrokerImported,
            calculated_at: now.naive_utc(),
            ..Default::default()
        }
    };
    let snap_day1 = mk_hist_snap(day1, 200);
    let snap_day2 = mk_hist_snap(day2, 220);
    snapshots
        .save_snapshots(&[snap_day1, snap_day2.clone()])
        .await
        .unwrap();
    {
        let sid = snap_day2.id.clone();
        let oid = opaque_id.clone();
        r.writer
            .exec(move |conn| {
                diesel::sql_query(
                    "DELETE FROM snapshot_positions WHERE snapshot_id = ? AND asset_id = ?",
                )
                .bind::<Text, _>(&sid)
                .bind::<Text, _>(&oid)
                .execute(conn)
                .map_err(sql)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    // Seed VOO market quotes from day1..today, leaving opaque DEPOSIT without pre-today quotes.
    let market = crate::market_data::MarketDataRepository::new(r.pool.clone(), r.writer.clone());
    let mut voo_quotes = Vec::new();
    for offset in 0..=4 {
        let d = day1 + chrono::Days::new(offset);
        let p = Decimal::from(480 + offset * 5);
        voo_quotes.push(Quote {
            id: format!("{voo_id}_{d}_YAHOO"),
            asset_id: voo_id.clone(),
            timestamp: d.and_hms_opt(20, 0, 0).unwrap().and_utc(),
            open: p,
            high: p,
            low: p,
            close: p,
            adjclose: p,
            volume: Decimal::ZERO,
            currency: "USD".into(),
            data_source: "YAHOO".into(),
            created_at: Utc::now(),
            notes: None,
        });
    }
    market.upsert_quotes(&voo_quotes).await.unwrap();

    let coordinator = PortfolioCoordinator::new(CoordinatorDeps {
        base_currency: Arc::new(RwLock::new("USD".into())),
        timezone,
        sources: FactSources {
            accounts: accounts.clone(),
            activities: r.service.activities.clone(),
            assets: assets.clone(),
            quotes: r.quotes.clone(),
            fx_rates,
            snapshots: snapshots.clone(),
            projections: projections.clone(),
        },
        fx_service: fx,
        snapshot_service,
        projections,
        lots: Arc::new(crate::lots::LotsRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        window_cadence: WindowCadence::Days(2),
    });

    // 2. Reproduce pre-fix failure: day1 valuation is PartialUnpriced & BasisStatus::Unknown.
    coordinator
        .run_job(
            PortfolioJobRequest {
                account_ids: Some(vec![m.account_id.clone()]),
                market_sync: MarketSyncMode::None,
                force_full: false,
            },
            &SilentObserver,
        )
        .await
        .unwrap();
    let before = valuations
        .get_historical_valuations(&m.account_id, Some(day1), Some(today))
        .unwrap();
    let day1_before = before.iter().find(|v| v.valuation_date == day1).unwrap();
    assert_eq!(day1_before.value_status, ValuationStatus::PartialUnpriced);
    assert_eq!(day1_before.basis_status, BasisStatus::PartialUnknown);

    // 3. Reclassify DEPOSIT as cash on first account; verify VOO cannot be marked as cash.
    r.api.0.lock().unwrap().holdings = Some(merrill_fixture("external", 250));
    let review = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    let digest = review["digest"].as_str().unwrap();
    assert!(matches!(
        r.service
            .approve_holdings_with_cash(
                &m.id,
                digest,
                false,
                Some(BTreeSet::from(["sec-voo".into()])),
                &|| Ok(()),
            )
            .await,
        Err(SyncError::Review)
    ));
    r.service
        .approve_holdings_with_cash(
            &m.id,
            digest,
            false,
            Some(BTreeSet::from(["sec-deposit".into()])),
            &|| Ok(()),
        )
        .await
        .unwrap();

    let updated_m = r.current(&m).await;
    assert!(updated_m.cash_security_ids.contains("sec-deposit"));
    assert!(!updated_m.opaque_security_ids.contains("sec-deposit"));
    // Second account still references opaque_id, so shared asset must remain until m2 reclassifies.
    assert!(assets.get_by_id(&opaque_id).is_ok());

    coordinator
        .run_job(
            PortfolioJobRequest {
                account_ids: Some(vec![m.account_id.clone()]),
                market_sync: MarketSyncMode::None,
                force_full: false,
            },
            &SilentObserver,
        )
        .await
        .unwrap();

    let after = valuations
        .get_historical_valuations(&m.account_id, Some(day1), Some(today))
        .unwrap();
    assert_eq!(after.len(), 5);
    for v in &after {
        assert_eq!(
            v.value_status,
            ValuationStatus::Complete,
            "{}",
            v.valuation_date
        );
        assert_eq!(
            v.basis_status,
            BasisStatus::Complete,
            "{}",
            v.valuation_date
        );
        assert_eq!(
            v.cost_basis_base,
            Decimal::from(900),
            "{}",
            v.valuation_date
        );
    }
    let v_day1 = after.iter().find(|v| v.valuation_date == day1).unwrap();
    assert_eq!(v_day1.cash_balance_base, Decimal::from(200));
    assert_eq!(v_day1.investment_market_value_base, Decimal::from(960));
    assert_eq!(v_day1.total_value_base, Decimal::from(1160));

    let v_day2 = after.iter().find(|v| v.valuation_date == day2).unwrap();
    assert_eq!(v_day2.cash_balance_base, Decimal::from(220));
    assert_eq!(v_day2.investment_market_value_base, Decimal::from(980));
    assert_eq!(v_day2.total_value_base, Decimal::from(1200));

    let v_today = after.iter().find(|v| v.valuation_date == today).unwrap();
    assert_eq!(v_today.cash_balance_base, Decimal::from(250));
    assert_eq!(v_today.investment_market_value_base, Decimal::from(1000));
    assert_eq!(v_today.total_value_base, Decimal::from(1250));

    // 4. Reclassify DEPOSIT as cash on second account; unreferenced opaque asset + quotes are deleted.
    r.api.0.lock().unwrap().holdings = Some(merrill_fixture("external-2", 125));
    let review2 = r
        .service
        .preview_holdings(&m2.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_holdings_with_cash(
            &m2.id,
            review2["digest"].as_str().unwrap(),
            false,
            Some(BTreeSet::from(["sec-deposit".into()])),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert!(assets.get_by_id(&opaque_id).is_err());

    // 5. Subsequent sync preserves cash_security_ids without recreating the opaque asset.
    {
        let m2_id = m2.id.clone();
        r.writer
            .exec(move |conn| {
                diesel::sql_query("DELETE FROM plaid_accounts WHERE id = ?")
                    .bind::<Text, _>(&m2_id)
                    .execute(conn)
                    .map_err(sql)?;
                Ok(())
            })
            .await
            .unwrap();
    }
    r.api.0.lock().unwrap().holdings = Some(merrill_fixture("external", 275));
    let sync_out = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(sync_out["failures"], json!([]));
    assert!(assets.get_by_id(&opaque_id).is_err());
    let final_snaps = snapshots
        .get_snapshots_by_account(&m.account_id, Some(today), Some(today))
        .unwrap();
    assert_eq!(
        final_snaps[0].cash_total_account_currency,
        Decimal::from(275)
    );
    assert_eq!(final_snaps[0].positions.len(), 1);
}

async fn review_plaid_cash(
    r: &Rig,
    m: &Mapping,
    ids: &[&str],
    allow_opaque: bool,
) -> std::result::Result<(), SyncError> {
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings_with_cash(
            &m.id,
            p["digest"].as_str().unwrap(),
            allow_opaque,
            Some(ids.iter().map(|id| id.to_string()).collect()),
            &|| Ok(()),
        )
        .await
}

#[tokio::test]
async fn plaid_reviewed_cash_sweep_fails_closed_on_unsafe_reviews_history_and_later_syncs() {
    use rust_decimal::Decimal;
    use wealthfolio_core::{
        portfolio::snapshot::{Position, SnapshotSource},
        quotes::{store::QuoteStore, Quote},
    };

    let r = Rig::new().await;
    let (_c, m) = r.seed(PlaidProduct::Investments).await;
    let fixture = |price: &str, value: &str, with_deposit: bool| {
        let mut securities = vec![json!({
            "security_id": "sec-voo",
            "ticker_symbol": "VOO",
            "name": "Vanguard S&P 500 ETF",
            "type": "etf",
            "iso_currency_code": "USD"
        })];
        let mut holdings = vec![json!({
            "account_id": "external",
            "security_id": "sec-voo",
            "quantity": "2",
            "institution_price": "500",
            "institution_value": "1000",
            "cost_basis": "900",
            "iso_currency_code": "USD"
        })];
        if with_deposit {
            securities.push(json!({
                "security_id": "sec-deposit",
                "ticker_symbol": "DEPOSIT",
                "name": "Ml Direct Deposit Progrm",
                "type": "other",
                "iso_currency_code": "USD"
            }));
            holdings.push(json!({
                "account_id": "external",
                "security_id": "sec-deposit",
                "quantity": "250",
                "institution_price": price,
                "institution_value": value,
                "cost_basis": null,
                "iso_currency_code": "USD"
            }));
        }
        let current = if with_deposit {
            1000.0 + value.parse::<f64>().unwrap()
        } else {
            1000.0
        };
        json!({
            "accounts": [{
                "account_id": "external",
                "name": "Merrill CMA",
                "type": "investment",
                "subtype": "brokerage",
                "balances": { "current": current, "iso_currency_code": "USD" }
            }],
            "item": item(),
            "securities": securities,
            "holdings": holdings
        })
    };

    // Opaque import first, as existing Merrill users already have.
    r.api.0.lock().unwrap().holdings = Some(fixture("1", "250", true));
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();
    let snapshots =
        crate::portfolio::snapshot::SnapshotRepository::new(r.pool.clone(), r.writer.clone());
    let current = snapshots
        .get_snapshots_by_account(&m.account_id, None, None)
        .unwrap();
    let opaque_id = current[0]
        .positions
        .keys()
        .find(|id| id.starts_with("plaid-"))
        .unwrap()
        .clone();

    // Owner review rejects unknown securities and non-par sweep balances.
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-missing"], false).await,
        Err(SyncError::Review)
    ));
    r.api.0.lock().unwrap().holdings = Some(fixture("1.05", "262.5", true));
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
        Err(SyncError::Review)
    ));
    r.api.0.lock().unwrap().holdings = Some(fixture("1", "250", true));

    let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
        &r.service.timezone.read().unwrap(),
    );
    let day1 = Utc::now().with_timezone(&tz).date_naive() - chrono::Days::new(4);
    let hist = |currency: &str, qty: i64| {
        let now = Utc::now();
        let mut sweep = Position::new(m.account_id.clone(), opaque_id.clone(), "USD".into(), now);
        sweep.quantity = Decimal::from(qty);
        AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id(&m.account_id, day1),
            account_id: m.account_id.clone(),
            snapshot_date: day1,
            currency: currency.into(),
            positions: HashMap::from([(opaque_id.clone(), sweep)]),
            cash_balances: HashMap::from([(currency.into(), Decimal::ZERO)]),
            base_currency: Some("USD".into()),
            source: SnapshotSource::BrokerImported,
            calculated_at: now.naive_utc(),
            ..Default::default()
        }
    };

    // A historical non-par quote proves the asset was not a $1 sweep.
    let market = crate::market_data::MarketDataRepository::new(r.pool.clone(), r.writer.clone());
    let bad_quote = format!("{opaque_id}_{day1}_MANUAL");
    market
        .upsert_quotes(&[Quote {
            id: bad_quote.clone(),
            asset_id: opaque_id.clone(),
            timestamp: day1.and_hms_opt(20, 0, 0).unwrap().and_utc(),
            open: Decimal::new(15, 1),
            high: Decimal::new(15, 1),
            low: Decimal::new(15, 1),
            close: Decimal::new(15, 1),
            adjclose: Decimal::new(15, 1),
            volume: Decimal::ZERO,
            currency: "USD".into(),
            data_source: "MANUAL".into(),
            created_at: Utc::now(),
            notes: None,
        }])
        .await
        .unwrap();
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
        Err(SyncError::Review)
    ));
    r.writer
        .exec(move |conn| {
            diesel::sql_query("DELETE FROM quotes WHERE id = ?")
                .bind::<Text, _>(&bad_quote)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();

    // Snapshots in another currency or with short sweep positions are never repaired.
    for snap in [hist("EUR", 200), hist("USD", -5)] {
        snapshots.save_snapshots(&[snap]).await.unwrap();
        assert!(matches!(
            review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
            Err(SyncError::Review)
        ));
    }

    // Corrupt JSON positions fail closed instead of silently dropping holdings.
    snapshots.save_snapshots(&[hist("USD", 200)]).await.unwrap();
    let corrupt_id = AccountStateSnapshot::stable_id(&m.account_id, day1);
    let restore_id = corrupt_id.clone();
    let original: String = {
        use crate::schema::holdings_snapshots::dsl as hs;
        let mut conn = r.pool.get().unwrap();
        hs::holdings_snapshots
            .find(&corrupt_id)
            .select(hs::positions)
            .first(&mut conn)
            .unwrap()
    };
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE holdings_snapshots SET positions = 'not-json' WHERE id = ?")
                .bind::<Text, _>(&corrupt_id)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
        Err(SyncError::Review)
    ));
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE holdings_snapshots SET positions = ? WHERE id = ?")
                .bind::<Text, _>(&original)
                .bind::<Text, _>(&restore_id)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();

    // Allowing opaque securities never makes a reviewed cash sweep opaque; the
    // repaired history adds the sweep to account and base cash.
    review_plaid_cash(&r, &m, &["sec-deposit"], true)
        .await
        .unwrap();
    let reviewed = r.current(&m).await;
    assert!(reviewed.cash_security_ids.contains("sec-deposit"));
    assert!(!reviewed.opaque_security_ids.contains("sec-deposit"));
    assert!(reviewed.opaque_security_ids.contains("sec-voo"));
    let repaired = snapshots
        .get_snapshots_by_account(&m.account_id, Some(day1), Some(day1))
        .unwrap();
    assert!(repaired[0].positions.is_empty());
    assert_eq!(repaired[0].cash_total_account_currency, Decimal::from(200));
    assert_eq!(repaired[0].cash_total_base_currency, Decimal::from(200));

    // Plaid dropping the zeroed sweep keeps the reviewed security classified as cash.
    r.api.0.lock().unwrap().holdings = Some(fixture("1", "250", false));
    let out = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(out["failures"], json!([]));
    assert!(r
        .current(&m)
        .await
        .cash_security_ids
        .contains("sec-deposit"));

    // A later non-par balance for a reviewed sweep blocks the sync for review.
    r.api.0.lock().unwrap().holdings = Some(fixture("1.05", "262.5", true));
    let out = r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_ne!(out["failures"], json!([]));
}

#[tokio::test]
async fn plaid_reviewed_cash_sweep_deduplicates_legacy_keys_checks_total_value_and_preserves_statement_assets(
) {
    use rust_decimal::Decimal;
    use wealthfolio_core::{
        assets::AssetRepositoryTrait,
        portfolio::snapshot::{Position, SnapshotSource},
        quotes::{store::QuoteStore, Quote},
    };

    let r = Rig::new().await;
    let (_c, m) = r.seed(PlaidProduct::Investments).await;
    let holdings_fixture = json!({
        "accounts": [{
            "account_id": "external",
            "name": "Merrill CMA",
            "type": "investment",
            "subtype": "brokerage",
            "balances": { "current": 1250, "iso_currency_code": "USD" }
        }],
        "item": item(),
        "securities": [
            {
                "security_id": "sec-voo",
                "ticker_symbol": "VOO",
                "name": "Vanguard S&P 500 ETF",
                "type": "etf",
                "iso_currency_code": "USD"
            },
            {
                "security_id": "sec-deposit",
                "ticker_symbol": "DEPOSIT",
                "name": "Ml Direct Deposit Progrm",
                "type": "other",
                "iso_currency_code": "USD"
            }
        ],
        "holdings": [
            {
                "account_id": "external",
                "security_id": "sec-voo",
                "quantity": "2",
                "institution_price": "500",
                "institution_value": "1000",
                "cost_basis": "900",
                "iso_currency_code": "USD"
            },
            {
                "account_id": "external",
                "security_id": "sec-deposit",
                "quantity": "250",
                "institution_price": "1",
                "institution_value": "250",
                "cost_basis": null,
                "iso_currency_code": "USD"
            }
        ]
    });

    r.api.0.lock().unwrap().holdings = Some(holdings_fixture.clone());
    let p = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    r.service
        .approve_holdings(&m.id, p["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();

    let assets = crate::assets::AssetRepository::new(r.pool.clone(), r.writer.clone());
    let snapshots =
        crate::portfolio::snapshot::SnapshotRepository::new(r.pool.clone(), r.writer.clone());
    let market = crate::market_data::MarketDataRepository::new(r.pool.clone(), r.writer.clone());
    let current = snapshots
        .get_snapshots_by_account(&m.account_id, None, None)
        .unwrap();
    let opaque_id = current[0]
        .positions
        .keys()
        .find(|id| id.starts_with("plaid-"))
        .unwrap()
        .clone();

    let tz = wealthfolio_core::utils::time_utils::parse_user_timezone_or_default(
        &r.service.timezone.read().unwrap(),
    );
    let day1 = Utc::now().with_timezone(&tz).date_naive() - chrono::Days::new(4);
    let day2 = day1 + chrono::Days::new(1);
    let day3 = day1 + chrono::Days::new(2);
    let snap3_id = AccountStateSnapshot::stable_id(&m.account_id, day3);
    let mk_snap = |date: chrono::NaiveDate, qty: i64| {
        let now = Utc::now();
        let mut sweep = Position::new(m.account_id.clone(), opaque_id.clone(), "USD".into(), now);
        sweep.quantity = Decimal::from(qty);
        AccountStateSnapshot {
            id: AccountStateSnapshot::stable_id(&m.account_id, date),
            account_id: m.account_id.clone(),
            snapshot_date: date,
            currency: "USD".into(),
            positions: HashMap::from([(opaque_id.clone(), sweep)]),
            cash_balances: HashMap::from([("USD".into(), Decimal::ZERO)]),
            source: SnapshotSource::BrokerImported,
            calculated_at: now.naive_utc(),
            ..Default::default()
        }
    };
    let mk_quote = |date: chrono::NaiveDate, source: &str, price: Decimal| Quote {
        id: format!("{opaque_id}_{date}_{source}"),
        asset_id: opaque_id.clone(),
        timestamp: date.and_hms_opt(20, 0, 0).unwrap().and_utc(),
        open: price,
        high: price,
        low: price,
        close: price,
        adjclose: price,
        volume: Decimal::ZERO,
        currency: "USD".into(),
        data_source: source.into(),
        created_at: Utc::now(),
        notes: None,
    };

    // 1. A $1.0001 quote on day1 passes per-unit tolerance, but a 200,000-unit position
    //    on day1 diverges by $20 from par (> $0.01) and must fail closed.
    snapshots
        .save_snapshots(&[mk_snap(day1, 200_000), mk_snap(day3, 100)])
        .await
        .unwrap();
    let drift_price = Decimal::new(10001, 4);
    market
        .upsert_quotes(&[mk_quote(day1, "MANUAL", drift_price)])
        .await
        .unwrap();
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
        Err(SyncError::Review)
    ));

    // 2. Even when day1 is exact $1.00 and day3 is 100 units at $1.0001, an intervening
    //    $1.0001 BROKER quote on day2 carries day1's 200,000 units ($20 diff > $0.01)
    //    and must fail closed.
    market
        .upsert_quotes(&[
            mk_quote(day1, "MANUAL", Decimal::ONE),
            mk_quote(day2, "BROKER", drift_price),
            mk_quote(day3, "BROKER", drift_price),
        ])
        .await
        .unwrap();
    assert!(matches!(
        review_plaid_cash(&r, &m, &["sec-deposit"], false).await,
        Err(SyncError::Review)
    ));

    // 3. A same-day $1.00 MANUAL quote on day2 overrides the $1.0001 BROKER quote under
    //    valuation precedence, so day1 ($1.00), day2 (MANUAL $1.00), and day3 (100 units
    //    at $1.0001 = $0.01 diff <= $0.01) all pass. Also store day3's sweep under a legacy
    //    JSON key ("DEPOSIT") alongside its relational mirror (opaque_id) to verify it
    //    credits $100 once, not $200.
    market
        .upsert_quotes(&[mk_quote(day2, "MANUAL", Decimal::ONE)])
        .await
        .unwrap();
    {
        let sid = snap3_id.clone();
        let oid = opaque_id.clone();
        let legacy_json: String = {
            use crate::schema::holdings_snapshots::dsl as hs;
            let mut conn = r.pool.get().unwrap();
            let raw: String = hs::holdings_snapshots
                .find(&sid)
                .select(hs::positions)
                .first(&mut conn)
                .unwrap();
            let mut map: HashMap<String, Position> = serde_json::from_str(&raw).unwrap();
            let pos = map.remove(&oid).unwrap();
            map.insert("DEPOSIT".into(), pos);
            serde_json::to_string(&map).unwrap()
        };
        r.writer
            .exec(move |conn| {
                diesel::sql_query("UPDATE holdings_snapshots SET positions = ? WHERE id = ?")
                    .bind::<Text, _>(&legacy_json)
                    .bind::<Text, _>(&sid)
                    .execute(conn)
                    .map_err(sql)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    // 4. A reviewed daily statement referencing opaque_id must prevent automatic
    //    asset + quote deletion even after all observed holdings are reclassified.
    {
        let acc_id = m.account_id.clone();
        let stmt_payload = json!({
            "closing_asset_ids": [opaque_id.clone()],
            "version": 1,
            "interval": {
                "datePolicy": "settlement",
                "from": "2026-10-01",
                "through": "2026-10-31",
                "currency": "USD",
                "openingCash": "0",
                "closingCash": "0",
                "closingValue": "100",
                "closingPositionsComplete": true,
                "activityComplete": true,
                "closingPositions": [{
                    "assetId": opaque_id.clone(),
                    "quantity": "100",
                    "marketValue": "100",
                    "costBasis": null
                }],
                "events": []
            }
        })
        .to_string();
        r.writer
            .exec(move |conn| {
                diesel::sql_query(
                    "INSERT INTO account_statement_history (account_id, statement_date, payload, revision)
                     VALUES (?, '2026-10-31', '{}', 'hist-rev-1')",
                )
                .bind::<Text, _>(&acc_id)
                .execute(conn)
                .map_err(sql)?;
                diesel::sql_query(
                    "INSERT INTO account_statement_reconstructions (account_id, through_date, payload, revision)
                     VALUES (?, '2026-10-31', ?, 'rev-1')",
                )
                .bind::<Text, _>(&acc_id)
                .bind::<Text, _>(&stmt_payload)
                .execute(conn)
                .map_err(sql)?;
                Ok(())
            })
            .await
            .unwrap();
    }

    review_plaid_cash(&r, &m, &["sec-deposit"], false)
        .await
        .unwrap();

    let repaired_day1 = snapshots
        .get_snapshots_by_account(&m.account_id, Some(day1), Some(day1))
        .unwrap();
    assert!(repaired_day1[0].positions.is_empty());
    assert_eq!(
        repaired_day1[0].cash_total_account_currency,
        Decimal::from(200_000)
    );
    let repaired_day3 = snapshots
        .get_snapshots_by_account(&m.account_id, Some(day3), Some(day3))
        .unwrap();
    assert!(repaired_day3[0].positions.is_empty());
    assert_eq!(
        repaired_day3[0].cash_total_account_currency,
        Decimal::from(100)
    );
    assert!(
        assets.get_by_id(&opaque_id).is_ok(),
        "asset referenced by reviewed daily statement must be preserved"
    );
    let remaining_quotes: i64 = {
        use crate::schema::quotes::dsl as q;
        let mut conn = r.pool.get().unwrap();
        q::quotes
            .filter(q::asset_id.eq(&opaque_id))
            .count()
            .get_result(&mut conn)
            .unwrap()
    };
    assert!(
        remaining_quotes > 0,
        "quotes for statement-referenced asset must be preserved"
    );
}

async fn connected_schwab_connection(r: &Rig) -> super::PlaidConnection {
    let mut c = r.connection(PlaidProduct::Investments);
    c.institution = "schwab".into();
    r.repo.put_connection(c.clone(), None).await.unwrap();
    r.vault
        .set_secret(&secret_key(&c, "access").unwrap(), "access")
        .unwrap();
    c
}

#[tokio::test]
async fn plaid_schwab_hosted_link_and_reconnect_request_transactions_consent_without_exchanging() {
    let r = Rig::new().await;
    let started = r
        .service
        .start("owner", "schwab", None, false, &|| Ok(()))
        .await
        .unwrap();
    let flow = started["flowId"].as_str().unwrap().to_string();
    assert_eq!(r.api.0.lock().unwrap().link_transactions, vec![true]);
    r.service.finish("owner", &flow, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("exchange"), 1);
    let connections = r.repo.connections().await.unwrap();
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0].institution, "schwab");
    assert_eq!(connections[0].product, PlaidProduct::Investments);
    assert_eq!(connections[0].status, "connected");

    // Reconnect reuses the same Item and also requests lazy Transactions consent.
    let reconnected = r
        .service
        .start("owner", "schwab", Some(&flow), false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(reconnected["flowId"], flow);
    assert_eq!(r.api.0.lock().unwrap().link_transactions, vec![true, true]);
    r.service.finish("owner", &flow, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("exchange"), 1);
    assert_eq!(r.count("plaid_connections"), 1);
}

#[tokio::test]
async fn plaid_schwab_brokerage_links_existing_statement_history_and_imports_holdings() {
    use wealthfolio_core::portfolio::statements::{
        StatementPosition, StatementRepository, StatementService, StatementSnapshot, StatementWrite,
    };

    let r = Rig::new().await;
    let c = connected_schwab_connection(&r).await;
    let mut brokerage = account("investment");
    brokerage["account_id"] = json!("schwab-brokerage");
    brokerage["subtype"] = json!("brokerage");
    let mut cma = account("depository");
    cma["account_id"] = json!("schwab-cma");
    cma["subtype"] = json!("cash management");
    r.api.0.lock().unwrap().accounts["accounts"] = json!([brokerage.clone(), cma.clone()]);

    // Existing manual SECURITIES/HOLDINGS account with schwab-monthly-v1 statements.
    let local = uuid::Uuid::new_v4().to_string();
    let local_id = local.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query(
                "INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Schwab Brokerage','SECURITIES','USD',0,1,0,'HOLDINGS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            )
            .bind::<Text, _>(local_id)
            .execute(conn)
            .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();

    let statements = Arc::new(crate::portfolio::statements::StatementStore::new(
        r.pool.clone(),
        r.writer.clone(),
    ));
    let statement_service = StatementService::new(
        statements.clone(),
        Arc::new(crate::accounts::AccountRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        Arc::new(crate::portfolio::valuation::ValuationRepository::new(
            r.pool.clone(),
            r.writer.clone(),
        )),
        Arc::new(RwLock::new("America/Los_Angeles".into())),
    );
    let statement_date = chrono::NaiveDate::from_ymd_opt(2025, 10, 31).unwrap();
    statement_service
        .save(
            local.clone(),
            vec![StatementWrite {
                snapshot: StatementSnapshot {
                    date: statement_date,
                    currency: "USD".into(),
                    total_value: "250.00".into(),
                    cash_value: Some("150.00".into()),
                    positions: vec![StatementPosition {
                        label: "Synthetic Treasury".into(),
                        quantity: "2.5".into(),
                        market_value: Some("100.00".into()),
                        cost_basis: Some("80.00".into()),
                    }],
                    positions_complete: true,
                    format: "schwab-monthly-v1".into(),
                    source_page: 1,
                },
                expected_revision: None,
            }],
            true,
        )
        .await
        .unwrap();

    // Schwab does not require Merrill's self-directed attestation flag.
    r.service
        .map_account(&c.id, "schwab-brokerage", &local, false, None, &|| Ok(()))
        .await
        .unwrap();
    let created_cma = r
        .service
        .create_and_map(&c.id, "schwab-cma", "Schwab Cash Mgmt", false, &|| Ok(()))
        .await
        .unwrap();
    assert!(created_cma.created);

    let m = r
        .repo
        .mappings()
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.external_id == "schwab-brokerage")
        .unwrap();
    assert_eq!(m.product(&c), PlaidProduct::Investments);
    assert_eq!(m.product, Some(PlaidProduct::Investments));

    // Opaque fixed-income holding requires allow_opaque=true before approval succeeds.
    let mut holdings = brokerage_fixture("fixed income", None, json!("80"));
    holdings["accounts"] = json!([brokerage.clone()]);
    holdings["holdings"][0]["account_id"] = json!("schwab-brokerage");
    r.api.0.lock().unwrap().holdings = Some(holdings);
    r.api.0.lock().unwrap().investments.push_back(json!({
        "accounts": [brokerage],
        "item": item(),
        "investment_transactions": [],
        "securities": [],
        "total_investment_transactions": 0
    }));
    let preview = r.service.preview_holdings(&m.id, &|| Ok(())).await.unwrap();
    assert!(matches!(
        r.service
            .approve_holdings(
                &m.id,
                preview["digest"].as_str().unwrap(),
                false,
                &|| Ok(())
            )
            .await,
        Err(SyncError::Review)
    ));
    r.service
        .approve_holdings(&m.id, preview["digest"].as_str().unwrap(), true, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(r.count("holdings_snapshots"), 1);
    assert_eq!(r.count("activities"), 0);
    // Existing schwab-monthly-v1 statement history remains untouched.
    let stored_statements = statements.list(&local).unwrap();
    assert_eq!(stored_statements.len(), 1);
    assert_eq!(stored_statements[0].snapshot.format, "schwab-monthly-v1");
    assert_eq!(stored_statements[0].snapshot.date, statement_date);
}

#[tokio::test]
async fn plaid_schwab_shared_item_bank_accounts_sync_alongside_brokerage_and_guard_product_drift() {
    let r = Rig::new().await;
    let c = connected_schwab_connection(&r).await;
    let mut brokerage = account("investment");
    brokerage["account_id"] = json!("schwab-brokerage");
    brokerage["subtype"] = json!("brokerage");
    let mut checking = account("depository");
    checking["account_id"] = json!("schwab-checking");
    checking["name"] = json!("Schwab Bank Investor Checking");
    checking["subtype"] = json!("checking");
    let mut savings = account("depository");
    savings["account_id"] = json!("schwab-savings");
    savings["name"] = json!("Schwab Bank Savings");
    savings["subtype"] = json!("savings");
    savings["balances"]["current"] = json!(1000);
    let mut card = account("credit");
    card["account_id"] = json!("unsupported-card");
    r.api.0.lock().unwrap().accounts["accounts"] = json!([
        brokerage.clone(),
        checking.clone(),
        savings.clone(),
        card.clone()
    ]);

    let discovered = r.service.accounts(&c.id, &|| Ok(())).await.unwrap();
    let accounts = discovered["accounts"].as_array().unwrap();
    assert_eq!(discovered["totalAccounts"], 4);
    assert_eq!(accounts.len(), 3);
    assert_eq!(accounts[0]["id"], "schwab-brokerage");
    assert_eq!(accounts[0]["product"], "investments");
    assert_eq!(accounts[1]["id"], "schwab-checking");
    assert_eq!(accounts[1]["product"], "transactions");
    assert_eq!(accounts[2]["id"], "schwab-savings");
    assert_eq!(accounts[2]["product"], "transactions");

    // Credit cards remain rejected on a Schwab connection.
    assert!(matches!(
        r.service
            .create_and_map(
                &c.id,
                "unsupported-card",
                "Unsupported Card",
                false,
                &|| { Ok(()) }
            )
            .await,
        Err(SyncError::Review)
    ));

    let created_brokerage = r
        .service
        .create_and_map(
            &c.id,
            "schwab-brokerage",
            "Schwab Brokerage",
            false,
            &|| Ok(()),
        )
        .await
        .unwrap();
    let created_checking = r
        .service
        .create_and_map(&c.id, "schwab-checking", "Schwab Checking", false, &|| {
            Ok(())
        })
        .await
        .unwrap();
    let created_savings = r
        .service
        .create_and_map(&c.id, "schwab-savings", "Schwab Savings", false, &|| Ok(()))
        .await
        .unwrap();
    assert!(created_brokerage.created && created_checking.created && created_savings.created);
    assert_ne!(created_checking.account_id, created_savings.account_id);
    // Idempotent retry on checking returns the existing mapping without creating another account.
    let retry_checking = r
        .service
        .create_and_map(&c.id, "schwab-checking", "Schwab Checking", false, &|| {
            Ok(())
        })
        .await
        .unwrap();
    assert!(!retry_checking.created);
    assert_eq!(retry_checking.mapping_id, created_checking.mapping_id);

    let mappings = r.repo.mappings().await.unwrap();
    let brokerage_map = mappings
        .iter()
        .find(|m| m.id == created_brokerage.mapping_id)
        .unwrap()
        .clone();
    let checking_map = mappings
        .iter()
        .find(|m| m.id == created_checking.mapping_id)
        .unwrap()
        .clone();
    let savings_map = mappings
        .iter()
        .find(|m| m.id == created_savings.mapping_id)
        .unwrap()
        .clone();
    assert!(!checking_map.credit_card && !savings_map.credit_card);
    assert_eq!(checking_map.product, Some(PlaidProduct::Transactions));
    assert_eq!(checking_map.product(&c), PlaidProduct::Transactions);
    assert_eq!(savings_map.product, Some(PlaidProduct::Transactions));
    assert_eq!(savings_map.product(&c), PlaidProduct::Transactions);
    assert_eq!(brokerage_map.product, Some(PlaidProduct::Investments));
    assert_eq!(brokerage_map.product(&c), PlaidProduct::Investments);

    // Verify local account settings: CASH/TRANSACTIONS for checking and savings, SECURITIES/HOLDINGS for brokerage.
    #[derive(QueryableByName)]
    struct LocalAccount {
        #[diesel(sql_type = Text)]
        account_type: String,
        #[diesel(sql_type = Text)]
        tracking_mode: String,
    }
    for local_id in [&checking_map.account_id, &savings_map.account_id] {
        let bank_local =
            diesel::sql_query("SELECT account_type, tracking_mode FROM accounts WHERE id = ?")
                .bind::<Text, _>(local_id)
                .get_result::<LocalAccount>(&mut r.pool.get().unwrap())
                .unwrap();
        assert_eq!(bank_local.account_type, "CASH");
        assert_eq!(bank_local.tracking_mode, "TRANSACTIONS");
    }

    // Cross-product endpoints fail closed.
    assert!(r
        .service
        .preview_holdings(&checking_map.id, &|| Ok(()))
        .await
        .is_err());
    assert!(r
        .service
        .preview_holdings(&savings_map.id, &|| Ok(()))
        .await
        .is_err());
    assert!(r
        .service
        .preview(&brokerage_map.id, false, &|| Ok(()))
        .await
        .is_err());

    let push_bank_page = |records: Value, cursor: &str| {
        r.api.0.lock().unwrap().bank.push_back(Ok(json!({
            "added": records,
            "modified": [],
            "removed": [],
            "next_cursor": cursor,
            "has_more": false,
            "transactions_update_status": "HISTORICAL_UPDATE_COMPLETE"
        })));
    };

    // Review and approve Schwab Bank checking import (positive bank balance sign).
    let mut checking_tx = transaction("schwab-debit", "25", "FOOD_AND_DRINK");
    checking_tx["account_id"] = json!("schwab-checking");
    push_bank_page(json!([checking_tx]), "checking-cursor-1");
    let checking_preview = r
        .service
        .preview(&checking_map.id, false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(checking_preview["balance"], "250");
    assert_eq!(checking_preview["suggestedOpening"], "275");
    r.service
        .approve(
            &checking_map.id,
            checking_preview["digest"].as_str().unwrap(),
            checking_preview["suggestedOpening"]
                .as_str()
                .map(str::to_owned),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        r.current(&checking_map).await.cursor.as_deref(),
        Some("checking-cursor-1")
    );

    // Review and approve Schwab Bank savings import with its own transaction and cursor.
    let mut savings_tx = transaction("schwab-savings-interest", "-12", "INCOME");
    savings_tx["account_id"] = json!("schwab-savings");
    push_bank_page(json!([savings_tx]), "savings-cursor-1");
    let savings_preview = r
        .service
        .preview(&savings_map.id, false, &|| Ok(()))
        .await
        .unwrap();
    assert_eq!(savings_preview["balance"], "1000");
    assert_eq!(savings_preview["suggestedOpening"], "988");
    r.service
        .approve(
            &savings_map.id,
            savings_preview["digest"].as_str().unwrap(),
            savings_preview["suggestedOpening"]
                .as_str()
                .map(str::to_owned),
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(
        r.current(&savings_map).await.cursor.as_deref(),
        Some("savings-cursor-1")
    );

    // Approve brokerage holdings on the same connection.
    let mut holdings = brokerage_fixture("equity", Some("SCHB"), json!("80"));
    holdings["accounts"] = json!([brokerage.clone()]);
    holdings["holdings"][0]["account_id"] = json!("schwab-brokerage");
    r.api.0.lock().unwrap().holdings = Some(holdings);
    r.api.0.lock().unwrap().investments.push_back(json!({
        "accounts": [brokerage.clone()],
        "item": item(),
        "investment_transactions": [],
        "securities": [],
        "total_investment_transactions": 0
    }));
    let holdings_preview = r
        .service
        .preview_holdings(&brokerage_map.id, &|| Ok(()))
        .await
        .unwrap();
    r.service
        .approve_holdings(
            &brokerage_map.id,
            holdings_preview["digest"].as_str().unwrap(),
            false,
            &|| Ok(()),
        )
        .await
        .unwrap();
    assert_eq!(r.count("holdings_snapshots"), 1);

    // One sync updates brokerage, checking, and savings independently on the single Schwab Item.
    r.api.0.lock().unwrap().investments.push_back(json!({
        "accounts": [brokerage.clone()],
        "item": item(),
        "investment_transactions": [],
        "securities": [],
        "total_investment_transactions": 0
    }));
    let mut checking_tx_2 = transaction("schwab-debit-2", "15", "FOOD_AND_DRINK");
    checking_tx_2["account_id"] = json!("schwab-checking");
    checking_tx_2["date"] = json!("2026-11-02");
    let mut savings_tx_2 = transaction("schwab-savings-interest-2", "-8", "INCOME");
    savings_tx_2["account_id"] = json!("schwab-savings");
    savings_tx_2["date"] = json!("2026-11-02");
    push_bank_page(json!([checking_tx_2]), "checking-cursor-2");
    push_bank_page(json!([savings_tx_2]), "savings-cursor-2");
    let holding_calls = r.api.calls("holdings");
    let transaction_calls = r.api.calls("transactions");
    r.service.sync(false, &|| Ok(())).await.unwrap();
    assert_eq!(r.api.calls("holdings"), holding_calls + 1);
    assert_eq!(r.api.calls("transactions"), transaction_calls + 2);
    assert_eq!(r.count("plaid_connections"), 1);
    assert_eq!(
        r.current(&checking_map).await.cursor.as_deref(),
        Some("checking-cursor-2")
    );
    assert_eq!(
        r.current(&savings_map).await.cursor.as_deref(),
        Some("savings-cursor-2")
    );
    assert_eq!(
        r.api.0.lock().unwrap().transaction_requests,
        vec![
            ("schwab-checking".into(), None),
            ("schwab-savings".into(), None),
            ("schwab-checking".into(), Some("checking-cursor-1".into())),
            ("schwab-savings".into(), Some("savings-cursor-1".into())),
        ]
    );

    // Assert each bank account's activities land only in its own local CASH account.
    #[derive(QueryableByName)]
    struct SourceRecord {
        #[diesel(sql_type = Text)]
        source_record_id: String,
    }
    let source_records_for = |account_id: &str| {
        let mut records: Vec<String> = diesel::sql_query(
            "SELECT source_record_id FROM activities WHERE account_id = ? ORDER BY source_record_id",
        )
        .bind::<Text, _>(account_id)
        .load::<SourceRecord>(&mut r.pool.get().unwrap())
        .unwrap()
        .into_iter()
        .map(|row| row.source_record_id)
        .collect();
        records.sort();
        records
    };
    assert_eq!(
        source_records_for(&checking_map.account_id),
        vec!["opening-balance", "schwab-debit", "schwab-debit-2"]
    );
    assert_eq!(
        source_records_for(&savings_map.account_id),
        vec![
            "opening-balance",
            "schwab-savings-interest",
            "schwab-savings-interest-2"
        ]
    );
    assert!(source_records_for(&brokerage_map.account_id).is_empty());

    let status = r.service.list(&|| Ok(())).await.unwrap();
    let status_accounts = status["connections"][0]["accounts"].as_array().unwrap();
    assert_eq!(
        status_accounts
            .iter()
            .find(|a| a["id"] == checking_map.id)
            .unwrap()["product"],
        "transactions"
    );
    assert_eq!(
        status_accounts
            .iter()
            .find(|a| a["id"] == savings_map.id)
            .unwrap()["product"],
        "transactions"
    );
    assert_eq!(
        status_accounts
            .iter()
            .find(|a| a["id"] == brokerage_map.id)
            .unwrap()["product"],
        "investments"
    );

    // Mapping product is immutable in SQLite.
    let mut tampered = r.current(&checking_map).await;
    tampered.product = Some(PlaidProduct::Investments);
    assert!(matches!(
        r.repo.put_mappings(vec![tampered]).await,
        Err(SyncError::Changed)
    ));

    // Reclassifying provider subtype between checking and cash management fails closed.
    let mut drifted_checking = checking.clone();
    drifted_checking["subtype"] = json!("cash management");
    r.api.0.lock().unwrap().accounts["accounts"] = json!([brokerage.clone(), drifted_checking]);
    assert!(matches!(
        r.service.preview(&checking_map.id, true, &|| Ok(())).await,
        Err(SyncError::Review)
    ));

    let mut drifted_brokerage = checking.clone();
    drifted_brokerage["account_id"] = json!("schwab-brokerage");
    r.api.0.lock().unwrap().holdings.as_mut().unwrap()["accounts"] = json!([drifted_brokerage]);
    assert!(matches!(
        r.service
            .preview_holdings(&brokerage_map.id, &|| Ok(()))
            .await,
        Err(SyncError::Review)
    ));

    // Repository enforces institution/product invariants (e.g. Merrill checking is forbidden).
    let mut merrill_conn = r.connection(PlaidProduct::Investments);
    merrill_conn.item_id = Some("merrill-item".into());
    r.repo
        .put_connection(merrill_conn.clone(), None)
        .await
        .unwrap();
    let cash_local = uuid::Uuid::new_v4().to_string();
    let cash_id = cash_local.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query(
                "INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Merrill Cash','CASH','USD',0,1,0,'TRANSACTIONS',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)",
            )
            .bind::<Text, _>(cash_id)
            .execute(conn)
            .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let mut forbidden = checking_map.clone();
    forbidden.id = uuid::Uuid::new_v4().to_string();
    forbidden.connection_id = merrill_conn.id.clone();
    forbidden.account_id = cash_local;
    assert!(matches!(
        r.repo
            .add_mapping(forbidden.clone(), PlaidProduct::Transactions)
            .await,
        Err(SyncError::Changed)
    ));
    let forbidden_account = wealthfolio_core::accounts::NewAccount {
        id: None,
        name: "Merrill Cash".into(),
        account_type: wealthfolio_core::accounts::account_types::CASH.into(),
        currency: "USD".into(),
        group: None,
        is_default: false,
        is_active: true,
        platform_id: None,
        account_number: None,
        meta: None,
        provider: Some(wealthfolio_connect::plaid::DIRECT_PLAID_PROVIDER.into()),
        provider_account_id: Some(forbidden.id.clone()),
        is_archived: false,
        tracking_mode: wealthfolio_core::accounts::TrackingMode::Transactions,
    };
    assert!(matches!(
        r.repo
            .create_mapped_account(
                forbidden,
                PlaidProduct::Transactions,
                forbidden_account.clone()
            )
            .await,
        Err(SyncError::Changed)
    ));
    // Retrying create_mapped_account for an existing external_id with a different product fails.
    let mut wrong_product_retry = checking_map.clone();
    wrong_product_retry.id = uuid::Uuid::new_v4().to_string();
    wrong_product_retry.product = Some(PlaidProduct::Investments);
    assert!(matches!(
        r.repo
            .create_mapped_account(
                wrong_product_retry,
                PlaidProduct::Investments,
                forbidden_account
            )
            .await,
        Err(SyncError::Changed)
    ));
    // Persisted corrupted product fails ensure_connected during commit.
    let mut bad_stored = r.current(&checking_map).await;
    bad_stored.credit_card = true;
    let bad_body = serde_json::to_string(&bad_stored).unwrap();
    let bad_id = bad_stored.id.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE plaid_accounts SET body=? WHERE id=?")
                .bind::<Text, _>(bad_body)
                .bind::<Text, _>(bad_id)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    let expected_cursor = bad_stored.cursor.clone();
    assert!(matches!(
        r.repo
            .commit_banks(vec![BankCommit {
                mapping: bad_stored,
                expected_cursor,
                activities: vec![],
                earliest: None,
                removed_ids: vec![],
            }])
            .await,
        Err(SyncError::Review)
    ));
}
