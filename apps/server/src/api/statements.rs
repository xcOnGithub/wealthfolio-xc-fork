use super::shared::parse_date;
use crate::{error::ApiResult, main_lib::AppState};
use axum::{
    extract::Query,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use wealthfolio_core::portfolio::statements::activity::StatementActivityPage;
use wealthfolio_core::portfolio::statements::daily::{
    DailyStatementPreview, DailyStatementWrite, StoredDailyStatement,
};
use wealthfolio_core::portfolio::statements::{StatementValue, StatementWrite, StoredStatement};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountQuery {
    account_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActivityQuery {
    account_id: String,
    offset: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Save {
    account_id: String,
    writes: Vec<StatementWrite>,
    confirmed: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Delete {
    account_id: String,
    date: String,
    revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    account_id: String,
    start_date: String,
    end_date: String,
}
async fn list(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Query(q): Query<AccountQuery>,
) -> ApiResult<Json<Vec<StoredStatement>>> {
    Ok(Json(s.statement_service.list(q.account_id).await?))
}
async fn save(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Json(q): Json<Save>,
) -> ApiResult<Json<()>> {
    s.statement_service
        .save(q.account_id, q.writes, q.confirmed)
        .await?;
    Ok(Json(()))
}
async fn delete(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Json(q): Json<Delete>,
) -> ApiResult<Json<()>> {
    s.statement_service
        .delete(q.account_id, parse_date(&q.date, "date")?, q.revision)
        .await?;
    Ok(Json(()))
}
async fn history(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Query(q): Query<History>,
) -> ApiResult<Json<Vec<StatementValue>>> {
    Ok(Json(
        s.statement_service
            .history(
                q.account_id,
                parse_date(&q.start_date, "startDate")?,
                parse_date(&q.end_date, "endDate")?,
            )
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DailySave {
    account_id: String,
    writes: Vec<DailyStatementWrite>,
    confirmed: bool,
}
async fn list_daily(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Query(q): Query<AccountQuery>,
) -> ApiResult<Json<Vec<StoredDailyStatement>>> {
    Ok(Json(s.statement_service.list_daily(q.account_id).await?))
}
async fn list_activity(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Query(q): Query<ActivityQuery>,
) -> ApiResult<Json<StatementActivityPage>> {
    Ok(Json(
        s.statement_service
            .list_activity(q.account_id, q.offset)
            .await?,
    ))
}
async fn preview_daily(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Json(q): Json<DailySave>,
) -> ApiResult<Json<Vec<DailyStatementPreview>>> {
    Ok(Json(
        s.statement_service
            .preview_daily(q.account_id, q.writes)
            .await?,
    ))
}
async fn save_daily(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Json(q): Json<DailySave>,
) -> ApiResult<Json<()>> {
    s.statement_service
        .save_daily(q.account_id, q.writes, q.confirmed)
        .await?;
    Ok(Json(()))
}
async fn refresh_daily(
    axum::Extension(s): axum::Extension<Arc<AppState>>,
    Json(q): Json<DailySave>,
) -> ApiResult<Json<usize>> {
    Ok(Json(
        s.statement_service
            .refresh_daily_quotes(q.account_id, q.writes)
            .await?,
    ))
}
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/statement-history/activities", get(list_activity))
        .route("/statement-history/daily", get(list_daily).post(save_daily))
        .route("/statement-history/daily/preview", post(preview_daily))
        .route("/statement-history/daily/quotes", post(refresh_daily))
        .route("/statement-history", get(list).post(save))
        .route("/statement-history/delete", post(delete))
        .route("/statement-history/values", get(history))
}
