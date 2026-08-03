use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_upload_display_mode() -> String {
    "file".to_string()
}

fn default_node_status() -> String {
    "active".to_string()
}

fn default_node_version() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub root_page_id: String,
    #[serde(default)]
    pub archive_deleted: bool,
    #[serde(default = "default_skip_hidden")]
    pub skip_hidden: bool,
    #[serde(default)]
    pub drive_database_id: String,
    #[serde(default)]
    pub drive_data_source_id: String,
    #[serde(default)]
    pub download_directory: String,
}

fn default_skip_hidden() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            folder_path: String::new(),
            root_page_id: String::new(),
            archive_deleted: false,
            skip_hidden: true,
            drive_database_id: String::new(),
            drive_data_source_id: String::new(),
            download_directory: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRequest {
    pub folder_path: String,
    pub root_page_id: String,
    pub archive_deleted: bool,
    pub skip_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleUploadRequest {
    pub file_path: String,
    pub root_page_id: String,
    #[serde(default = "default_upload_display_mode")]
    pub display_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegStatus {
    pub available: bool,
    pub ffmpeg_path: Option<String>,
    pub ffprobe_path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedFile {
    pub relative_path: String,
    pub absolute_path: String,
    pub size: u64,
    pub modified_at: i64,
    pub mime_type: String,
    pub status: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub root: String,
    pub files: Vec<ScannedFile>,
    pub total_bytes: u64,
    pub changed_count: usize,
    pub unchanged_count: usize,
    pub deleted_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncEntry {
    pub page_id: String,
    pub hash: String,
    pub synced_at: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub document_page_id: Option<String>,
    #[serde(default)]
    pub document_page_url: Option<String>,
    #[serde(default)]
    pub entries: HashMap<String, SyncEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItemResult {
    pub relative_path: String,
    pub status: String,
    pub page_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub started_at: String,
    pub finished_at: String,
    pub document_title: String,
    pub page_id: String,
    pub page_url: Option<String>,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub archived: usize,
    pub failed: usize,
    pub items: Vec<SyncItemResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRecord {
    pub id: String,
    pub file_path: String,
    pub file_name: String,
    pub size: u64,
    pub mime_type: String,
    pub sha256: String,
    pub uploaded_at: String,
    pub status: String,
    pub page_id: Option<String>,
    pub page_url: Option<String>,
    pub message: Option<String>,
    #[serde(default = "default_upload_display_mode")]
    pub display_mode: String,
    #[serde(default)]
    pub segment_count: usize,
    #[serde(default)]
    pub used_ffmpeg: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub node_type: String,
    pub name: String,
    pub logical_path: String,
    pub mime_type: Option<String>,
    pub size: u64,
    pub sha256: Option<String>,
    pub notion_page_id: String,
    pub notion_page_url: Option<String>,
    pub notion_block_id: Option<String>,
    pub file_upload_id: Option<String>,
    #[serde(default = "default_node_status")]
    pub status: String,
    #[serde(default = "default_node_version")]
    pub version: i64,
    pub original_path: Option<String>,
    pub created_at: String,
    pub modified_at: String,
}

impl DriveNode {
    pub fn is_folder(&self) -> bool {
        self.node_type == "folder"
    }

    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveTransfer {
    pub id: String,
    pub node_id: Option<String>,
    pub direction: String,
    pub file_name: String,
    pub local_path: Option<String>,
    pub status: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveUploadRequest {
    pub file_path: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveDownloadRequest {
    pub node_id: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFolderDownloadRequest {
    pub folder_id: String,
    pub destination_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveBatchItemResult {
    pub node_id: String,
    pub logical_path: String,
    pub destination_path: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFolderDownloadResult {
    pub folder_id: String,
    pub destination_directory: String,
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub items: Vec<DriveBatchItemResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveVersion {
    pub id: String,
    pub node_id: String,
    pub version: i64,
    pub size: u64,
    pub sha256: String,
    pub mime_type: String,
    pub file_upload_id: String,
    pub notion_block_id: String,
    pub original_path: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveVersionUploadRequest {
    pub node_id: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveVersionDownloadRequest {
    pub version_id: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInitResult {
    pub database_id: String,
    pub data_source_id: String,
    pub created: bool,
    pub node_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub current: usize,
    pub total: usize,
    pub relative_path: String,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadProgress {
    pub current: usize,
    pub total: usize,
    pub stage: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveTransferProgress {
    pub transfer_id: String,
    pub node_id: Option<String>,
    pub direction: String,
    pub file_name: String,
    pub stage: String,
    pub stage_code: String,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub current_speed_bytes_per_second: f64,
    pub average_speed_bytes_per_second: f64,
    pub elapsed_ms: u64,
    pub stage_elapsed_ms: u64,
    pub endpoint_url: Option<String>,
    pub endpoint_host: Option<String>,
    pub current_part: Option<u64>,
    pub total_parts: Option<u64>,
    pub diagnostic_hint: Option<String>,
}
