use crate::models::{AppConfig, BackupEntry, BackupJob, BackupSnapshot, BackupState, TaskState};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const KEYRING_SERVICE: &str = "com.weepwood.notionfile";
const KEYRING_ACCOUNT: &str = "notion-token";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyConfig {
    folder_path: String,
    root_page_id: String,
    #[serde(default = "default_true")]
    skip_hidden: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyEntry {
    page_id: String,
    hash: String,
    #[serde(default)]
    synced_at: String,
    #[serde(default)]
    mime_type: String,
}

#[derive(Debug, Deserialize)]
struct LegacyState {
    entries: HashMap<String, LegacyEntry>,
}

fn default_true() -> bool { true }

fn ensure_parent(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("无法创建应用数据目录")?;
    }
    Ok(())
}

fn config_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_config_dir()?.join("config.json"))
}

fn state_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("backup-state.json"))
}

fn legacy_state_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("sync-state.json"))
}

pub fn load_config(app: &AppHandle) -> Result<AppConfig> {
    let path = config_path(app)?;
    if !path.exists() { return Ok(AppConfig::default()); }
    let content = std::fs::read_to_string(&path).context("无法读取配置文件")?;
    if let Ok(mut config) = serde_json::from_str::<AppConfig>(&content) {
        if config.jobs.is_empty() { config.jobs.push(BackupJob::default()); }
        if config.active_job_id.is_none() { config.active_job_id = config.jobs.first().map(|job| job.id.clone()); }
        return Ok(config);
    }
    let legacy: LegacyConfig = serde_json::from_str(&content).context("配置文件格式无效")?;
    let mut job = BackupJob::default();
    job.folder_path = legacy.folder_path;
    job.root_page_id = legacy.root_page_id;
    job.skip_hidden = legacy.skip_hidden;
    Ok(AppConfig { jobs: vec![job], active_job_id: Some("default".into()) })
}

pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<()> {
    let path = config_path(app)?;
    ensure_parent(&path)?;
    std::fs::write(path, serde_json::to_string_pretty(config)?).context("无法保存配置文件")
}

pub fn load_state(app: &AppHandle) -> Result<BackupState> {
    let path = state_path(app)?;
    if path.exists() {
        let content = std::fs::read_to_string(&path).context("无法读取备份状态")?;
        return serde_json::from_str(&content).context("备份状态文件格式无效");
    }

    let legacy_path = legacy_state_path(app)?;
    if !legacy_path.exists() { return Ok(BackupState::default()); }
    let content = std::fs::read_to_string(&legacy_path).context("无法读取旧同步状态")?;
    let legacy: LegacyState = serde_json::from_str(&content).context("旧同步状态格式无效")?;
    let entries = legacy.entries.into_iter().map(|(path, entry)| {
        let backed_up_at = if entry.synced_at.is_empty() { chrono::Utc::now().to_rfc3339() } else { entry.synced_at };
        (path, BackupEntry {
            page_id: entry.page_id,
            hash: entry.hash,
            upload_id: None,
            size: 0,
            modified_at: 0,
            backed_up_at,
            mime_type: entry.mime_type,
            version: 1,
            deleted: false,
        })
    }).collect();
    let mut tasks = HashMap::new();
    tasks.insert("default".into(), TaskState { entries, snapshots: Vec::<BackupSnapshot>::new() });
    Ok(BackupState { tasks })
}

pub fn save_state(app: &AppHandle, state: &BackupState) -> Result<()> {
    let path = state_path(app)?;
    ensure_parent(&path)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_string_pretty(state)?).context("无法写入临时备份状态")?;
    std::fs::rename(&temporary, &path).or_else(|_| {
        std::fs::copy(&temporary, &path)?;
        std::fs::remove_file(&temporary)
    }).context("无法原子更新备份状态")?;
    Ok(())
}

fn token_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).context("无法打开系统凭据库")
}

pub fn save_token(token: &str) -> Result<()> {
    if token.trim().is_empty() { anyhow::bail!("Notion Token 不能为空"); }
    token_entry()?.set_password(token.trim()).context("无法将 Token 写入系统凭据库")
}

pub fn load_token() -> Result<String> {
    token_entry()?.get_password().context("尚未保存 Notion Token，或系统凭据库不可用")
}

pub fn has_token() -> bool {
    token_entry().and_then(|entry| entry.get_password().map_err(anyhow::Error::from)).map(|value| !value.trim().is_empty()).unwrap_or(false)
}
