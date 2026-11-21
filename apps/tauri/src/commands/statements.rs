use crate::profiles::ProfileAccess;
use wealthfolio_core::portfolio::statements::{StatementValue, StatementWrite, StoredStatement};
#[tauri::command]
pub async fn list_statements(
    account_id: String,
    state: ProfileAccess,
) -> Result<Vec<StoredStatement>, String> {
    let c = state.context()?;
    c.statement_service
        .list(account_id)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn save_statements(
    account_id: String,
    writes: Vec<StatementWrite>,
    confirmed: bool,
    state: ProfileAccess,
) -> Result<(), String> {
    let c = state.context()?;
    c.statement_service
        .save(account_id, writes, confirmed)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn delete_statement(
    account_id: String,
    date: String,
    revision: String,
    state: ProfileAccess,
) -> Result<(), String> {
    let c = state.context()?;
    let date = date
        .parse()
        .map_err(|_| "Invalid statement date".to_string())?;
    c.statement_service
        .delete(account_id, date, revision)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn get_statement_values(
    account_id: String,
    start_date: String,
    end_date: String,
    state: ProfileAccess,
) -> Result<Vec<StatementValue>, String> {
    let c = state.context()?;
    let start = start_date
        .parse()
        .map_err(|_| "Invalid statement date".to_string())?;
    let end = end_date
        .parse()
        .map_err(|_| "Invalid statement date".to_string())?;
    c.statement_service
        .history(account_id, start, end)
        .await
        .map_err(Into::into)
}

use wealthfolio_core::portfolio::statements::daily::{
    DailyStatementPreview, DailyStatementWrite, StoredDailyStatement,
};
#[tauri::command]
pub async fn list_daily_statements(
    account_id: String,
    state: ProfileAccess,
) -> Result<Vec<StoredDailyStatement>, String> {
    state
        .context()?
        .statement_service
        .list_daily(account_id)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn preview_daily_statements(
    account_id: String,
    writes: Vec<DailyStatementWrite>,
    state: ProfileAccess,
) -> Result<Vec<DailyStatementPreview>, String> {
    state
        .context()?
        .statement_service
        .preview_daily(account_id, writes)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn save_daily_statements(
    account_id: String,
    writes: Vec<DailyStatementWrite>,
    confirmed: bool,
    state: ProfileAccess,
) -> Result<(), String> {
    state
        .context()?
        .statement_service
        .save_daily(account_id, writes, confirmed)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn refresh_statement_quotes(
    account_id: String,
    writes: Vec<DailyStatementWrite>,
    state: ProfileAccess,
) -> Result<usize, String> {
    state
        .context()?
        .statement_service
        .refresh_daily_quotes(account_id, writes)
        .await
        .map_err(Into::into)
}
