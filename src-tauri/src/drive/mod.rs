mod advanced;
mod notion_index;
mod queue;
mod transfer;
mod version_store;

use crate::models::{
    AppConfig, DriveDownloadRequest, DriveFolderDownloadRequest, DriveFolderDownloadResult,
    DriveInitResult, DriveNode, DriveQueueEnqueueRequest, DriveQueueSnapshot, DriveTransfer,
    DriveUploadRequest, DriveVersion,
    DriveVersionDownloadRequest, DriveVersionUploadRequest,
};
use crate::notion::normalize_page_id;
use crate::notion_request::NotionHttp;
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::AppHandle;

const MAX_NAME_CHARS: usize = 240;
const MAX_PATH_CHARS: usize = 1800;
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) struct DriveContext {
    pub config: AppConfig,
    pub http: NotionHttp,
}

pub async fn initialize(app: &AppHandle, root_page_id: String) -> Result<DriveInitResult> {
    let mut config = storage::load_config(app)?;
    if !root_page_id.trim().is_empty() {
        config.root_page_id = normalize_page_id(root_page_id.trim())?;
    }

    let token = storage::load_token()?;
    let http = NotionHttp::new(token)?;
    let mut created = false;

    let has_existing = !config.drive_database_id.trim().is_empty()
        && !config.drive_data_source_id.trim().is_empty();
    let (database_id, data_source_id) = if has_existing {
        notion_index::verify_data_source(&http, &config.drive_data_source_id).await?;
        (
            normalize_page_id(&config.drive_database_id)?,
            normalize_page_id(&config.drive_data_source_id)?,
        )
    } else {
        if config.root_page_id.trim().is_empty() {
            anyhow::bail!("新建云盘前必须填写并共享一个 Notion 父页面");
        }
        created = true;
        notion_index::create_drive_database(&http, &config.root_page_id).await?
    };

    config.drive_database_id = database_id.clone();
    config.drive_data_source_id = data_source_id.clone();
    storage::save_config(app, &config)?;

    let nodes = notion_index::fetch_remote_nodes(&http, &data_source_id).await?;
    storage::replace_drive_nodes(app, &nodes)?;
    for node in &nodes {
        let _ = version_store::ensure_current_version(app, node);
    }

    queue::start_if_ready(app);
    Ok(DriveInitResult {
        database_id,
        data_source_id,
        created,
        node_count: nodes.len(),
    })
}

pub async fn refresh_index(app: &AppHandle) -> Result<Vec<DriveNode>> {
    let context = drive_context(app)?;
    let nodes = notion_index::fetch_remote_nodes(
        &context.http,
        &context.config.drive_data_source_id,
    )
    .await?;
    storage::replace_drive_nodes(app, &nodes)?;
    for node in &nodes {
        let _ = version_store::ensure_current_version(app, node);
    }
    Ok(nodes)
}

pub fn list_nodes(app: &AppHandle, include_trashed: bool) -> Result<Vec<DriveNode>> {
    storage::list_drive_nodes(app, include_trashed)
}

pub fn list_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {
    storage::list_drive_transfers(app)
}

pub fn clear_finished_transfers(app: &AppHandle) -> Result<usize> {
    storage::clear_finished_drive_transfers(app)
}

pub fn disconnect(app: &AppHandle) -> Result<()> {
    let mut config = storage::load_config(app)?;
    config.drive_database_id.clear();
    config.drive_data_source_id.clear();
    storage::save_config(app, &config)?;
    storage::replace_drive_nodes(app, &[])
}

pub async fn create_folder(
    app: &AppHandle,
    name: String,
    parent_id: Option<String>,
) -> Result<DriveNode> {
    let name = validate_name(&name)?;
    let context = drive_context(app)?;
    let parent_path = parent_path(app, parent_id.as_deref())?;
    let logical_path = join_path(&parent_path, &name);
    validate_logical_path(&logical_path)?;
    ensure_unique_path(app, &logical_path, None)?;

    let now = Utc::now().to_rfc3339();
    let mut node = DriveNode {
        id: new_id("node"),
        parent_id,
        node_type: "folder".to_string(),
        name,
        logical_path,
        mime_type: None,
        size: 0,
        sha256: None,
        notion_page_id: String::new(),
        notion_page_url: None,
        notion_block_id: None,
        file_upload_id: None,
        status: "active".to_string(),
        version: 1,
        original_path: None,
        created_at: now.clone(),
        modified_at: now,
    };

    let (page_id, page_url) = notion_index::create_remote_node_page(
        &context.http,
        &context.config.drive_data_source_id,
        &node,
    )
    .await?;
    node.notion_page_id = page_id;
    node.notion_page_url = page_url;
    storage::insert_drive_node(app, &node)?;
    Ok(node)
}

pub async fn upload_file(app: &AppHandle, request: DriveUploadRequest) -> Result<DriveNode> {
    transfer::upload_file(app, request).await
}

pub fn recover_queue(app: AppHandle) -> Result<()> {
    storage::mark_interrupted_transfers(&app, &Utc::now().to_rfc3339())?;
    queue::recover_and_start(app)
}

pub fn start_queue_if_ready(app: &AppHandle) {
    queue::start_if_ready(app);
}

pub fn queue_snapshot(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    queue::snapshot(app)
}

pub fn enqueue_uploads(
    app: &AppHandle,
    request: DriveQueueEnqueueRequest,
) -> Result<DriveQueueSnapshot> {
    queue::enqueue(app, request)
}

pub fn pause_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    queue::pause(app)
}

pub fn resume_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    queue::resume(app)
}

pub fn retry_queue_job(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {
    queue::retry(app, job_id)
}

pub fn cancel_queue_job(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {
    queue::cancel(app, job_id)
}

pub fn clear_finished_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    queue::clear_finished(app)
}

pub async fn download_file(
    app: &AppHandle,
    request: DriveDownloadRequest,
) -> Result<DriveTransfer> {
    transfer::download_file(app, request).await
}

pub async fn download_folder(
    app: &AppHandle,
    request: DriveFolderDownloadRequest,
) -> Result<DriveFolderDownloadResult> {
    advanced::download_folder(app, request).await
}

pub fn list_versions(app: &AppHandle, node_id: String) -> Result<Vec<DriveVersion>> {
    advanced::list_versions(app, node_id)
}

pub async fn upload_version(
    app: &AppHandle,
    request: DriveVersionUploadRequest,
) -> Result<DriveNode> {
    advanced::upload_version(app, request).await
}

pub async fn download_version(
    app: &AppHandle,
    request: DriveVersionDownloadRequest,
) -> Result<DriveTransfer> {
    advanced::download_version(app, request).await
}

pub async fn retry_transfer(
    app: &AppHandle,
    transfer_id: String,
) -> Result<DriveTransfer> {
    advanced::retry_transfer(app, transfer_id).await
}

pub async fn rename_node(app: &AppHandle, node_id: String, new_name: String) -> Result<DriveNode> {
    let new_name = validate_name(&new_name)?;
    let context = drive_context(app)?;
    let root = storage::get_drive_node(app, &node_id)?;
    let parent = parent_path(app, root.parent_id.as_deref())?;
    let new_root_path = join_path(&parent, &new_name);
    validate_logical_path(&new_root_path)?;
    ensure_unique_path(app, &new_root_path, Some(&root.id))?;

    let old_root_path = root.logical_path.clone();
    let old_prefix = format!("{}/", old_root_path.trim_end_matches('/'));
    let new_prefix = format!("{}/", new_root_path.trim_end_matches('/'));
    let now = Utc::now().to_rfc3339();
    let mut nodes = storage::list_drive_subtree(app, &node_id)?;

    for node in &mut nodes {
        if node.id == root.id {
            node.name = new_name.clone();
            node.logical_path = new_root_path.clone();
        } else if node.logical_path.starts_with(&old_prefix) {
            node.logical_path = format!("{}{}", new_prefix, &node.logical_path[old_prefix.len()..]);
        }
        node.modified_at = now.clone();
        notion_index::patch_remote_node(&context.http, node).await?;
    }

    storage::update_drive_nodes(app, &nodes)?;
    nodes
        .into_iter()
        .find(|node| node.id == node_id)
        .context("重命名完成后找不到目标节点")
}

pub async fn move_node(
    app: &AppHandle,
    node_id: String,
    new_parent_id: Option<String>,
) -> Result<DriveNode> {
    let context = drive_context(app)?;
    let root = storage::get_drive_node(app, &node_id)?;
    if new_parent_id.as_deref() == Some(root.id.as_str()) {
        anyhow::bail!("不能将文件夹移动到自身");
    }

    let destination_path = parent_path(app, new_parent_id.as_deref())?;
    if root.is_folder()
        && (destination_path == root.logical_path
            || destination_path.starts_with(&format!("{}/", root.logical_path)))
    {
        anyhow::bail!("不能将文件夹移动到其子目录中");
    }

    let new_root_path = join_path(&destination_path, &root.name);
    validate_logical_path(&new_root_path)?;
    ensure_unique_path(app, &new_root_path, Some(&root.id))?;

    let old_root_path = root.logical_path.clone();
    let old_prefix = format!("{}/", old_root_path.trim_end_matches('/'));
    let new_prefix = format!("{}/", new_root_path.trim_end_matches('/'));
    let now = Utc::now().to_rfc3339();
    let mut nodes = storage::list_drive_subtree(app, &node_id)?;

    for node in &mut nodes {
        if node.id == root.id {
            node.parent_id = new_parent_id.clone();
            node.logical_path = new_root_path.clone();
        } else if node.logical_path.starts_with(&old_prefix) {
            node.logical_path = format!("{}{}", new_prefix, &node.logical_path[old_prefix.len()..]);
        }
        node.modified_at = now.clone();
        notion_index::patch_remote_node(&context.http, node).await?;
    }

    storage::update_drive_nodes(app, &nodes)?;
    nodes
        .into_iter()
        .find(|node| node.id == node_id)
        .context("移动完成后找不到目标节点")
}

pub async fn set_trashed(app: &AppHandle, node_id: String, trashed: bool) -> Result<usize> {
    let context = drive_context(app)?;
    let root = storage::get_drive_node(app, &node_id)?;
    if !trashed {
        if let Some(parent_id) = root.parent_id.as_deref() {
            let parent = storage::get_drive_node(app, parent_id)?;
            if !parent.is_active() {
                anyhow::bail!("请先恢复上级文件夹");
            }
        }
        ensure_unique_path(app, &root.logical_path, Some(&root.id))?;
    }

    let now = Utc::now().to_rfc3339();
    let status = if trashed { "trashed" } else { "active" };
    let mut nodes = storage::list_drive_subtree(app, &node_id)?;
    for node in &mut nodes {
        node.status = status.to_string();
        node.modified_at = now.clone();
        notion_index::patch_remote_node(&context.http, node).await?;
    }
    storage::update_drive_nodes(app, &nodes)?;
    Ok(nodes.len())
}

pub(super) fn drive_context(app: &AppHandle) -> Result<DriveContext> {
    let config = storage::load_config(app)?;
    if config.drive_data_source_id.trim().is_empty() {
        anyhow::bail!("尚未初始化 Notion 云盘");
    }
    let token = storage::load_token()?;
    Ok(DriveContext {
        config,
        http: NotionHttp::new(token)?,
    })
}

pub(super) fn parent_path(app: &AppHandle, parent_id: Option<&str>) -> Result<String> {
    match parent_id.filter(|value| !value.trim().is_empty()) {
        Some(parent_id) => {
            let parent = storage::get_drive_node(app, parent_id)?;
            if !parent.is_folder() {
                anyhow::bail!("目标上级节点不是文件夹");
            }
            if !parent.is_active() {
                anyhow::bail!("不能在回收站文件夹中创建或移动文件");
            }
            Ok(parent.logical_path)
        }
        None => Ok("/".to_string()),
    }
}

pub(super) fn ensure_unique_path(
    app: &AppHandle,
    path: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let conflict = storage::list_drive_nodes(app, false)?
        .into_iter()
        .any(|node| node.logical_path == path && exclude_id != Some(node.id.as_str()));
    if conflict {
        anyhow::bail!("云盘路径已存在：{path}");
    }
    Ok(())
}

pub(super) fn validate_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("名称不能为空");
    }
    if value.contains('/') || value.contains('\\') {
        anyhow::bail!("名称不能包含斜杠或反斜杠");
    }
    if value.chars().count() > MAX_NAME_CHARS {
        anyhow::bail!("名称不能超过 {MAX_NAME_CHARS} 个字符");
    }
    Ok(value.to_string())
}

pub(super) fn validate_logical_path(value: &str) -> Result<()> {
    if value.chars().count() > MAX_PATH_CHARS {
        anyhow::bail!("云盘路径过长，不能超过 {MAX_PATH_CHARS} 个字符");
    }
    Ok(())
}

pub(super) fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

pub(super) fn new_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", Utc::now().timestamp_micros())
}

#[cfg(test)]
mod tests {
    use super::join_path;

    #[test]
    fn joins_virtual_paths() {
        assert_eq!(join_path("/", "a.txt"), "/a.txt");
        assert_eq!(join_path("/docs", "a.txt"), "/docs/a.txt");
    }
}
