mod database;
mod drive;
mod ffmpeg;
mod file_upload;
mod models;
mod notion;
mod notion_request;
mod scanner;
mod storage;
mod syncer;
mod uploader;

use models::{
    AppConfig, DriveDownloadRequest, DriveFolderDownloadRequest, DriveFolderDownloadResult,
    DriveInitResult, DriveNode, DriveQueueEnqueueRequest, DriveQueueSnapshot, DriveTransfer,
    DriveUploadRequest, DriveVersion,
    DriveVersionDownloadRequest, DriveVersionUploadRequest, FfmpegStatus, ScanResult,
    SingleUploadRequest, SyncRequest, SyncResult, UploadRecord,
};
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
fn save_notion_token(app: AppHandle, token: String) -> Result<(), String> {
    storage::save_token(&token).map_err(|error| error.to_string())?;
    drive::start_queue_if_ready(&app);
    Ok(())
}

#[tauri::command]
fn has_saved_token() -> bool {
    storage::has_token()
}

#[tauri::command]
async fn detect_ffmpeg() -> FfmpegStatus {
    tauri::async_runtime::spawn_blocking(ffmpeg::detect_ffmpeg)
        .await
        .unwrap_or_else(|error| FfmpegStatus {
            available: false,
            ffmpeg_path: None,
            ffprobe_path: None,
            version: None,
            message: format!("检测 ffmpeg 失败：{error}"),
        })
}

#[tauri::command]
async fn get_upload_history(app: AppHandle) -> Result<Vec<UploadRecord>, String> {
    tauri::async_runtime::spawn_blocking(move || storage::load_upload_history(&app))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn clear_upload_history(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || storage::clear_upload_history(&app))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn upload_single_file(
    app: AppHandle,
    request: SingleUploadRequest,
) -> Result<UploadRecord, String> {
    uploader::upload(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn test_notion_connection(root_page_id: String) -> Result<String, String> {
    let token = storage::load_token().map_err(|error| error.to_string())?;
    let client = notion::NotionClient::new(token).map_err(|error| error.to_string())?;

    if root_page_id.trim().is_empty() {
        client
            .get_connection_label()
            .await
            .map_err(|error| error.to_string())
    } else {
        client
            .get_page_title(&root_page_id)
            .await
            .map_err(|error| error.to_string())
    }
}

#[tauri::command]
async fn scan_folder(
    app: AppHandle,
    folder_path: String,
    skip_hidden: bool,
) -> Result<ScanResult, String> {
    let state = storage::load_state(&app).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || scanner::scan(&folder_path, skip_hidden, &state))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_folder(app: AppHandle, request: SyncRequest) -> Result<SyncResult, String> {
    syncer::synchronize(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn initialize_drive(
    app: AppHandle,
    root_page_id: String,
) -> Result<DriveInitResult, String> {
    drive::initialize(&app, root_page_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn refresh_drive_index(app: AppHandle) -> Result<Vec<DriveNode>, String> {
    drive::refresh_index(&app)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_drive_nodes(app: AppHandle, include_trashed: bool) -> Result<Vec<DriveNode>, String> {
    drive::list_nodes(&app, include_trashed).map_err(|error| error.to_string())
}

#[tauri::command]
async fn create_drive_folder(
    app: AppHandle,
    name: String,
    parent_id: Option<String>,
) -> Result<DriveNode, String> {
    drive::create_folder(&app, name, parent_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn upload_drive_file(
    app: AppHandle,
    request: DriveUploadRequest,
) -> Result<DriveNode, String> {
    drive::upload_file(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {
    drive::queue_snapshot(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn enqueue_drive_uploads(
    app: AppHandle,
    request: DriveQueueEnqueueRequest,
) -> Result<DriveQueueSnapshot, String> {
    drive::enqueue_uploads(&app, request).map_err(|error| error.to_string())
}

#[tauri::command]
fn pause_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {
    drive::pause_queue(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn resume_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {
    drive::resume_queue(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn retry_drive_upload_job(
    app: AppHandle,
    job_id: String,
) -> Result<DriveQueueSnapshot, String> {
    drive::retry_queue_job(&app, job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn cancel_drive_upload_job(
    app: AppHandle,
    job_id: String,
) -> Result<DriveQueueSnapshot, String> {
    drive::cancel_queue_job(&app, job_id).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_finished_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {
    drive::clear_finished_queue(&app).map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_drive_file(
    app: AppHandle,
    request: DriveDownloadRequest,
) -> Result<DriveTransfer, String> {
    drive::download_file(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_drive_folder(
    app: AppHandle,
    request: DriveFolderDownloadRequest,
) -> Result<DriveFolderDownloadResult, String> {
    drive::download_folder(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_drive_versions(app: AppHandle, node_id: String) -> Result<Vec<DriveVersion>, String> {
    drive::list_versions(&app, node_id).map_err(|error| error.to_string())
}

#[tauri::command]
async fn upload_drive_version(
    app: AppHandle,
    request: DriveVersionUploadRequest,
) -> Result<DriveNode, String> {
    drive::upload_version(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_drive_version(
    app: AppHandle,
    request: DriveVersionDownloadRequest,
) -> Result<DriveTransfer, String> {
    drive::download_version(&app, request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn retry_drive_transfer(
    app: AppHandle,
    transfer_id: String,
) -> Result<DriveTransfer, String> {
    drive::retry_transfer(&app, transfer_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn rename_drive_node(
    app: AppHandle,
    node_id: String,
    new_name: String,
) -> Result<DriveNode, String> {
    drive::rename_node(&app, node_id, new_name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn move_drive_node(
    app: AppHandle,
    node_id: String,
    new_parent_id: Option<String>,
) -> Result<DriveNode, String> {
    drive::move_node(&app, node_id, new_parent_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_drive_node_trashed(
    app: AppHandle,
    node_id: String,
    trashed: bool,
) -> Result<usize, String> {
    drive::set_trashed(&app, node_id, trashed)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_drive_transfers(app: AppHandle) -> Result<Vec<DriveTransfer>, String> {
    drive::list_transfers(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_finished_drive_transfers(app: AppHandle) -> Result<usize, String> {
    drive::clear_finished_transfers(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn disconnect_drive(app: AppHandle) -> Result<(), String> {
    drive::disconnect(&app).map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Err(error) = drive::recover_queue(app.handle().clone()) {
                eprintln!("恢复持久化上传队列失败：{error}");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_saved_config,
            save_config,
            save_notion_token,
            has_saved_token,
            detect_ffmpeg,
            get_upload_history,
            clear_upload_history,
            upload_single_file,
            test_notion_connection,
            scan_folder,
            sync_folder,
            initialize_drive,
            refresh_drive_index,
            get_drive_nodes,
            create_drive_folder,
            upload_drive_file,
            get_drive_upload_queue,
            enqueue_drive_uploads,
            pause_drive_upload_queue,
            resume_drive_upload_queue,
            retry_drive_upload_job,
            cancel_drive_upload_job,
            clear_finished_drive_upload_queue,
            download_drive_file,
            download_drive_folder,
            get_drive_versions,
            upload_drive_version,
            download_drive_version,
            retry_drive_transfer,
            rename_drive_node,
            move_drive_node,
            set_drive_node_trashed,
            get_drive_transfers,
            clear_finished_drive_transfers,
            disconnect_drive,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
