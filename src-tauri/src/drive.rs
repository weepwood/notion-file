use crate::file_upload;
use crate::models::{
    AppConfig, DriveDownloadRequest, DriveInitResult, DriveNode, DriveTransfer,
    DriveTransferProgress, DriveUploadRequest,
};
use crate::notion::{file_block, normalize_page_id, page_url_from_id};
use crate::notion_request::{parse_json_response, NotionHttp};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
const HASH_BUFFER_SIZE: usize = 1024 * 1024;
const MAX_NAME_CHARS: usize = 240;
const MAX_PATH_CHARS: usize = 1800;
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

struct DriveContext {
    config: AppConfig,
    http: NotionHttp,
}

pub async fn initialize(app: &AppHandle, root_page_id: String) -> Result<DriveInitResult> {
    let mut config = storage::load_config(app)?;
    if !root_page_id.trim().is_empty() {
        config.root_page_id = normalize_page_id(root_page_id.trim())?;
    }
    if config.root_page_id.trim().is_empty() {
        anyhow::bail!("初始化云盘前必须填写并共享一个 Notion 父页面");
    }

    let token = storage::load_token()?;
    let http = NotionHttp::new(token)?;
    let mut created = false;

    let (database_id, data_source_id) = if !config.drive_database_id.trim().is_empty()
        && !config.drive_data_source_id.trim().is_empty()
    {
        verify_data_source(&http, &config.drive_data_source_id).await?;
        (
            normalize_page_id(&config.drive_database_id)?,
            normalize_page_id(&config.drive_data_source_id)?,
        )
    } else {
        created = true;
        create_drive_database(&http, &config.root_page_id).await?
    };

    config.drive_database_id = database_id.clone();
    config.drive_data_source_id = data_source_id.clone();
    storage::save_config(app, &config)?;

    let nodes = fetch_remote_nodes(&http, &data_source_id).await?;
    storage::replace_drive_nodes(app, &nodes)?;

    Ok(DriveInitResult {
        database_id,
        data_source_id,
        created,
        node_count: nodes.len(),
    })
}

pub async fn refresh_index(app: &AppHandle) -> Result<Vec<DriveNode>> {
    let context = drive_context(app)?;
    let nodes = fetch_remote_nodes(&context.http, &context.config.drive_data_source_id).await?;
    storage::replace_drive_nodes(app, &nodes)?;
    Ok(nodes)
}

pub fn list_nodes(app: &AppHandle, include_trashed: bool) -> Result<Vec<DriveNode>> {
    storage::list_drive_nodes(app, include_trashed)
}

pub fn list_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {
    let now = Utc::now().to_rfc3339();
    let _ = storage::mark_interrupted_transfers(app, &now)?;
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

    let (page_id, page_url) = create_remote_node_page(
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
    let path = PathBuf::from(request.file_path.trim());
    if request.file_path.trim().is_empty() {
        anyhow::bail!("请选择需要上传到云盘的文件");
    }
    let metadata = tokio::fs::metadata(&path)
        .await
        .context("无法读取待上传文件")?;
    if !metadata.is_file() {
        anyhow::bail!("所选路径不是文件");
    }

    let canonical_path = std::fs::canonicalize(&path).unwrap_or(path);
    let file_name = canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("无法读取文件名")?
        .to_string();
    let file_name = validate_name(&file_name)?;
    let parent_path = parent_path(app, request.parent_id.as_deref())?;
    let logical_path = join_path(&parent_path, &file_name);
    validate_logical_path(&logical_path)?;
    ensure_unique_path(app, &logical_path, None)?;

    let mime_type = mime_guess::from_path(&canonical_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let size = metadata.len();
    let node_id = new_id("node");
    let transfer_id = new_id("upload");
    let now = Utc::now().to_rfc3339();
    let mut transfer = DriveTransfer {
        id: transfer_id.clone(),
        node_id: Some(node_id.clone()),
        direction: "upload".to_string(),
        file_name: file_name.clone(),
        local_path: Some(canonical_path.to_string_lossy().to_string()),
        status: "running".to_string(),
        total_bytes: size,
        transferred_bytes: 0,
        message: Some("正在计算 SHA-256".to_string()),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    storage::append_drive_transfer(app, &transfer)?;
    emit_progress(
        app,
        &transfer,
        "正在计算 SHA-256",
        0,
        size,
    );

    let result = async {
        let sha256 = hash_file(&canonical_path).await?;
        let context = drive_context(app)?;
        let reusable = storage::find_drive_file_by_hash(app, &sha256)?
            .and_then(|node| node.file_upload_id);

        let file_upload_id = if let Some(upload_id) = reusable {
            emit_progress(app, &transfer, "检测到重复内容，复用远端文件", size, size);
            upload_id
        } else {
            emit_progress(app, &transfer, "正在上传到 Notion", 0, size);
            let app_handle = app.clone();
            let progress_transfer = transfer.clone();
            file_upload::upload_file(
                &context.http,
                &canonical_path,
                &file_name,
                &mime_type,
                size,
                move |part, total| {
                    let transferred = (part * file_upload::MULTI_PART_SIZE).min(size);
                    emit_progress(
                        &app_handle,
                        &progress_transfer,
                        &format!("正在上传分片 {part}/{total}"),
                        transferred,
                        size,
                    );
                },
            )
            .await?
        };

        let created_at = Utc::now().to_rfc3339();
        let mut node = DriveNode {
            id: node_id,
            parent_id: request.parent_id,
            node_type: "file".to_string(),
            name: file_name.clone(),
            logical_path,
            mime_type: Some(mime_type.clone()),
            size,
            sha256: Some(sha256),
            notion_page_id: String::new(),
            notion_page_url: None,
            notion_block_id: None,
            file_upload_id: Some(file_upload_id.clone()),
            status: "active".to_string(),
            version: 1,
            original_path: Some(canonical_path.to_string_lossy().to_string()),
            created_at: created_at.clone(),
            modified_at: created_at,
        };

        let (page_id, page_url) = create_remote_node_page(
            &context.http,
            &context.config.drive_data_source_id,
            &node,
        )
        .await?;
        node.notion_page_id = page_id.clone();
        node.notion_page_url = page_url;

        let block_result = append_file_block(
            &context.http,
            &page_id,
            &file_upload_id,
            &mime_type,
        )
        .await;
        let block_id = match block_result {
            Ok(value) => value,
            Err(error) => {
                let _ = trash_remote_page(&context.http, &page_id).await;
                return Err(error).context("附件写入失败，未完成的文件页面已移入 Notion 回收站");
            }
        };
        node.notion_block_id = Some(block_id.clone());
        if let Err(error) = patch_remote_block_id(&context.http, &page_id, &block_id).await {
            let _ = trash_remote_page(&context.http, &page_id).await;
            return Err(error).context("写入云盘远端索引失败，未完成的文件页面已移入回收站");
        }

        storage::insert_drive_node(app, &node)?;
        Ok(node)
    }
    .await;

    match result {
        Ok(node) => {
            transfer.status = "completed".to_string();
            transfer.transferred_bytes = size;
            transfer.message = Some("上传完成".to_string());
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress(app, &transfer, "上传完成", size, size);
            Ok(node)
        }
        Err(error) => {
            transfer.status = "failed".to_string();
            transfer.message = Some(error.to_string());
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress(app, &transfer, "上传失败", transfer.transferred_bytes, size);
            Err(error)
        }
    }
}

pub async fn download_file(
    app: &AppHandle,
    request: DriveDownloadRequest,
) -> Result<DriveTransfer> {
    if request.destination_path.trim().is_empty() {
        anyhow::bail!("请选择文件保存位置");
    }
    let mut node = storage::get_drive_node(app, &request.node_id)?;
    if node.node_type != "file" {
        anyhow::bail!("只有文件节点可以下载");
    }
    if !node.is_active() {
        anyhow::bail!("回收站中的文件需要先恢复后才能下载");
    }

    let context = drive_context(app)?;
    let transfer_id = new_id("download");
    let now = Utc::now().to_rfc3339();
    let destination = PathBuf::from(request.destination_path.trim());
    let mut transfer = DriveTransfer {
        id: transfer_id,
        node_id: Some(node.id.clone()),
        direction: "download".to_string(),
        file_name: node.name.clone(),
        local_path: Some(destination.to_string_lossy().to_string()),
        status: "running".to_string(),
        total_bytes: node.size,
        transferred_bytes: 0,
        message: Some("正在获取下载地址".to_string()),
        created_at: now.clone(),
        updated_at: now,
    };
    storage::append_drive_transfer(app, &transfer)?;
    emit_progress(app, &transfer, "正在获取下载地址", 0, node.size);

    let result = async {
        let block_id = match node.notion_block_id.as_deref() {
            Some(value) if !value.trim().is_empty() => value.to_string(),
            _ => {
                let discovered = find_first_file_block(&context.http, &node.notion_page_id).await?;
                node.notion_block_id = Some(discovered.clone());
                patch_remote_block_id(&context.http, &node.notion_page_id, &discovered).await?;
                storage::insert_drive_node(app, &node)?;
                discovered
            }
        };

        let mut url = resolve_file_url(&context.http, &block_id).await?;
        let part_path = PathBuf::from(format!("{}.part", destination.to_string_lossy()));
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("无法创建下载目录")?;
        }
        let _ = tokio::fs::remove_file(&part_path).await;

        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(None)
            .build()
            .context("无法初始化下载客户端")?;

        let mut response = client.get(&url).send().await.context("下载请求失败")?;
        if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            url = resolve_file_url(&context.http, &block_id).await?;
            response = client.get(&url).send().await.context("刷新地址后下载请求失败")?;
        }
        if !response.status().is_success() {
            anyhow::bail!("下载失败（HTTP {}）", response.status().as_u16());
        }

        let response_total = response.content_length().unwrap_or(node.size);
        if response_total > 0 {
            transfer.total_bytes = response_total;
        }
        let mut file = tokio::fs::File::create(&part_path)
            .await
            .context("无法创建临时下载文件")?;
        let mut stream = response.bytes_stream();
        let mut transferred = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("读取下载数据失败")?;
            file.write_all(&chunk).await.context("写入下载文件失败")?;
            transferred += chunk.len() as u64;
            transfer.transferred_bytes = transferred;
            emit_progress(
                app,
                &transfer,
                "正在下载",
                transferred,
                transfer.total_bytes,
            );
        }
        file.flush().await.context("刷新下载文件失败")?;
        drop(file);

        if let Some(expected) = node.sha256.as_deref() {
            emit_progress(
                app,
                &transfer,
                "正在校验 SHA-256",
                transferred,
                transfer.total_bytes,
            );
            let actual = hash_file(&part_path).await?;
            if actual != expected {
                anyhow::bail!("下载文件校验失败，临时文件已保留：{}", part_path.display());
            }
        }

        if destination.exists() {
            tokio::fs::remove_file(&destination)
                .await
                .context("无法覆盖目标文件")?;
        }
        tokio::fs::rename(&part_path, &destination)
            .await
            .context("无法将临时下载文件保存为目标文件")?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            transfer.status = "completed".to_string();
            transfer.transferred_bytes = transfer.total_bytes.max(node.size);
            transfer.message = Some("下载并校验完成".to_string());
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress(
                app,
                &transfer,
                "下载完成",
                transfer.transferred_bytes,
                transfer.total_bytes,
            );
            Ok(transfer)
        }
        Err(error) => {
            transfer.status = "failed".to_string();
            transfer.message = Some(error.to_string());
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress(
                app,
                &transfer,
                "下载失败",
                transfer.transferred_bytes,
                transfer.total_bytes,
            );
            Err(error)
        }
    }
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
        patch_remote_node(&context.http, node).await?;
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
        patch_remote_node(&context.http, node).await?;
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
        patch_remote_node(&context.http, node).await?;
    }
    storage::update_drive_nodes(app, &nodes)?;
    Ok(nodes.len())
}

fn drive_context(app: &AppHandle) -> Result<DriveContext> {
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

async fn create_drive_database(http: &NotionHttp, root_page_id: &str) -> Result<(String, String)> {
    let root_page_id = normalize_page_id(root_page_id)?;
    let body = json!({
        "parent": { "type": "page_id", "page_id": root_page_id },
        "title": [{ "type": "text", "text": { "content": "Notion Drive" } }],
        "description": [{
            "type": "text",
            "text": { "content": "由 Notion File 管理的远端文件索引。请勿随意删除属性。" }
        }],
        "is_inline": false,
        "initial_data_source": {
            "title": [{ "type": "text", "text": { "content": "Files" } }],
            "properties": drive_schema()
        },
        "icon": { "type": "emoji", "emoji": "☁️" }
    });
    let value = parse_json_response(
        http.request(Method::POST, format!("{NOTION_BASE_URL}/databases"))
            .json(&body)
            .send()
            .await
            .context("创建 Notion Drive 数据库请求失败")?,
        "创建 Notion Drive 数据库",
    )
    .await?;
    let database_id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Notion 未返回云盘 database_id")?
        .to_string();
    let data_source_id = value
        .get("data_sources")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("initial_data_source")
                .and_then(|source| source.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });

    let data_source_id = match data_source_id {
        Some(value) => value,
        None => retrieve_first_data_source(http, &database_id).await?,
    };
    Ok((database_id, data_source_id))
}

fn drive_schema() -> Value {
    json!({
        "Name": { "title": {} },
        "Node ID": { "rich_text": {} },
        "Parent ID": { "rich_text": {} },
        "Node Type": {
            "select": { "options": [
                { "name": "file", "color": "blue" },
                { "name": "folder", "color": "yellow" }
            ] }
        },
        "Path": { "rich_text": {} },
        "MIME": { "rich_text": {} },
        "Size": { "number": { "format": "number" } },
        "SHA-256": { "rich_text": {} },
        "Status": {
            "select": { "options": [
                { "name": "active", "color": "green" },
                { "name": "trashed", "color": "gray" }
            ] }
        },
        "Version": { "number": { "format": "number" } },
        "File Upload ID": { "rich_text": {} },
        "Block ID": { "rich_text": {} },
        "Original Path": { "rich_text": {} },
        "Created At": { "date": {} },
        "Modified At": { "date": {} }
    })
}

async fn retrieve_first_data_source(http: &NotionHttp, database_id: &str) -> Result<String> {
    let value = parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/databases/{database_id}"),
        )
        .send()
        .await
        .context("读取新建数据库失败")?,
        "读取新建数据库",
    )
    .await?;
    value
        .get("data_sources")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("新建数据库未返回 data_source_id")
}

async fn verify_data_source(http: &NotionHttp, data_source_id: &str) -> Result<()> {
    parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/data_sources/{data_source_id}"),
        )
        .send()
        .await
        .context("验证云盘数据源请求失败")?,
        "验证云盘数据源",
    )
    .await?;
    Ok(())
}

async fn fetch_remote_nodes(http: &NotionHttp, data_source_id: &str) -> Result<Vec<DriveNode>> {
    let mut cursor: Option<String> = None;
    let mut nodes = Vec::new();
    loop {
        let mut body = json!({ "page_size": 100 });
        if let Some(value) = cursor.as_ref() {
            body["start_cursor"] = json!(value);
        }
        let value = parse_json_response(
            http.request(
                Method::POST,
                format!("{NOTION_BASE_URL}/data_sources/{data_source_id}/query"),
            )
            .json(&body)
            .send()
            .await
            .context("查询 Notion Drive 远端索引失败")?,
            "查询 Notion Drive 远端索引",
        )
        .await?;

        if let Some(results) = value.get("results").and_then(Value::as_array) {
            for page in results {
                if let Some(node) = remote_page_to_node(page)? {
                    nodes.push(node);
                }
            }
        }
        if !value
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        cursor = value
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(nodes)
}

fn remote_page_to_node(page: &Value) -> Result<Option<DriveNode>> {
    if page.get("in_trash").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(None);
    }
    let properties = page
        .get("properties")
        .and_then(Value::as_object)
        .context("远端索引页面缺少 properties")?;
    let id = property_text(properties, "Node ID");
    if id.trim().is_empty() {
        return Ok(None);
    }
    let page_id = page
        .get("id")
        .and_then(Value::as_str)
        .context("远端索引页面缺少 page_id")?
        .to_string();
    let parent_id = property_text(properties, "Parent ID");
    let size = property_number(properties, "Size").max(0.0) as u64;
    let version = property_number(properties, "Version").max(1.0) as i64;
    let mime_type = empty_to_none(property_text(properties, "MIME"));
    let sha256 = empty_to_none(property_text(properties, "SHA-256"));
    let block_id = empty_to_none(property_text(properties, "Block ID"));
    let file_upload_id = empty_to_none(property_text(properties, "File Upload ID"));
    let original_path = empty_to_none(property_text(properties, "Original Path"));
    let created_at = property_date(properties, "Created At")
        .or_else(|| page.get("created_time").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let modified_at = property_date(properties, "Modified At")
        .or_else(|| page.get("last_edited_time").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| created_at.clone());

    Ok(Some(DriveNode {
        id,
        parent_id: empty_to_none(parent_id),
        node_type: property_select(properties, "Node Type")
            .unwrap_or_else(|| "file".to_string()),
        name: property_title(properties, "Name"),
        logical_path: property_text(properties, "Path"),
        mime_type,
        size,
        sha256,
        notion_page_id: page_id.clone(),
        notion_page_url: page
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| Some(page_url_from_id(&page_id))),
        notion_block_id: block_id,
        file_upload_id,
        status: property_select(properties, "Status")
            .unwrap_or_else(|| "active".to_string()),
        version,
        original_path,
        created_at,
        modified_at,
    }))
}

async fn create_remote_node_page(
    http: &NotionHttp,
    data_source_id: &str,
    node: &DriveNode,
) -> Result<(String, Option<String>)> {
    let body = json!({
        "parent": { "type": "data_source_id", "data_source_id": data_source_id },
        "properties": remote_properties(node),
        "icon": {
            "type": "emoji",
            "emoji": if node.is_folder() { "📁" } else { "📄" }
        }
    });
    let value = parse_json_response(
        http.request(Method::POST, format!("{NOTION_BASE_URL}/pages"))
            .json(&body)
            .send()
            .await
            .context("创建云盘索引页面失败")?,
        "创建云盘索引页面",
    )
    .await?;
    let page_id = value
        .get("id")
        .and_then(Value::as_str)
        .context("Notion 未返回云盘页面 ID")?
        .to_string();
    let page_url = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| Some(page_url_from_id(&page_id)));
    Ok((page_id, page_url))
}

async fn append_file_block(
    http: &NotionHttp,
    page_id: &str,
    upload_id: &str,
    mime_type: &str,
) -> Result<String> {
    let body = json!({ "children": [file_block(upload_id, mime_type)] });
    let value = parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/blocks/{page_id}/children"),
        )
        .json(&body)
        .send()
        .await
        .context("向云盘页面写入文件块失败")?,
        "向云盘页面写入文件块",
    )
    .await?;
    value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("Notion 未返回新文件块 ID")
}

async fn patch_remote_block_id(http: &NotionHttp, page_id: &str, block_id: &str) -> Result<()> {
    let body = json!({
        "properties": {
            "Block ID": rich_text_property(block_id)
        }
    });
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{page_id}"),
        )
        .json(&body)
        .send()
        .await
        .context("更新远端文件块索引失败")?,
        "更新远端文件块索引",
    )
    .await?;
    Ok(())
}

async fn patch_remote_node(http: &NotionHttp, node: &DriveNode) -> Result<()> {
    let body = json!({ "properties": remote_properties(node) });
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{}", node.notion_page_id),
        )
        .json(&body)
        .send()
        .await
        .with_context(|| format!("更新远端节点“{}”失败", node.logical_path))?,
        "更新云盘远端索引",
    )
    .await?;
    Ok(())
}

async fn trash_remote_page(http: &NotionHttp, page_id: &str) -> Result<()> {
    parse_json_response(
        http.request(
            Method::PATCH,
            format!("{NOTION_BASE_URL}/pages/{page_id}"),
        )
        .json(&json!({ "in_trash": true }))
        .send()
        .await?,
        "清理未完成云盘页面",
    )
    .await?;
    Ok(())
}

fn remote_properties(node: &DriveNode) -> Value {
    json!({
        "Name": {
            "title": [{ "type": "text", "text": { "content": node.name } }]
        },
        "Node ID": rich_text_property(&node.id),
        "Parent ID": rich_text_property(node.parent_id.as_deref().unwrap_or("")),
        "Node Type": { "select": { "name": node.node_type } },
        "Path": rich_text_property(&node.logical_path),
        "MIME": rich_text_property(node.mime_type.as_deref().unwrap_or("")),
        "Size": { "number": node.size },
        "SHA-256": rich_text_property(node.sha256.as_deref().unwrap_or("")),
        "Status": { "select": { "name": node.status } },
        "Version": { "number": node.version },
        "File Upload ID": rich_text_property(node.file_upload_id.as_deref().unwrap_or("")),
        "Block ID": rich_text_property(node.notion_block_id.as_deref().unwrap_or("")),
        "Original Path": rich_text_property(node.original_path.as_deref().unwrap_or("")),
        "Created At": { "date": { "start": node.created_at } },
        "Modified At": { "date": { "start": node.modified_at } }
    })
}

fn rich_text_property(content: &str) -> Value {
    if content.trim().is_empty() {
        json!({ "rich_text": [] })
    } else {
        json!({
            "rich_text": [{
                "type": "text",
                "text": { "content": truncate_chars(content, 1900) }
            }]
        })
    }
}

async fn find_first_file_block(http: &NotionHttp, page_id: &str) -> Result<String> {
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{NOTION_BASE_URL}/blocks/{page_id}/children?page_size=100");
        if let Some(value) = cursor.as_ref() {
            url.push_str("&start_cursor=");
            url.push_str(value);
        }
        let result = parse_json_response(
            http.request(Method::GET, url).send().await?,
            "查找云盘文件块",
        )
        .await?;
        if let Some(blocks) = result.get("results").and_then(Value::as_array) {
            for block in blocks {
                if is_file_block(block) {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        return Ok(id.to_string());
                    }
                }
            }
        }
        if !result
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        cursor = result
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    anyhow::bail!("该云盘页面中没有可下载的文件块")
}

async fn resolve_file_url(http: &NotionHttp, block_id: &str) -> Result<String> {
    let value = parse_json_response(
        http.request(
            Method::GET,
            format!("{NOTION_BASE_URL}/blocks/{block_id}"),
        )
        .send()
        .await
        .context("读取文件块失败")?,
        "读取文件块",
    )
    .await?;
    extract_file_url(&value).context("Notion 文件块没有返回可下载地址")
}

fn extract_file_url(block: &Value) -> Option<String> {
    let block_type = block.get("type")?.as_str()?;
    if !matches!(block_type, "file" | "image" | "video" | "audio" | "pdf") {
        return None;
    }
    let body = block.get(block_type)?;
    body.pointer("/file/url")
        .or_else(|| body.pointer("/external/url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn is_file_block(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("file" | "image" | "video" | "audio" | "pdf")
    )
}

fn parent_path(app: &AppHandle, parent_id: Option<&str>) -> Result<String> {
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

fn ensure_unique_path(app: &AppHandle, path: &str, exclude_id: Option<&str>) -> Result<()> {
    let conflict = storage::list_drive_nodes(app, false)?
        .into_iter()
        .any(|node| node.logical_path == path && exclude_id != Some(node.id.as_str()));
    if conflict {
        anyhow::bail!("云盘路径已存在：{path}");
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("名称不能为空");
    }
    if value.contains('/') || value.contains('\\') {
        anyhow::bail!("名称不能包含 / 或 \\");
    }
    if value.chars().count() > MAX_NAME_CHARS {
        anyhow::bail!("名称不能超过 {MAX_NAME_CHARS} 个字符");
    }
    Ok(value.to_string())
}

fn validate_logical_path(value: &str) -> Result<()> {
    if value.chars().count() > MAX_PATH_CHARS {
        anyhow::bail!("云盘路径过长，不能超过 {MAX_PATH_CHARS} 个字符");
    }
    Ok(())
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("无法打开文件计算校验值：{}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn emit_progress(
    app: &AppHandle,
    transfer: &DriveTransfer,
    stage: &str,
    transferred_bytes: u64,
    total_bytes: u64,
) {
    let _ = app.emit(
        "drive-transfer-progress",
        DriveTransferProgress {
            transfer_id: transfer.id.clone(),
            node_id: transfer.node_id.clone(),
            direction: transfer.direction.clone(),
            file_name: transfer.file_name.clone(),
            stage: stage.to_string(),
            transferred_bytes,
            total_bytes,
        },
    );
}

fn new_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", Utc::now().timestamp_micros())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn empty_to_none(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn property_title(properties: &Map<String, Value>, name: &str) -> String {
    properties
        .get(name)
        .and_then(|value| value.get("title"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("plain_text"))
        .and_then(Value::as_str)
        .unwrap_or("未命名")
        .to_string()
}

fn property_text(properties: &Map<String, Value>, name: &str) -> String {
    properties
        .get(name)
        .and_then(|value| value.get("rich_text"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("plain_text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn property_select(properties: &Map<String, Value>, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|value| value.get("select"))
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn property_number(properties: &Map<String, Value>, name: &str) -> f64 {
    properties
        .get(name)
        .and_then(|value| value.get("number"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn property_date(properties: &Map<String, Value>, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|value| value.get("date"))
        .and_then(|value| value.get("start"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{extract_file_url, join_path, remote_page_to_node};
    use serde_json::json;

    #[test]
    fn joins_virtual_paths() {
        assert_eq!(join_path("/", "a.txt"), "/a.txt");
        assert_eq!(join_path("/docs", "a.txt"), "/docs/a.txt");
    }

    #[test]
    fn extracts_signed_file_url() {
        let block = json!({
            "type": "file",
            "file": {
                "type": "file",
                "file": { "url": "https://example.com/file", "expiry_time": "x" }
            }
        });
        assert_eq!(extract_file_url(&block).as_deref(), Some("https://example.com/file"));
    }

    #[test]
    fn parses_remote_node_properties() {
        let page = json!({
            "id": "page-1",
            "url": "https://notion.so/page-1",
            "created_time": "2026-08-03T00:00:00Z",
            "last_edited_time": "2026-08-03T00:00:00Z",
            "properties": {
                "Name": { "title": [{ "plain_text": "docs" }] },
                "Node ID": { "rich_text": [{ "plain_text": "node-1" }] },
                "Parent ID": { "rich_text": [] },
                "Node Type": { "select": { "name": "folder" } },
                "Path": { "rich_text": [{ "plain_text": "/docs" }] },
                "MIME": { "rich_text": [] },
                "Size": { "number": 0 },
                "SHA-256": { "rich_text": [] },
                "Status": { "select": { "name": "active" } },
                "Version": { "number": 1 },
                "File Upload ID": { "rich_text": [] },
                "Block ID": { "rich_text": [] },
                "Original Path": { "rich_text": [] },
                "Created At": { "date": { "start": "2026-08-03T00:00:00Z" } },
                "Modified At": { "date": { "start": "2026-08-03T00:00:00Z" } }
            }
        });
        let node = remote_page_to_node(&page).unwrap().unwrap();
        assert_eq!(node.id, "node-1");
        assert!(node.is_folder());
        assert_eq!(node.logical_path, "/docs");
    }
}
