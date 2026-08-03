use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_upload_display_mode() -> String {
    "file".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub folder_path: String,
    pub root_page_id: String,
    pub archive_deleted: bool,
    pub skip_hidden: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            folder_path: String::new(),
            root_page_id: String::new(),
            archive_deleted: false,
            skip_hidden: true,
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
