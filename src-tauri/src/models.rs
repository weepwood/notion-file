use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupJob {
    pub id: String,
    pub name: String,
    pub folder_path: String,
    pub root_page_id: String,
    pub skip_hidden: bool,
    pub include_text_preview: bool,
    pub auto_backup_minutes: u64,
    pub enabled: bool,
    pub last_backup_at: Option<String>,
}

impl Default for BackupJob {
    fn default() -> Self {
        Self {
            id: "default".into(),
            name: "本地文件备份".into(),
            folder_path: String::new(),
            root_page_id: String::new(),
            skip_hidden: true,
            include_text_preview: true,
            auto_backup_minutes: 0,
            enabled: true,
            last_backup_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub jobs: Vec<BackupJob>,
    pub active_job_id: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { jobs: vec![BackupJob::default()], active_job_id: Some("default".into()) }
    }
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
pub struct BackupEntry {
    pub page_id: String,
    pub hash: String,
    pub upload_id: Option<String>,
    pub size: u64,
    pub modified_at: i64,
    pub backed_up_at: String,
    pub mime_type: String,
    pub version: u32,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSnapshot {
    pub id: String,
    pub started_at: String,
    pub finished_at: String,
    pub summary_page_id: Option<String>,
    pub total_files: usize,
    pub total_bytes: u64,
    pub uploaded: usize,
    pub unchanged: usize,
    pub marked_deleted: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaskState {
    pub entries: HashMap<String, BackupEntry>,
    pub snapshots: Vec<BackupSnapshot>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupState {
    pub tasks: HashMap<String, TaskState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupItemResult {
    pub relative_path: String,
    pub status: String,
    pub page_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub started_at: String,
    pub finished_at: String,
    pub snapshot_page_id: Option<String>,
    pub uploaded: usize,
    pub unchanged: usize,
    pub marked_deleted: usize,
    pub failed: usize,
    pub total_bytes: u64,
    pub items: Vec<BackupItemResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub current: usize,
    pub total: usize,
    pub relative_path: String,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub job_id: String,
    pub destination_path: String,
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub restored: usize,
    pub skipped: usize,
    pub failed: usize,
    pub items: Vec<BackupItemResult>,
}
