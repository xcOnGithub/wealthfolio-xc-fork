use crate::profiles::ProfileAccess;
use wealthfolio_core::tax::{TaxExportFormat, TaxPacketExport, TaxYearReport};

#[tauri::command]
pub async fn get_tax_year_report(
    tax_year: i32,
    member_name: Option<String>,
    state: ProfileAccess,
) -> Result<TaxYearReport, String> {
    let context = state.context()?;
    context
        .tax_service
        .get_tax_year_report(tax_year, member_name)
        .await
        .map_err(|e| format!("Failed to generate tax year report: {e}"))
}

#[tauri::command]
pub async fn export_tax_packet(
    tax_year: i32,
    format: TaxExportFormat,
    member_name: Option<String>,
    state: ProfileAccess,
) -> Result<TaxPacketExport, String> {
    let context = state.context()?;
    context
        .tax_service
        .export_tax_packet(tax_year, format, member_name)
        .await
        .map_err(|e| format!("Failed to export tax packet: {e}"))
}
