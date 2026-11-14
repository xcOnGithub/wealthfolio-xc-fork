//! Profile-local statement observations, independent of recalculated valuations.
use crate::{
    db::{get_connection, DbPool, WriteHandle},
    errors::StorageError,
};
use async_trait::async_trait;
use chrono::NaiveDate;
use diesel::{prelude::*, sql_query, sql_types::Text};
use std::sync::Arc;
use wealthfolio_core::{
    errors::Result,
    portfolio::statements::{invalid, StatementRepository, StatementWrite, StoredStatement},
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

#[async_trait]
impl StatementRepository for StatementStore {
    fn list(&self, account: &str) -> Result<Vec<StoredStatement>> {
        let mut conn = get_connection(&self.pool)?;
        read(&mut conn, account)
    }

    async fn save(&self, account: &str, writes: Vec<StatementWrite>) -> Result<()> {
        let account = account.to_owned();
        self.writer.exec_tx(move |tx| {
            let conn = tx.conn();
            let existing = read(conn, &account)?;
            for write in writes {
                let old = existing.iter().find(|s| s.snapshot.date == write.snapshot.date);
                if old.is_some_and(|s| s.snapshot == write.snapshot) {
                    continue;
                }
                if old.map(|s| &s.revision) != write.expected_revision.as_ref() {
                    return Err(invalid());
                }
                let payload = serde_json::to_string(&write.snapshot)
                    .map_err(wealthfolio_core::errors::Error::from)?;
                sql_query(
                    "INSERT INTO account_statement_history(account_id,statement_date,payload,revision) \
                     VALUES (?,?,?,?) ON CONFLICT(account_id,statement_date) DO UPDATE \
                     SET payload=excluded.payload, revision=excluded.revision",
                )
                .bind::<Text, _>(&account)
                .bind::<Text, _>(write.snapshot.date.to_string())
                .bind::<Text, _>(payload)
                .bind::<Text, _>(uuid::Uuid::new_v4().to_string())
                .execute(conn)
                .map_err(StorageError::from)?;
            }
            Ok(())
        }).await
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
}
