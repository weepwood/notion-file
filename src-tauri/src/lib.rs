mod models;
mod notion;
mod scanner;
mod storage;
mod syncer;

use models::{AppConfig, ScanResult, SyncRequest, SyncResult};
use tauri::AppHandle;

#[tauri::command]
fn get_saved_config(app: AppHandle) -> Result<AppConfig, String> {
    storage::load_config(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    storage::save_config(&app, &config).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_notion_token(token: String) -> Result<(), String> {
    storage::save_token(&token).map_err(|error| error.to_string())
}

#[tauri::command]
fn has_saved_token() -> bool {
    storage::has_token()
}

#[tauri::command]
async fn test_notion_connection(root_page_id: String) -> Result<String, String> {
    let token = storage::load_token().map_err(|error| error.to_string())?;
    let client = notion::NotionClient::new(token).map_err(|error| error.to_string())?;
    client.get_page_title(&root_page_id).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn scan_folder(app: AppHandle, folder_path: String, skip_hidden: bool) -> Result<ScanResult, String> {
    let state = storage::load_state(&app).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || scanner::scan(&folder_path, skip_hidden, &state))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_folder(app: AppHandle, request: SyncRequest) -> Result<SyncResult, String> {
    syncer::synchronize(&app, request).await.map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_saved_config,
            save_config,
            save_notion_token,
            has_saved_token,
            test_notion_connection,
            scan_folder,
            sync_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
