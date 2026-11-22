use crate::{
    error::{ApiError, ApiResult},
    AppState,
};
use axum::{
    extract::{Extension, Query},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use wealthfolio_family::{
    FamilySnapshot, FamilyTask, InboxAction, MonthlyDigest, SaveFamilySettings,
};

fn mutation_origin(headers: &HeaderMap) -> ApiResult<()> {
    let origin = headers
        .get("origin")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| reqwest::Url::parse(h).ok());
    let host = headers.get("host").and_then(|h| h.to_str().ok());
    let valid = origin.as_ref().zip(host).is_some_and(|(origin, host)| {
        matches!(origin.scheme(), "http" | "https")
            && origin.username().is_empty()
            && origin.password().is_none()
            && reqwest::Url::parse(&format!("{}://{host}", origin.scheme()))
                .is_ok_and(|expected| expected.origin() == origin.origin())
    }) && headers
        .get("sec-fetch-site")
        .and_then(|h| h.to_str().ok())
        .is_none_or(|h| matches!(h, "same-origin" | "none"));
    if valid {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "Family mutations require a same-origin request".into(),
        ))
    }
}
async fn snapshot(Extension(state): Extension<Arc<AppState>>) -> ApiResult<Json<FamilySnapshot>> {
    Ok(Json(state.family_service.snapshot().await?))
}
async fn refresh(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<FamilySnapshot>> {
    mutation_origin(&headers)?;
    Ok(Json(
        state.family_service.refresh(chrono::Utc::now()).await?,
    ))
}
async fn settings(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<SaveFamilySettings>,
) -> ApiResult<Json<FamilySnapshot>> {
    mutation_origin(&headers)?;
    Ok(Json(state.family_service.save_settings(input).await?))
}
async fn action(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<InboxAction>,
) -> ApiResult<Json<FamilySnapshot>> {
    mutation_origin(&headers)?;
    Ok(Json(
        state
            .family_service
            .action(input, chrono::Utc::now())
            .await?,
    ))
}
async fn task(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<FamilyTask>,
) -> ApiResult<Json<FamilySnapshot>> {
    mutation_origin(&headers)?;
    Ok(Json(
        state.family_service.task(input, chrono::Utc::now()).await?,
    ))
}
#[derive(serde::Deserialize)]
struct DigestQuery {
    month: String,
}
async fn digest(
    Extension(state): Extension<Arc<AppState>>,
    Query(query): Query<DigestQuery>,
) -> ApiResult<Json<MonthlyDigest>> {
    Ok(Json(
        state
            .family_service
            .preview_digest(&query.month, chrono::Utc::now())
            .await?,
    ))
}
#[derive(serde::Deserialize)]
struct Retry {
    id: String,
}
async fn retry(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<Retry>,
) -> ApiResult<Json<FamilySnapshot>> {
    mutation_origin(&headers)?;
    Ok(Json(
        state
            .family_service
            .retry_delivery(&input.id, chrono::Utc::now())
            .await?,
    ))
}
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/family", get(snapshot))
        .route("/family/refresh", post(refresh))
        .route("/family/settings", post(settings))
        .route("/family/action", post(action))
        .route("/family/task", post(task))
        .route("/family/digest", get(digest))
        .route("/family/retry", post(retry))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mutations_require_matching_origin_and_fetch_site() {
        let mut h = HeaderMap::new();
        h.insert("host", "money.example".parse().unwrap());
        assert!(mutation_origin(&h).is_err());
        h.insert("origin", "https://evil.example".parse().unwrap());
        assert!(mutation_origin(&h).is_err());
        h.insert("origin", "https://money.example".parse().unwrap());
        assert!(mutation_origin(&h).is_ok());
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert!(mutation_origin(&h).is_err());
    }
}
