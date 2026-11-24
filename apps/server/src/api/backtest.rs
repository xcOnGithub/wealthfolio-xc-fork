use std::sync::Arc;

use axum::{routing::post, Json, Router};
use wealthfolio_core::portfolio::backtest::{
    run_backtest as execute_backtest, BacktestRequest, BacktestResponse,
};

use crate::{error::ApiResult, main_lib::AppState};

async fn run_backtest(
    axum::Extension(state): axum::Extension<Arc<AppState>>,
    Json(request): Json<BacktestRequest>,
) -> ApiResult<Json<BacktestResponse>> {
    let response = execute_backtest(state.quote_service.clone(), request).await?;
    Ok(Json(response))
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/backtest/run", post(run_backtest))
}
