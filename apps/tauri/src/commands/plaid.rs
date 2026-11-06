/// Direct Plaid is initially supported by the authenticated self-hosted web runtime.
#[tauri::command]
pub async fn plaid_action() -> Result<serde_json::Value, String> {
    Err("Direct Plaid linking is available in the self-hosted web application.".into())
}
