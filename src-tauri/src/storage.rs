use crate::database;
use crate::models::{AppConfig, DriveNode, DriveTransfer, SyncState, UploadRecord};
use anyhow::{Context, Result};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const KEYRING_SERVICE: &str = "com.weepwood.notionfile";
const KEYRING_ACCOUNT: &str = "notion-token";

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
    Ok(app.path().app_data_dir()?.join("sync-state.json"))
}

fn upload_history_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("upload-history.json"))
}

pub fn load_config(app: &AppHandle) -> Result<AppConfig> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = std::fs::read_to_string(&path).context("无法读取配置文件")?;
    serde_json::from_str(&content).context("配置文件格式无效")
}

pub fn save_config(app: &AppHandle, config: &AppConfig) -> Result<()> {
    let path = config_path(app)?;
    ensure_parent(&path)?;
    std::fs::write(path, serde_json::to_string_pretty(config)?).context("无法保存配置文件")
}

pub fn load_state(app: &AppHandle) -> Result<SyncState> {
    let path = state_path(app)?;
    if !path.exists() {
        return Ok(SyncState::default());
    }
    let content = std::fs::read_to_string(&path).context("无法读取同步状态")?;
    serde_json::from_str(&content).context("同步状态文件格式无效")
}

pub fn save_state(app: &AppHandle, state: &SyncState) -> Result<()> {
    let path = state_path(app)?;
    ensure_parent(&path)?;
    std::fs::write(path, serde_json::to_string_pretty(state)?).context("无法写入同步状态")
}

pub fn load_upload_history(app: &AppHandle) -> Result<Vec<UploadRecord>> {
    let legacy_path = upload_history_path(app)?;
    database::load_upload_history(app, &legacy_path)
}

pub fn append_upload_record(app: &AppHandle, record: UploadRecord) -> Result<()> {
    let legacy_path = upload_history_path(app)?;
    database::append_upload_record(app, &legacy_path, &record)
}

pub fn clear_upload_history(app: &AppHandle) -> Result<()> {
    let legacy_path = upload_history_path(app)?;
    database::clear_upload_history(app, &legacy_path)
}

pub fn list_drive_nodes(app: &AppHandle, include_trashed: bool) -> Result<Vec<DriveNode>> {
    database::list_drive_nodes(app, include_trashed)
}

pub fn get_drive_node(app: &AppHandle, node_id: &str) -> Result<DriveNode> {
    database::get_drive_node(app, node_id)
}

pub fn insert_drive_node(app: &AppHandle, node: &DriveNode) -> Result<()> {
    database::insert_drive_node(app, node)
}

pub fn update_drive_nodes(app: &AppHandle, nodes: &[DriveNode]) -> Result<()> {
    database::update_drive_nodes(app, nodes)
}

pub fn replace_drive_nodes(app: &AppHandle, nodes: &[DriveNode]) -> Result<()> {
    database::replace_drive_nodes(app, nodes)
}

pub fn find_drive_file_by_hash(app: &AppHandle, sha256: &str) -> Result<Option<DriveNode>> {
    database::find_drive_file_by_hash(app, sha256)
}

pub fn list_drive_subtree(app: &AppHandle, root_id: &str) -> Result<Vec<DriveNode>> {
    database::list_drive_subtree(app, root_id)
}

pub fn append_drive_transfer(app: &AppHandle, transfer: &DriveTransfer) -> Result<()> {
    database::append_drive_transfer(app, transfer)
}

pub fn update_drive_transfer(app: &AppHandle, transfer: &DriveTransfer) -> Result<()> {
    database::update_drive_transfer(app, transfer)
}

pub fn list_drive_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {
    database::list_drive_transfers(app)
}

pub fn mark_interrupted_transfers(app: &AppHandle, updated_at: &str) -> Result<usize> {
    database::mark_interrupted_transfers(app, updated_at)
}

pub fn clear_finished_drive_transfers(app: &AppHandle) -> Result<usize> {
    database::clear_finished_drive_transfers(app)
}

fn token_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .context("无法打开系统凭据库；请确认当前 Windows 用户可使用凭据管理器")
}

pub fn save_token(token: &str) -> Result<()> {
    let token = token.trim();
    if token.is_empty() {
        anyhow::bail!("Notion Token 不能为空");
    }

    let entry = token_entry()?;
    entry
        .set_password(token)
        .context("无法将 Token 写入系统凭据库")?;

    let saved = entry
        .get_password()
        .context("Token 已提交保存，但无法从系统凭据库读回")?;

    if saved != token {
        anyhow::bail!("系统凭据库回读结果不一致，Token 未可靠保存");
    }

    Ok(())
}

pub fn load_token() -> Result<String> {
    let token = token_entry()?
        .get_password()
        .context("无法从系统凭据库读取 Notion Token；请在连接设置中重新保存 Token")?;

    if token.trim().is_empty() {
        anyhow::bail!("系统凭据库中的 Notion Token 为空，请重新保存");
    }

    Ok(token)
}

pub fn has_token() -> bool {
    load_token().is_ok()
}
