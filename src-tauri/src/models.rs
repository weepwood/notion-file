use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
        Self { folder_path: String::new(), root_page_id: String::new(), archive_deleted: false, skip_hidden: true }
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
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub archived: usize,
    pub failed: usize,
    pub items: Vec<SyncItemResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub current: usize,
    pub total: usize,
    pub relative_path: String,
    pub stage: String,
}
