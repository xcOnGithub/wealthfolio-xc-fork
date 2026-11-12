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
