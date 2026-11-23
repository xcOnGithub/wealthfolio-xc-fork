use crate::profiles::ProfileAccess;
use wealthfolio_core::portfolio::backtest::{
    run_backtest as execute_backtest, BacktestRequest, BacktestResponse,
};

#[tauri::command]
pub async fn run_backtest(
    request: BacktestRequest,
    state: ProfileAccess,
) -> Result<BacktestResponse, String> {
    let context = state.context()?;
    let quote_service = context.quote_service();
    execute_backtest(quote_service, request)
        .await
        .map_err(|e| format!("{e}"))
}
