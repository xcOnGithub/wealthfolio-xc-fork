use crate::profiles::ProfileAccess;
use wealthfolio_family::{
    BulkInboxAction, FamilySnapshot, FamilyTask, InboxAction, MonthlyDigest, SaveFamilySettings,
};

#[tauri::command]
pub async fn get_family_snapshot(state: ProfileAccess) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .snapshot()
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn refresh_family_inbox(state: ProfileAccess) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .refresh(chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn save_family_settings(
    state: ProfileAccess,
    input: SaveFamilySettings,
) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .save_settings(input)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn update_family_item(
    state: ProfileAccess,
    input: InboxAction,
) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .action(input, chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn update_family_items(
    state: ProfileAccess,
    input: BulkInboxAction,
) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .bulk_action(input, chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn save_family_task(
    state: ProfileAccess,
    input: FamilyTask,
) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .task(input, chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn preview_family_digest(
    state: ProfileAccess,
    month: String,
) -> Result<MonthlyDigest, String> {
    state
        .context()?
        .family_service
        .preview_digest(&month, chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn retry_family_delivery(
    state: ProfileAccess,
    id: String,
) -> Result<FamilySnapshot, String> {
    state
        .context()?
        .family_service
        .retry_delivery(&id, chrono::Utc::now())
        .await
        .map_err(|e| e.to_string())
}
