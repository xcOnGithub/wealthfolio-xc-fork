use super::shared::parse_date;
use crate::{error::ApiResult, main_lib::AppState};
use axum::{
    extract::Query,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use wealthfolio_core::portfolio::statements::{StatementValue, StatementWrite, StoredStatement};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountQuery {
    account_id: String,
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
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/statement-history", get(list).post(save))
        .route("/statement-history/delete", post(delete))
        .route("/statement-history/values", get(history))
}
