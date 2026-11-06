//! Direct Plaid metadata/staging is profile-local and never produces an outbox.
use crate::{
    db::{write_actor::DbWriteTx, WriteHandle},
    Error,
};
use async_trait::async_trait;
use diesel::{prelude::*, sql_types::Text};
use serde::{de::DeserializeOwned, Serialize};
use wealthfolio_connect::plaid::state::Connection as PlaidConnection;
use wealthfolio_connect::plaid::{state::*, PlaidProduct, DIRECT_PLAID_PROVIDER};
use wealthfolio_core::portfolio::snapshot::AccountStateSnapshot;

#[derive(QueryableByName)]
struct Body {
    #[diesel(sql_type = Text)]
    body: String,
}

#[cfg(test)]
#[path = "plaid_tests.rs"]
mod tests;

pub struct PlaidRepositorySqlite {
    writer: WriteHandle,
}
impl PlaidRepositorySqlite {
    pub fn new(writer: WriteHandle) -> Self {
        Self { writer }
    }
}
fn changed() -> Error {
    Error::ConstraintViolation("PLAID_CHANGED".into())
}
fn review() -> Error {
    Error::ConstraintViolation("PLAID_REVIEW".into())
}
fn storage(error: Error) -> SyncError {
    match error {
        Error::ConstraintViolation(s) if s == "PLAID_CHANGED" => SyncError::Changed,
        Error::ConstraintViolation(s) if s == "PLAID_REVIEW" => SyncError::Review,
        _ => SyncError::Storage,
    }
}
fn sql(error: diesel::result::Error) -> Error {
    crate::StorageError::from(error).into()
}
fn encode<T: Serialize>(value: &T) -> crate::Result<String> {
    serde_json::to_string(value)
        .map_err(|_| Error::Repository("Plaid metadata encoding failed".into()))
}
fn decode<T: DeserializeOwned>(body: Body) -> crate::Result<T> {
    serde_json::from_str(&body.body)
        .map_err(|_| Error::Repository("Plaid metadata decoding failed".into()))
}
fn rows<T: DeserializeOwned>(conn: &mut SqliteConnection, table: &str) -> crate::Result<Vec<T>> {
    diesel::sql_query(format!("SELECT body FROM {table}"))
        .load::<Body>(conn)
        .map_err(sql)?
        .into_iter()
        .map(decode)
        .collect()
}
fn mapping(conn: &mut SqliteConnection, id: &str) -> crate::Result<Mapping> {
    decode(
        diesel::sql_query("SELECT body FROM plaid_accounts WHERE id=?")
            .bind::<Text, _>(id)
            .get_result::<Body>(conn)
            .map_err(sql)?,
    )
}
fn connection(conn: &mut SqliteConnection, id: &str) -> crate::Result<PlaidConnection> {
    decode(
        diesel::sql_query("SELECT body FROM plaid_connections WHERE id=?")
            .bind::<Text, _>(id)
            .get_result::<Body>(conn)
            .map_err(sql)?,
    )
}
fn put_mapping(tx: &mut DbWriteTx<'_>, m: &Mapping) -> crate::Result<()> {
    let old = mapping(tx.conn(), &m.id)?;
    if old.account_id != m.account_id
        || old.external_id != m.external_id
        || old.connection_id != m.connection_id
    {
        return Err(changed());
    }
    diesel::sql_query("UPDATE plaid_accounts SET body=? WHERE id=?")
        .bind::<Text, _>(encode(m)?)
        .bind::<Text, _>(&m.id)
        .execute(tx.conn())
        .map_err(sql)?;
    Ok(())
}
fn ensure_connected(tx: &mut DbWriteTx<'_>, m: &Mapping) -> crate::Result<()> {
    let c = connection(tx.conn(), &m.connection_id)?;
    if c.status != "connected" {
        return Err(changed());
    }
    use crate::schema::accounts::dsl as a;
    let (provider, external, archived, mode, kind, currency) = a::accounts
        .find(&m.account_id)
        .select((
            a::provider,
            a::provider_account_id,
            a::is_archived,
            a::tracking_mode,
            a::account_type,
            a::currency,
        ))
        .first::<(Option<String>, Option<String>, bool, String, String, String)>(tx.conn())
        .map_err(sql)?;
    let expected_mode = if c.product == PlaidProduct::Investments {
        "HOLDINGS"
    } else {
        "TRANSACTIONS"
    };
    let expected_kind = if c.product == PlaidProduct::Investments {
        "SECURITIES"
    } else if m.credit_card {
        "CREDIT_CARD"
    } else {
        "CASH"
    };
    if provider.as_deref() != Some(DIRECT_PLAID_PROVIDER)
        || external.as_deref() != Some(&m.id)
        || archived
        || mode != expected_mode
        || kind != expected_kind
        || currency != m.currency
    {
        return Err(review());
    }

    Ok(())
}

#[async_trait]
impl PlaidRepository for PlaidRepositorySqlite {
    async fn connections(&self) -> Result<Vec<PlaidConnection>> {
        self.writer
            .exec(|conn| rows(conn, "plaid_connections"))
            .await
            .map_err(storage)
    }
    async fn mappings(&self) -> Result<Vec<Mapping>> {
        self.writer
            .exec(|conn| rows(conn, "plaid_accounts"))
            .await
            .map_err(storage)
    }
    async fn put_connection(&self, c: PlaidConnection, expected: Option<String>) -> Result<()> {
        self.writer.exec_tx(move |tx| {
            let env = serde_json::to_value(c.environment).map_err(|_| changed())?.as_str().ok_or_else(changed)?.to_string();
            if let Some(expected) = expected {
                let old = connection(tx.conn(), &c.id)?;
                if old.environment != c.environment || (old.owner != c.owner && !(c.status == "linking" && matches!(old.status.as_str(),"connected" | "attention"))) || old.institution != c.institution { return Err(changed()); }
                let n = diesel::sql_query("UPDATE plaid_connections SET item_id=?, status=?, body=? WHERE id=? AND status=?")
                    .bind::<diesel::sql_types::Nullable<Text>,_>(&c.item_id).bind::<Text,_>(&c.status).bind::<Text,_>(encode(&c)?)
                    .bind::<Text,_>(&c.id).bind::<Text,_>(expected).execute(tx.conn()).map_err(sql)?;
                if n != 1 { return Err(changed()); }
            } else {
                diesel::sql_query("INSERT INTO plaid_connections(id,environment,item_id,status,body) VALUES(?,?,?,?,?)")
                    .bind::<Text,_>(&c.id).bind::<Text,_>(env).bind::<diesel::sql_types::Nullable<Text>,_>(&c.item_id)
                    .bind::<Text,_>(&c.status).bind::<Text,_>(encode(&c)?).execute(tx.conn()).map_err(sql)?;
            }
            Ok(())
        }).await.map_err(storage)
    }
    async fn put_mappings(&self, mappings: Vec<Mapping>) -> Result<()> {
        self.writer
            .exec_tx(move |tx| {
                for m in mappings {
                    put_mapping(tx, &m)?;
                }
                Ok(())
            })
            .await
            .map_err(storage)
    }
    async fn add_mapping(&self, m: Mapping, product: PlaidProduct) -> Result<()> {
        self.writer.exec_tx(move |tx| {
            let c = connection(tx.conn(), &m.connection_id)?;
            if c.status != "connected" || c.product != product { return Err(changed()); }
            use crate::schema::{accounts::dsl as a, activities::dsl as act, holdings_snapshots::dsl as hs};
            let (kind, currency, provider, mode, archived) = a::accounts.find(&m.account_id)
                .select((a::account_type,a::currency,a::provider,a::tracking_mode,a::is_archived))
                .first::<(String,String,Option<String>,String,bool)>(tx.conn()).map_err(sql)?;
            let expected = if product == PlaidProduct::Investments { "HOLDINGS" } else { "TRANSACTIONS" };
            if archived || provider.is_some() || currency != m.currency || !mode.eq_ignore_ascii_case(expected)
                || (m.credit_card && kind != wealthfolio_core::accounts::account_types::CREDIT_CARD)
                || (product == PlaidProduct::Investments && kind != wealthfolio_core::accounts::account_types::SECURITIES)
                || (!m.credit_card && product == PlaidProduct::Transactions && kind != wealthfolio_core::accounts::account_types::CASH)
            { return Err(review()); }
            let count = act::activities.filter(act::account_id.eq(&m.account_id)).count().get_result::<i64>(tx.conn()).map_err(sql)?;
            if let Some(cutover) = m.cutover_date {
                if product != PlaidProduct::Transactions || count == 0 || act::activities.filter(act::account_id.eq(&m.account_id))
                    .filter(act::activity_date.ge(cutover.to_string())).count().get_result::<i64>(tx.conn()).map_err(sql)? != 0 { return Err(review()); }
            } else if count != 0 || hs::holdings_snapshots.filter(hs::account_id.eq(&m.account_id)).count().get_result::<i64>(tx.conn()).map_err(sql)? != 0 { return Err(review()); }
            diesel::sql_query("INSERT INTO plaid_accounts(id,connection_id,external_id,account_id,body) VALUES(?,?,?,?,?)")
                .bind::<Text,_>(&m.id).bind::<Text,_>(&m.connection_id).bind::<Text,_>(&m.external_id)
                .bind::<Text,_>(&m.account_id).bind::<Text,_>(encode(&m)?).execute(tx.conn()).map_err(sql)?;
            diesel::update(a::accounts.find(&m.account_id)).set((a::provider.eq(DIRECT_PLAID_PROVIDER),a::provider_account_id.eq(&m.id)))
                .execute(tx.conn()).map_err(sql)?;
            // Broker-bound accounts do not replicate through device sync.
            Ok(())
        }).await.map_err(storage)
    }
    async fn commit_banks(&self, batches: Vec<BankCommit>) -> Result<()> {
        let sync = self.writer.sync_state();
        self.writer
            .exec_tx(move |tx| {
                use crate::schema::activities::dsl as a;
                let mut groups = std::collections::HashSet::new();
                let mut incoming_groups = std::collections::HashSet::new();
                for batch in batches {
                    let m = &batch.mapping;
                    ensure_connected(tx, m)?;
                    if mapping(tx.conn(), &m.id)?.cursor != batch.expected_cursor {
                        return Err(changed());
                    }
                    if batch.expected_cursor.is_none()
                        && m.cutover_date.is_none()
                        && a::activities
                            .filter(a::account_id.eq(&m.account_id))
                            .count()
                            .get_result::<i64>(tx.conn())
                            .map_err(sql)?
                            != 0
                    {
                        return Err(review());
                    }
                    if batch.expected_cursor.is_none() {
                        if let Some(cutover) = m.cutover_date {
                            let overlap = a::activities
                                .filter(a::account_id.eq(&m.account_id))
                                .filter(a::activity_date.ge(cutover.to_string()))
                                .count()
                                .get_result::<i64>(tx.conn())
                                .map_err(sql)?;
                            if overlap != 0 {
                                return Err(review());
                            }
                        }
                    }
                    for removed in &batch.removed_ids {
                        let existing = a::activities
                            .filter(a::account_id.eq(&m.account_id))
                            .filter(a::source_system.eq("PLAID"))
                            .filter(a::source_record_id.eq(removed))
                            .select(crate::activities::ActivityDB::as_select())
                            .load::<crate::activities::ActivityDB>(tx.conn())
                            .map_err(sql)?;
                        for row in existing {
                            if row.is_user_modified != 0 || row.source_group_id.is_some() {
                                return Err(review());
                            }
                            diesel::delete(a::activities.find(&row.id))
                                .execute(tx.conn())
                                .map_err(sql)?;
                        }
                    }
                    // Provider corrections must preserve or consistently replace old groups.
                    // Validate pre-images as well as incoming group IDs in this transaction.
                    for activity in &batch.activities {
                        let old = a::activities
                            .find(&activity.id)
                            .select((a::source_group_id, a::is_user_modified))
                            .first::<(Option<String>, i32)>(tx.conn())
                            .optional()
                            .map_err(sql)?;
                        if let Some((Some(group), _)) = &old {
                            groups.insert(group.clone());
                        }
                        // The existing importer preserves owner edits; skipped provider
                        // decisions must not re-impose a group the owner removed.
                        if old.is_none_or(|(_, modified)| modified == 0) {
                            if let Some(group) = &activity.source_group_id {
                                groups.insert(group.clone());
                                incoming_groups.insert(group.clone());
                            }
                        }
                    }
                    let run_id = uuid::Uuid::new_v4().to_string();
                    let mut run = wealthfolio_connect::broker_ingest::ImportRun::new(
                        m.account_id.clone(),
                        "PLAID".into(),
                        wealthfolio_connect::broker_ingest::ImportRunType::Sync,
                        if batch.expected_cursor.is_none() {
                            wealthfolio_connect::broker_ingest::ImportRunMode::Initial
                        } else {
                            wealthfolio_connect::broker_ingest::ImportRunMode::Incremental
                        },
                        wealthfolio_connect::broker_ingest::ReviewMode::Never,
                    );
                    run.id = run_id.clone();
                    run.status = wealthfolio_connect::broker_ingest::ImportRunStatus::Applied;
                    run.finished_at = Some(chrono::Utc::now());
                    run.applied_at = run.finished_at;
                    run.checkpoint_in = batch
                        .expected_cursor
                        .map(|cursor| serde_json::json!({"cursor":cursor}));
                    run.checkpoint_out = Some(serde_json::json!({"cursor":m.cursor}));
                    let run_db = crate::sync::ImportRunDB::from(run);
                    diesel::insert_into(crate::schema::import_runs::table)
                        .values(&run_db)
                        .execute(tx.conn())
                        .map_err(sql)?;
                    let mut activities = batch.activities;
                    for activity in &mut activities {
                        activity.import_run_id = Some(run_id.clone());
                    }
                    crate::activities::apply_activities_tx(tx, activities, &sync)?;
                    put_mapping(tx, m)?;
                }
                // An explicitly paired internal transfer must have both legs in the ledger.
                for group in groups {
                    let rows = a::activities
                        .filter(a::source_group_id.eq(&group))
                        .select(crate::activities::ActivityDB::as_select())
                        .load::<crate::activities::ActivityDB>(tx.conn())
                        .map_err(sql)?;
                    if rows.is_empty() && !incoming_groups.contains(&group) {
                        continue;
                    }
                    let amounts: Vec<_> = rows
                        .iter()
                        .filter_map(|row| {
                            row.amount
                                .as_deref()
                                .and_then(|v| v.parse::<rust_decimal::Decimal>().ok())
                        })
                        .collect();
                    if amounts.len() != 2
                        || amounts.iter().any(|v| *v <= rust_decimal::Decimal::ZERO)
                    {
                        return Err(review());
                    }
                    if rows.len() != 2
                        || rows[0].status != "POSTED"
                        || rows[1].status != "POSTED"
                        || rows[0].account_id == rows[1].account_id
                        || rows[0].currency != rows[1].currency
                        || rows[0]
                            .amount
                            .as_deref()
                            .and_then(|v| v.parse::<rust_decimal::Decimal>().ok())
                            != rows[1]
                                .amount
                                .as_deref()
                                .and_then(|v| v.parse::<rust_decimal::Decimal>().ok())
                        || !((rows[0].activity_type == "TRANSFER_IN"
                            && rows[1].activity_type == "TRANSFER_OUT")
                            || (rows[0].activity_type == "TRANSFER_OUT"
                                && rows[1].activity_type == "TRANSFER_IN"))
                    {
                        return Err(review());
                    }
                }
                Ok(())
            })
            .await
            .map_err(storage)
    }
    async fn commit_holdings(
        &self,
        m: Mapping,
        snapshot: AccountStateSnapshot,
        quotes: Vec<wealthfolio_core::quotes::Quote>,
    ) -> Result<()> {
        self.writer
            .exec_tx(move |tx| {
                ensure_connected(tx, &m)?;
                use crate::schema::holdings_snapshots::dsl as hs;
                let source = hs::holdings_snapshots
                    .find(&snapshot.id)
                    .select(hs::source)
                    .first::<String>(tx.conn())
                    .optional()
                    .map_err(sql)?;
                if source.is_some_and(|s| s != "BROKER_IMPORTED") {
                    return Err(review());
                }
                crate::portfolio::snapshot::SnapshotRepository::save_snapshot_tx(tx, &snapshot)?;
                for quote in quotes {
                    let row = crate::market_data::QuoteDB::from(&quote);
                    diesel::replace_into(crate::schema::quotes::table)
                        .values(row)
                        .execute(tx.conn())
                        .map_err(sql)?;
                }
                put_mapping(tx, &m)?;
                Ok(())
            })
            .await
            .map_err(storage)
    }
}
