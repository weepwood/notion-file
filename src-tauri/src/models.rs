use serde::{Deserialize, Serialize, Serializer};
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

/// Windows 的 `canonicalize` 会返回 `\\?\C:\...` 或
/// `\\?\UNC\server\share\...` 形式的扩展长度路径。该前缀是系统内部语法，
/// 应保留在 Rust/SQLite 内部以兼容超长路径，但不应该直接显示给用户。
fn user_visible_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    path.to_string()
}

fn serialize_path<S>(path: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&user_visible_path(path))
}

fn serialize_optional_path<S>(path: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match path {
        Some(value) => serializer.serialize_some(&user_visible_path(value)),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default, serialize_with = "serialize_path")]
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
    #[serde(default, serialize_with = "serialize_path")]
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
    #[serde(serialize_with = "serialize_path")]
    pub folder_path: String,
    pub root_page_id: String,
    pub archive_deleted: bool,
    pub skip_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleUploadRequest {
    #[serde(serialize_with = "serialize_path")]
    pub file_path: String,
    pub root_page_id: String,
    #[serde(default = "default_upload_display_mode")]
    pub display_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegStatus {
    pub available: bool,
    #[serde(serialize_with = "serialize_optional_path")]
    pub ffmpeg_path: Option<String>,
    #[serde(serialize_with = "serialize_optional_path")]
    pub ffprobe_path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedFile {
    pub relative_path: String,
    #[serde(serialize_with = "serialize_path")]
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
    #[serde(serialize_with = "serialize_path")]
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
    #[serde(default, serialize_with = "serialize_path")]
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
    #[serde(serialize_with = "serialize_path")]
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
    #[serde(serialize_with = "serialize_optional_path")]
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
    #[serde(serialize_with = "serialize_optional_path")]
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
    #[serde(serialize_with = "serialize_path")]
    pub file_path: String,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveQueueEnqueueRequest {
    pub file_paths: Vec<String>,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveQueueJob {
    pub id: String,
    pub node_id: String,
    pub parent_id: Option<String>,
    #[serde(serialize_with = "serialize_path")]
    pub file_path: String,
    pub file_name: String,
    pub size: u64,
    pub status: String,
    pub stage: String,
    pub transferred_bytes: u64,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveQueueSnapshot {
    pub paused: bool,
    pub worker_running: bool,
    pub running_job_id: Option<String>,
    pub pending_count: usize,
    pub failed_count: usize,
    pub jobs: Vec<DriveQueueJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveDownloadRequest {
    pub node_id: String,
    #[serde(serialize_with = "serialize_path")]
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFolderDownloadRequest {
    pub folder_id: String,
    #[serde(serialize_with = "serialize_path")]
    pub destination_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveBatchItemResult {
    pub node_id: String,
    pub logical_path: String,
    #[serde(serialize_with = "serialize_path")]
    pub destination_path: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFolderDownloadResult {
    pub folder_id: String,
    #[serde(serialize_with = "serialize_path")]
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
    #[serde(serialize_with = "serialize_optional_path")]
    pub original_path: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveVersionUploadRequest {
    pub node_id: String,
    #[serde(serialize_with = "serialize_path")]
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveVersionDownloadRequest {
    pub version_id: String,
    #[serde(serialize_with = "serialize_path")]
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

#[cfg(test)]
mod tests {
    use super::{user_visible_path, DriveQueueJob};

    #[test]
    fn removes_windows_verbatim_drive_prefix_for_display() {
        assert_eq!(user_visible_path(r"\\?\E:\H\example.txt"), r"E:\H\example.txt");
    }

    #[test]
    fn converts_windows_verbatim_unc_prefix_for_display() {
        assert_eq!(
            user_visible_path(r"\\?\UNC\server\share\example.txt"),
            r"\\server\share\example.txt"
        );
    }

    #[test]
    fn serializes_queue_path_without_internal_windows_prefix() {
        let job = DriveQueueJob {
            id: "queue-1".to_string(),
            node_id: "node-1".to_string(),
            parent_id: None,
            file_path: r"\\?\E:\H\example.txt".to_string(),
            file_name: "example.txt".to_string(),
            size: 1,
            status: "pending".to_string(),
            stage: "pending".to_string(),
            transferred_bytes: 0,
            attempts: 0,
            last_error: None,
            created_at: "2026-08-03T00:00:00Z".to_string(),
            updated_at: "2026-08-03T00:00:00Z".to_string(),
            started_at: None,
            completed_at: None,
        };
        let value = serde_json::to_value(job).unwrap();
        assert_eq!(value["filePath"], r"E:\H\example.txt");
    }
}
