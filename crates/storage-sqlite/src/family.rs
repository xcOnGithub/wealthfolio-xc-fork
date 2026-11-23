//! Local-only family state; uses the existing writer, never the sync outbox.
use crate::{
    db::{get_connection, DbPool, WriteHandle},
    StorageError,
};
use async_trait::async_trait;
use diesel::{prelude::*, sql_types::Text};
use std::sync::Arc;
use wealthfolio_core::{Error, Result};
use wealthfolio_family::{FamilyRepository, FamilyState};

pub struct FamilyRepositorySqlite {
    pool: Arc<DbPool>,
    writer: WriteHandle,
}
impl FamilyRepositorySqlite {
    pub fn new(pool: Arc<DbPool>, writer: WriteHandle) -> Self {
        Self { pool, writer }
    }
}
#[derive(QueryableByName)]
struct Stored {
    #[diesel(sql_type=Text)]
    body: String,
}
#[async_trait]
impl FamilyRepository for FamilyRepositorySqlite {
    async fn load(&self) -> Result<FamilyState> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = get_connection(&pool)?;
            let row = diesel::sql_query("SELECT body FROM family_money_state WHERE id=1")
                .get_result::<Stored>(&mut conn)
                .optional()
                .map_err(StorageError::from)?;
            row.map(|r| {
                serde_json::from_str(&r.body)
                    .map_err(|_| Error::Repository("Family state cannot be read".into()))
            })
            .unwrap_or_else(|| Ok(FamilyState::default()))
        })
        .await
        .map_err(|_| Error::Repository("Family state read interrupted".into()))?
    }
    async fn save(&self, state: FamilyState) -> Result<()> {
        let body = serde_json::to_string(&state)
            .map_err(|_| Error::Repository("Family state cannot be encoded".into()))?;
        self.writer.exec_tx(move |tx| {
            diesel::sql_query("INSERT INTO family_money_state (id,body) VALUES (1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body")
                .bind::<Text,_>(body).execute(tx.conn()).map_err(StorageError::from)?;
            Ok(())
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{create_pool, run_migrations, write_actor::spawn_writer, TestDir};
    async fn setup() -> (FamilyRepositorySqlite, TestDir) {
        let dir = TestDir::new();
        let path = dir.path().join("family.db").to_string_lossy().to_string();
        run_migrations(&path).unwrap();
        let pool = create_pool(&path).unwrap();
        let writer = spawn_writer((*pool).clone()).unwrap();
        (FamilyRepositorySqlite::new(pool, writer), dir)
    }
    #[tokio::test]
    async fn state_round_trips_without_a_device_sync_outbox_and_is_profile_local() {
        let (a, _a) = setup().await;
        let (b, _b) = setup().await;
        let mut state = FamilyState {
            config_revision: 7,
            warnings: vec!["Synthetic source unavailable".into()],
            ..Default::default()
        };
        wealthfolio_family::engine::reconcile(
            &mut state,
            wealthfolio_family::Evaluation {
                candidates: vec![wealthfolio_family::Candidate {
                    key: "synthetic:one".into(),
                    source: "charges".into(),
                    kind: "large_charge".into(),
                    severity: "warning".into(),
                    title: "Synthetic charge".into(),
                    detail: "Synthetic only".into(),
                    href: "/family".into(),
                    amount: Some(1200.25),
                    currency: Some("USD".into()),
                    notify: true,
                }],
                complete_sources: vec!["charges".into()],
                warnings: vec![],
            },
            chrono::Utc::now(),
        );
        a.save(state).await.unwrap();
        assert_eq!(a.load().await.unwrap().items[0].fact.amount, Some(1200.25));
        assert_eq!(a.load().await.unwrap().config_revision, 7);
        assert_eq!(b.load().await.unwrap().config_revision, 0);
        let mut conn = get_connection(&a.pool).unwrap();
        let count: i64 = crate::schema::sync_outbox::table
            .count()
            .get_result(&mut conn)
            .unwrap();
        assert_eq!(count, 0);
    }
}
