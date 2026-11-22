//! Profile-local statement observations, independent of recalculated valuations.
use crate::{
    db::{get_connection, DbPool, WriteHandle},
    errors::StorageError,
};
use async_trait::async_trait;
use chrono::NaiveDate;
use diesel::{
    prelude::*,
    sql_query,
    sql_types::{Nullable, Text},
};
use std::{collections::BTreeMap, sync::Arc};
use wealthfolio_core::{
    errors::Result,
    portfolio::statements::{
        daily::{reviewed_chain, DailyStatementWrite, StoredDailyStatement},
        invalid, StatementRepository, StatementSnapshot, StatementWrite, StoredStatement,
    },
};

pub struct StatementStore {
    pool: Arc<DbPool>,
    writer: WriteHandle,
}
impl StatementStore {
    pub fn new(pool: Arc<DbPool>, writer: WriteHandle) -> Self {
        Self { pool, writer }
    }
}
#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type=Text)]
    payload: String,
    #[diesel(sql_type=Text)]
    revision: String,
}
fn read(conn: &mut SqliteConnection, account: &str) -> Result<Vec<StoredStatement>> {
    let rows = sql_query(
        "SELECT payload, revision FROM account_statement_history \
         WHERE account_id = ? ORDER BY statement_date",
    )
    .bind::<Text, _>(account)
    .load::<Row>(conn)
    .map_err(StorageError::from)?;
    rows.into_iter()
        .map(|row| {
            Ok(StoredStatement {
                snapshot: serde_json::from_str(&row.payload)?,
                revision: row.revision,
            })
        })
        .collect()
}

fn write_snapshot(
    conn: &mut SqliteConnection,
    account: &str,
    snapshot: &StatementSnapshot,
) -> Result<()> {
    sql_query(
        "INSERT INTO account_statement_history(account_id,statement_date,payload,revision)
         VALUES (?,?,?,?) ON CONFLICT(account_id,statement_date) DO UPDATE
         SET payload=excluded.payload, revision=excluded.revision",
    )
    .bind::<Text, _>(account)
    .bind::<Text, _>(snapshot.date.to_string())
    .bind::<Text, _>(serde_json::to_string(snapshot)?)
    .bind::<Text, _>(uuid::Uuid::new_v4().to_string())
    .execute(conn)
    .map_err(StorageError::from)?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DailyPayload {
    #[serde(default)]
    closing_asset_ids: Vec<String>,
    version: u32,
    interval: wealthfolio_core::portfolio::statements::reconstruction::StatementInterval,
}

// Store matching metadata separately so an older app can still read daily facts.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SecurityReview {
    revision: String,
    activity_security_keys: BTreeMap<String, String>,
    review_order: u64,
}

#[derive(QueryableByName)]
struct DailyRow {
    #[diesel(embed)]
    facts: Row,
    #[diesel(sql_type = Nullable<Text>)]
    security_review: Option<String>,
}

fn read_daily(conn: &mut SqliteConnection, account: &str) -> Result<Vec<StoredDailyStatement>> {
    let rows = sql_query("SELECT payload, revision, security_review FROM account_statement_reconstructions WHERE account_id=? ORDER BY through_date")
        .bind::<Text,_>(account).load::<DailyRow>(conn).map_err(StorageError::from)?;
    rows.into_iter()
        .map(|row| {
            let facts: DailyPayload = serde_json::from_str(&row.facts.payload)?;
            if facts.version != 1 {
                return Err(invalid());
            }
            let matches = row
                .security_review
                .as_deref()
                .map(serde_json::from_str::<SecurityReview>)
                .transpose()?
                // An older app may replace the facts and revision without
                // writing match metadata. Discard evidence tied to the old facts.
                .filter(|review| review.revision == row.facts.revision)
                .unwrap_or_default();
            Ok(StoredDailyStatement {
                version: facts.version,
                closing_asset_ids: facts.closing_asset_ids,
                activity_security_keys: matches.activity_security_keys,
                review_order: matches.review_order,
                priced: None,
                live_handoff: Default::default(),
                interval: facts.interval,
                revision: row.facts.revision,
            })
        })
        .collect()
}

#[derive(QueryableByName)]
struct StatementYearCountRow {
    #[diesel(sql_type = Text)]
    account_id: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    statement_count: i64,
}

#[async_trait]
impl StatementRepository for StatementStore {
    fn list(&self, account: &str) -> Result<Vec<StoredStatement>> {
        let mut conn = get_connection(&self.pool)?;
        read(&mut conn, account)
    }

    fn count_by_year(
        &self,
        account_ids: &[String],
        year: i32,
    ) -> Result<std::collections::HashMap<String, usize>> {
        if account_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let mut conn = get_connection(&self.pool)?;
        let start = format!("{year:04}-01-01");
        let end = format!("{year:04}-12-31");
        let mut counts = std::collections::HashMap::new();
        for chunk in crate::utils::chunk_for_sqlite(account_ids) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT account_id, COUNT(*) AS statement_count \
                 FROM account_statement_history \
                 WHERE account_id IN ({placeholders}) \
                   AND statement_date >= ? AND statement_date <= ? \
                 GROUP BY account_id"
            );
            let mut bound = sql_query(query).into_boxed();
            for account_id in chunk {
                bound = bound.bind::<Text, _>(account_id);
            }
            let rows = bound
                .bind::<Text, _>(&start)
                .bind::<Text, _>(&end)
                .load::<StatementYearCountRow>(&mut conn)
                .map_err(StorageError::from)?;
            for row in rows {
                if row.statement_count > 0 {
                    counts.insert(row.account_id, row.statement_count as usize);
                }
            }
        }
        Ok(counts)
    }

    fn list_daily(&self, account: &str) -> Result<Vec<StoredDailyStatement>> {
        let mut conn = get_connection(&self.pool)?;
        read_daily(&mut conn, account)
    }
    async fn save_daily(&self, account: &str, writes: Vec<DailyStatementWrite>) -> Result<()> {
        if writes.is_empty()
            || writes.len() > wealthfolio_core::portfolio::statements::MAX_STATEMENT_BATCH
        {
            return Err(invalid());
        }
        let account = account.to_owned();
        self.writer.exec_tx(move |tx| {
            let conn = tx.conn();
            #[derive(QueryableByName)]
            struct Scope {
                #[diesel(sql_type=Text)] account_type: String,
                #[diesel(sql_type=Text)] currency: String,
            }
            let scope = sql_query("SELECT account_type,currency FROM accounts WHERE id=?")
                .bind::<Text,_>(&account).get_result::<Scope>(conn).map_err(StorageError::from)?;
            if scope.account_type != wealthfolio_core::accounts::account_types::SECURITIES { return Err(invalid()); }
            let mut dates = std::collections::BTreeSet::new();
            for write in &writes {
                // Structural/account checks belong at persistence too. The service
                // owns user-timezone future-date admission, not SQLite's clock.
                write.validate(&scope.currency, NaiveDate::MAX)?;
                if !dates.insert(write.interval.through) { return Err(invalid()); }
            }
            let existing = read_daily(conn, &account)?;
            reviewed_chain(&existing, &writes)?;
            let snapshots = read(conn, &account)?;
            let review_order = existing.iter().map(|s| s.review_order).max().unwrap_or(0)
                .checked_add(1).ok_or_else(invalid)?;
            for write in writes {
                let previous = existing.iter().find(|s| s.interval.through == write.interval.through);
                let snapshot = snapshots.iter().find(|s|s.snapshot.date == write.snapshot.date);
                let same_interval = previous.is_some_and(|s| write.matches_daily(s));
                let same_snapshot = snapshot.is_some_and(|s|s.snapshot == write.snapshot);
                if same_interval && same_snapshot { continue; }
                if previous.map(|s| &s.revision) != write.expected_revision.as_ref()
                    || snapshot.map(|s| &s.revision) != write.expected_snapshot_revision.as_ref() { return Err(invalid()); }
                // Monthly review and delete also protect the attached daily facts.
                write_snapshot(conn, &account, &write.snapshot)?;
                let payload = DailyPayload {
                    version: 1,
                    closing_asset_ids: write.closing_asset_ids,
                    interval: write.interval,
                };
                let matches = SecurityReview {
                    revision: uuid::Uuid::new_v4().to_string(),
                    activity_security_keys: write.activity_security_keys.unwrap_or_default(),
                    review_order,
                };
                sql_query("INSERT INTO account_statement_reconstructions(account_id,through_date,payload,revision,security_review) VALUES (?,?,?,?,?) ON CONFLICT(account_id,through_date) DO UPDATE SET payload=excluded.payload,revision=excluded.revision,security_review=excluded.security_review")
                    .bind::<Text,_>(&account).bind::<Text,_>(payload.interval.through.to_string())
                    .bind::<Text,_>(serde_json::to_string(&payload)?).bind::<Text,_>(&matches.revision)
                    .bind::<Text,_>(serde_json::to_string(&matches)?)
                    .execute(conn).map_err(StorageError::from)?;
            }
            Ok(())
        }).await
    }
    async fn save(&self, account: &str, writes: Vec<StatementWrite>) -> Result<()> {
        let account = account.to_owned();
        self.writer
            .exec_tx(move |tx| {
                let conn = tx.conn();
                let existing = read(conn, &account)?;
                for write in writes {
                    let old = existing
                        .iter()
                        .find(|s| s.snapshot.date == write.snapshot.date);
                    if old.is_some_and(|s| s.snapshot == write.snapshot) {
                        continue;
                    }
                    if old.map(|s| &s.revision) != write.expected_revision.as_ref() {
                        return Err(invalid());
                    }
                    write_snapshot(conn, &account, &write.snapshot)?;
                }
                Ok(())
            })
            .await
    }

    async fn delete(&self, account: &str, date: NaiveDate, revision: &str) -> Result<()> {
        let (account, revision) = (account.to_owned(), revision.to_owned());
        self.writer
            .exec_tx(move |tx| {
                let count = sql_query(
                    "DELETE FROM account_statement_history \
                 WHERE account_id=? AND statement_date=? AND revision=?",
                )
                .bind::<Text, _>(account)
                .bind::<Text, _>(date.to_string())
                .bind::<Text, _>(revision)
                .execute(tx.conn())
                .map_err(StorageError::from)?;
                if count != 1 {
                    return Err(invalid());
                }
                Ok(())
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::RwLock;
    use wealthfolio_core::portfolio::{
        statements::{StatementService, StatementSnapshot},
        valuation::ValuationRepositoryTrait,
    };
    struct Rig {
        _dir: tempfile::TempDir,
        pool: Arc<DbPool>,
        writer: WriteHandle,
        store: Arc<StatementStore>,
        service: StatementService,
    }
    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db =
                crate::db::DbAccess::plaintext(dir.path().join("statements.db").to_string_lossy());
            db.prepare().unwrap();
            db.run_migrations().unwrap();
            let pool = db.create_pool().unwrap();
            let writer = crate::db::write_actor::spawn_writer(pool.as_ref().clone()).unwrap();
            sql_query("INSERT INTO accounts(id,name,account_type,currency,is_default,is_active,tracking_mode,is_archived) VALUES ('a','Synthetic','SECURITIES','USD',0,1,'TRANSACTIONS',0),('b','Other','SECURITIES','USD',0,1,'HOLDINGS',0)").execute(&mut pool.get().unwrap()).unwrap();
            let store = Arc::new(StatementStore::new(pool.clone(), writer.clone()));
            let service = StatementService::new(
                store.clone(),
                Arc::new(crate::accounts::AccountRepository::new(
                    pool.clone(),
                    writer.clone(),
                )),
                Arc::new(crate::portfolio::valuation::ValuationRepository::new(
                    pool.clone(),
                    writer.clone(),
                )),
                Arc::new(RwLock::new("UTC".into())),
            );
            Self {
                _dir: dir,
                pool,
                writer,
                store,
                service,
            }
        }
    }
    fn write(date: &str, total: &str) -> StatementWrite {
        StatementWrite {
            snapshot: StatementSnapshot {
                date: date.parse().unwrap(),
                currency: "USD".into(),
                total_value: total.into(),
                cash_value: None,
                positions: vec![],
                positions_complete: false,
                format: "merrill-cma-monthly-v1".into(),
                source_page: 1,
            },
            expected_revision: None,
        }
    }
    #[tokio::test]
    async fn statement_history_review_is_required_and_account_scope_is_enforced() {
        let r = Rig::new();
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100")], false)
            .await
            .is_err());
        assert!(r
            .service
            .save("missing".into(), vec![write("2025-01-31", "100")], true)
            .await
            .is_err());
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100"); 6], true)
            .await
            .is_err());
        assert!(r
            .service
            .save("a".into(), vec![write("2025-01-31", "100"); 2], true)
            .await
            .is_err());
        r.service
            .save("a".into(), vec![write("2025-01-31", "100")], true)
            .await
            .unwrap();
        assert!(r.service.list("b".into()).await.unwrap().is_empty());
        let saved = r.store.list("a").unwrap();
        assert!(r
            .service
            .delete(
                "b".into(),
                saved[0].snapshot.date,
                saved[0].revision.clone()
            )
            .await
            .is_err());
        let other = Rig::new();
        assert!(other.store.list("a").unwrap().is_empty());
    }
    #[tokio::test]
    async fn statement_history_batch_conflicts_roll_back_and_replacements_compare_revision() {
        let r = Rig::new();
        let original = write("2025-01-31", "100");
        r.service
            .save("a".into(), vec![original.clone()], true)
            .await
            .unwrap();
        let saved = r.store.list("a").unwrap();
        r.service
            .save("a".into(), vec![original], true)
            .await
            .unwrap();
        assert_eq!(r.store.list("a").unwrap()[0].revision, saved[0].revision);
        assert!(r
            .service
            .save(
                "a".into(),
                vec![write("2025-02-28", "110"), write("2025-01-31", "90")],
                true
            )
            .await
            .is_err());
        assert_eq!(r.store.list("a").unwrap().len(), 1);
        let mut updated = write("2025-01-31", "90");
        updated.expected_revision = Some(saved[0].revision.clone());
        r.service
            .save("a".into(), vec![updated.clone()], true)
            .await
            .unwrap();
        let current = r.store.list("a").unwrap();
        assert_eq!(current[0].snapshot.total_value, "90");
        assert!(r
            .service
            .delete(
                "a".into(),
                saved[0].snapshot.date,
                saved[0].revision.clone()
            )
            .await
            .is_err());
        updated.snapshot.total_value = "80".into();
        assert!(r
            .service
            .save("a".into(), vec![updated], true)
            .await
            .is_err());
        r.service
            .delete(
                "a".into(),
                current[0].snapshot.date,
                current[0].revision.clone(),
            )
            .await
            .unwrap();
        assert!(r.store.list("a").unwrap().is_empty());
    }
    #[tokio::test]
    async fn statement_history_survives_projection_replacement_and_account_deletion_cascades() {
        let r = Rig::new();
        r.service
            .save(
                "a".into(),
                vec![write("2025-01-31", "100"), write("2025-02-28", "110")],
                true,
            )
            .await
            .unwrap();
        let valuations =
            crate::portfolio::valuation::ValuationRepository::new(r.pool.clone(), r.writer.clone());
        valuations
            .replace_valuations_for_account("a", None, &[])
            .await
            .unwrap();
        let points = r
            .service
            .history(
                "a".into(),
                "2025-01-01".parse().unwrap(),
                "2025-03-01".parse().unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(points.first().unwrap().value, "100");
        assert_eq!(points.last().unwrap().value, "110");
        sql_query("UPDATE accounts SET tracking_mode='HOLDINGS' WHERE id='a'")
            .execute(&mut r.pool.get().unwrap())
            .unwrap();
        assert_eq!(r.store.list("a").unwrap().len(), 2);
        r.writer
            .exec(|conn| {
                sql_query("DELETE FROM accounts WHERE id='a'")
                    .execute(conn)
                    .map_err(StorageError::from)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(r.store.list("a").unwrap().is_empty());
    }
    mod daily_tests;

    #[tokio::test]
    async fn statement_count_by_year_groups_within_calendar_year() {
        let r = Rig::new();
        r.service
            .save(
                "a".into(),
                vec![
                    write("2024-12-31", "95"),
                    write("2025-01-31", "100"),
                    write("2025-06-30", "110"),
                ],
                true,
            )
            .await
            .unwrap();
        let counts_2025 = r.store.count_by_year(&["a".into()], 2025).unwrap();
        assert_eq!(counts_2025.get("a"), Some(&2));
        let counts_2024 = r.store.count_by_year(&["a".into()], 2024).unwrap();
        assert_eq!(counts_2024.get("a"), Some(&1));
        let counts_2026 = r.store.count_by_year(&["a".into()], 2026).unwrap();
        assert!(counts_2026.is_empty());
    }

    #[tokio::test]
    async fn statement_history_accepts_multi_month_bundle_batches_up_to_limit() {
        let r = Rig::new();
        let mut writes: Vec<_> = (1..=12)
            .map(|month| {
                let mut w = write(&format!("2024-{month:02}-28"), "100");
                w.snapshot.source_page = (month as u32) * 15;
                w
            })
            .collect();
        r.service
            .save("a".into(), writes.clone(), true)
            .await
            .unwrap();
        assert_eq!(r.store.list("a").unwrap().len(), 12);
        let too_many: Vec<_> = (0..=wealthfolio_core::portfolio::statements::MAX_STATEMENT_BATCH)
            .map(|i| {
                let date = NaiveDate::from_ymd_opt(2020, 1, 1)
                    .unwrap()
                    .checked_add_days(chrono::Days::new(i as u64))
                    .unwrap();
                write(&date.to_string(), "100")
            })
            .collect();
        assert!(r.service.save("a".into(), too_many, true).await.is_err());
        writes[0].snapshot.source_page =
            wealthfolio_core::portfolio::statements::MAX_STATEMENT_SOURCE_PAGE + 1;
        assert!(r
            .service
            .save("a".into(), vec![writes[0].clone()], true)
            .await
            .is_err());
    }
}
