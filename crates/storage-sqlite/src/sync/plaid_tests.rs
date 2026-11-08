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
    ) -> std::result::Result<HostedLink, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "link")?;
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
        _: &str,
        _: Option<&str>,
    ) -> std::result::Result<TransactionsPage, PlaidError> {
        let mut s = self.0.lock().unwrap();
        Self::check(&mut s, "transactions")?;
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
        let assets = Arc::new(AssetService::new(assets, quotes).unwrap());
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
                ("amex".into(), "ins_1".into()),
            ]),
            assets,
            activities,
            events: events.clone(),
            timezone: Arc::new(RwLock::new("America/Los_Angeles".into())),
        };
        Self {
            service,
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
            update_mode: false,
        }
    }
    async fn seed(&self, product: PlaidProduct) -> (super::PlaidConnection, Mapping) {
        let c = self.connection(product);
        self.repo.put_connection(c.clone(), None).await.unwrap();
        self.vault
            .set_secret(&secret_key(&c, "access").unwrap(), "access")
            .unwrap();
        let local = uuid::Uuid::new_v4().to_string();
        let l = local.clone();
        self.writer.exec(move|conn|{diesel::sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,is_archived,tracking_mode,created_at,updated_at) VALUES(?,'Synthetic',?,'USD',0,1,0,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)").bind::<Text,_>(l).bind::<Text,_>(if product==PlaidProduct::Investments{"SECURITIES"}else{"CASH"}).bind::<Text,_>(if product==PlaidProduct::Investments{"HOLDINGS"}else{"TRANSACTIONS"}).execute(conn).map_err(sql)?;Ok(())}).await.unwrap();
        let m = Mapping {
            id: uuid::Uuid::new_v4().to_string(),
            connection_id: c.id.clone(),
            external_id: "external".into(),
            account_id: local,
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
    let (_, mut m) = r.seed(PlaidProduct::Transactions).await;
    let account = m.account_id.clone();
    r.writer
        .exec(move |conn| {
            diesel::sql_query("UPDATE accounts SET account_type='CREDIT_CARD' WHERE id=?")
                .bind::<Text, _>(account)
                .execute(conn)
                .map_err(sql)?;
            Ok(())
        })
        .await
        .unwrap();
    m.credit_card = true;
    r.repo.put_mappings(vec![m.clone()]).await.unwrap();
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
    for field in ["account", "external", "connection"] {
        let mut altered = m.clone();
        match field {
            "account" => altered.account_id = "foreign".into(),
            "external" => altered.external_id = "foreign".into(),
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
        .commit_holdings(m.clone(), snapshot, vec![quote])
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
            assert_eq!(eligible.len(), 1);
            assert_eq!(eligible[0]["id"], "external");
            assert_eq!(eligible[0]["type"], "investment");
            assert_eq!(eligible[0]["currency"], "USD");
        } else {
            assert!(eligible.is_empty());
            assert_eq!(
                result,
                if accounts.as_array().unwrap().is_empty() {
                    json!({"accounts":[], "totalAccounts":0, "accountTypes":[]})
                } else {
                    json!({"accounts":[], "totalAccounts":1, "accountTypes":["depository"]})
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
