use std::sync::Arc;

use axum::{extract::Query, routing::get, Json, Router};
use serde::Deserialize;
use wealthfolio_core::tax::{TaxExportFormat, TaxPacketExport, TaxYearReport};

use crate::{error::ApiResult, main_lib::AppState};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaxReportQuery {
    year: i32,
    member: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaxExportQuery {
    year: i32,
    format: String,
    member: Option<String>,
}

async fn get_tax_year_report(
    Query(query): Query<TaxReportQuery>,
    axum::Extension(state): axum::Extension<Arc<AppState>>,
) -> ApiResult<Json<TaxYearReport>> {
    let report = state
        .tax_service
        .get_tax_year_report(query.year, query.member)
        .await?;
    Ok(Json(report))
}

async fn export_tax_packet(
    Query(query): Query<TaxExportQuery>,
    axum::Extension(state): axum::Extension<Arc<AppState>>,
) -> ApiResult<Json<TaxPacketExport>> {
    let format = TaxExportFormat::from_code(&query.format)?;
    let packet = state
        .tax_service
        .export_tax_packet(query.year, format, query.member)
        .await?;
    Ok(Json(packet))
}

pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/tax/report", get(get_tax_year_report))
        .route("/tax/export", get(export_tax_packet))
}
