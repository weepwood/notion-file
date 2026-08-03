use super::{
    drive_context, ensure_unique_path, join_path, new_id, notion_index, parent_path,
    validate_logical_path, validate_name, version_store,
};
use crate::file_upload;
use crate::models::{
    DriveDownloadRequest, DriveNode, DriveTransfer, DriveTransferProgress, DriveUploadRequest,
    DriveVersion,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;
const TRANSFER_PERSIST_INTERVAL: u64 = 8 * 1024 * 1024;

pub(super) async fn upload_file(
    app: &AppHandle,
    request: DriveUploadRequest,
) -> Result<DriveNode> {
    if request.file_path.trim().is_empty() {
        anyhow::bail!("请选择需要上传到云盘的文件");
    }
    let requested_path = PathBuf::from(request.file_path.trim());
    let metadata = tokio::fs::metadata(&requested_path)
        .await
        .context("无法读取待上传文件")?;
    if !metadata.is_file() {
        anyhow::bail!("所选路径不是文件");
    }

    let canonical_path = std::fs::canonicalize(&requested_path).unwrap_or(requested_path);
    let file_name = canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("无法读取文件名")?;
    let file_name = validate_name(file_name)?;
    let parent_id = request.parent_id;
    let virtual_parent = parent_path(app, parent_id.as_deref())?;
    let logical_path = join_path(&virtual_parent, &file_name);
    validate_logical_path(&logical_path)?;
    ensure_unique_path(app, &logical_path, None)?;

    let mime_type = mime_guess::from_path(&canonical_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let size = metadata.len();
    let node_id = new_id("node");
    let now = Utc::now().to_rfc3339();
    let mut transfer = DriveTransfer {
        id: new_id("upload"),
        node_id: Some(node_id.clone()),
        direction: "upload".to_string(),
        file_name: file_name.clone(),
        local_path: Some(canonical_path.to_string_lossy().to_string()),
        status: "running".to_string(),
        total_bytes: size,
        transferred_bytes: 0,
        message: Some("正在计算 SHA-256".to_string()),
        created_at: now.clone(),
        updated_at: now,
    };
    storage::append_drive_transfer(app, &transfer)?;
    emit_progress(app, &transfer, "正在计算 SHA-256", 0, size);

    let result: Result<DriveNode> = async {
        let sha256 = hash_file(&canonical_path).await?;
        let context = drive_context(app)?;
        let reusable_upload_id = storage::find_drive_file_by_hash(app, &sha256)?
            .and_then(|node| node.file_upload_id);

        let file_upload_id = if let Some(upload_id) = reusable_upload_id {
            emit_progress(
                app,
                &transfer,
                "检测到重复内容，复用远端文件",
                size,
                size,
            );
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
            parent_id: parent_id.clone(),
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

        let (page_id, page_url) = notion_index::create_remote_node_page(
            &context.http,
            &context.config.drive_data_source_id,
            &node,
        )
        .await?;
        node.notion_page_id = page_id.clone();
        node.notion_page_url = page_url;

        let block_id = match notion_index::append_file_block(
            &context.http,
            &page_id,
            &file_upload_id,
            &mime_type,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = notion_index::trash_remote_page(&context.http, &page_id).await;
                return Err(error)
                    .context("附件写入失败，未完成的文件页面已移入 Notion 回收站");
            }
        };
        node.notion_block_id = Some(block_id.clone());

        if let Err(error) = notion_index::patch_remote_block_id(
            &context.http,
            &page_id,
            &block_id,
        )
        .await
        {
            let _ = notion_index::trash_remote_page(&context.http, &page_id).await;
            return Err(error)
                .context("写入云盘远端索引失败，未完成的文件页面已移入回收站");
        }

        storage::insert_drive_node(app, &node)?;
        version_store::ensure_current_version(app, &node)?;
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
            emit_progress(
                app,
                &transfer,
                "上传失败",
                transfer.transferred_bytes,
                size,
            );
            Err(error)
        }
    }
}

pub(super) async fn download_file(
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
    let block_id = match node.notion_block_id.as_deref() {
        Some(value) if !value.trim().is_empty() => value.to_string(),
        _ => {
            let discovered = notion_index::find_first_file_block(
                &context.http,
                &node.notion_page_id,
            )
            .await?;
            node.notion_block_id = Some(discovered.clone());
            notion_index::patch_remote_block_id(
                &context.http,
                &node.notion_page_id,
                &discovered,
            )
            .await?;
            storage::insert_drive_node(app, &node)?;
            discovered
        }
    };

    download_block(
        app,
        Some(node.id.clone()),
        node.name.clone(),
        block_id,
        node.sha256.clone(),
        node.size,
        PathBuf::from(request.destination_path.trim()),
    )
    .await
}

pub(super) async fn download_version(
    app: &AppHandle,
    version: &DriveVersion,
    destination_path: String,
) -> Result<DriveTransfer> {
    if destination_path.trim().is_empty() {
        anyhow::bail!("请选择版本文件保存位置");
    }
    download_block(
        app,
        Some(version.node_id.clone()),
        format!("版本 v{}", version.version),
        version.notion_block_id.clone(),
        Some(version.sha256.clone()),
        version.size,
        PathBuf::from(destination_path.trim()),
    )
    .await
}

pub(super) async fn retry_download(
    app: &AppHandle,
    transfer_id: String,
) -> Result<DriveTransfer> {
    let transfer = storage::list_drive_transfers(app)?
        .into_iter()
        .find(|item| item.id == transfer_id)
        .context("找不到需要续传的下载记录")?;
    if transfer.direction != "download" {
        anyhow::bail!("只有下载任务可以续传");
    }
    let node_id = transfer.node_id.context("下载记录缺少云盘节点 ID")?;
    let destination_path = transfer.local_path.context("下载记录缺少本地保存路径")?;
    download_file(
        app,
        DriveDownloadRequest {
            node_id,
            destination_path,
        },
    )
    .await
}

async fn download_block(
    app: &AppHandle,
    node_id: Option<String>,
    file_name: String,
    block_id: String,
    expected_sha256: Option<String>,
    expected_size: u64,
    destination: PathBuf,
) -> Result<DriveTransfer> {
    let context = drive_context(app)?;
    let now = Utc::now().to_rfc3339();
    let part_path = part_path_for(&destination);
    let mut existing = tokio::fs::metadata(&part_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if expected_size > 0 && existing > expected_size {
        tokio::fs::remove_file(&part_path)
            .await
            .context("现有临时文件大于远端文件，无法重新初始化")?;
        existing = 0;
    }

    let mut transfer = DriveTransfer {
        id: new_id("download"),
        node_id,
        direction: "download".to_string(),
        file_name,
        local_path: Some(destination.to_string_lossy().to_string()),
        status: "running".to_string(),
        total_bytes: expected_size,
        transferred_bytes: existing,
        message: Some(if existing > 0 {
            format!("从 {} 继续下载", format_bytes(existing))
        } else {
            "正在获取下载地址".to_string()
        }),
        created_at: now.clone(),
        updated_at: now,
    };
    storage::append_drive_transfer(app, &transfer)?;
    emit_progress(
        app,
        &transfer,
        transfer.message.as_deref().unwrap_or("正在下载"),
        existing,
        expected_size,
    );

    let result = download_block_inner(
        app,
        &context.http,
        &block_id,
        expected_sha256.as_deref(),
        &destination,
        &part_path,
        &mut transfer,
    )
    .await;

    match result {
        Ok(()) => {
            transfer.status = "completed".to_string();
            transfer.message = Some(if existing > 0 {
                "续传、校验并保存完成".to_string()
            } else {
                "下载并校验完成".to_string()
            });
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
            let resumable = tokio::fs::metadata(&part_path)
                .await
                .map(|metadata| metadata.len() > 0)
                .unwrap_or(false);
            transfer.status = "failed".to_string();
            transfer.message = Some(if resumable {
                format!("{error}；临时文件保留，可稍后续传")
            } else {
                format!("{error}；未保留可续传的临时文件")
            });
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress(
                app,
                &transfer,
                if resumable { "下载中断，可续传" } else { "下载失败，需要重下" },
                transfer.transferred_bytes,
                transfer.total_bytes,
            );
            Err(error)
        }
    }
}

async fn download_block_inner(
    app: &AppHandle,
    http: &crate::notion_request::NotionHttp,
    block_id: &str,
    expected_sha256: Option<&str>,
    destination: &Path,
    part_path: &Path,
    transfer: &mut DriveTransfer,
) -> Result<()> {
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("无法创建下载目录")?;
    }

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .context("无法初始化下载客户端")?;
    let existing = tokio::fs::metadata(part_path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    if transfer.total_bytes > 0 && existing == transfer.total_bytes {
        transfer.transferred_bytes = existing;
    } else {
        let mut response = request_download(&client, http, block_id, existing).await?;
        if response.status() == StatusCode::RANGE_NOT_SATISFIABLE {
            if transfer.total_bytes > 0 && existing == transfer.total_bytes {
                transfer.transferred_bytes = existing;
            } else {
                let _ = tokio::fs::remove_file(part_path).await;
                response = request_download(&client, http, block_id, 0).await?;
                ensure_success_status(&response)?;
                update_total_from_response(&response, transfer, 0);
                write_response(app, part_path, response, transfer, 0).await?;
            }
        } else if response.status() == StatusCode::PARTIAL_CONTENT {
            let range_start = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range_start);
            if existing > 0 && range_start != Some(existing) {
                let _ = tokio::fs::remove_file(part_path).await;
                response = request_download(&client, http, block_id, 0).await?;
                ensure_success_status(&response)?;
                update_total_from_response(&response, transfer, 0);
                write_response(app, part_path, response, transfer, 0).await?;
            } else {
                update_total_from_response(&response, transfer, existing);
                write_response(app, part_path, response, transfer, existing).await?;
            }
        } else if response.status().is_success() {
            update_total_from_response(&response, transfer, 0);
            write_response(app, part_path, response, transfer, 0).await?;
        } else {
            anyhow::bail!("下载失败（HTTP {}）", response.status().as_u16());
        }
    }

    let actual_size = tokio::fs::metadata(part_path)
        .await
        .context("无法读取临时下载文件大小")?
        .len();
    transfer.transferred_bytes = actual_size;
    if transfer.total_bytes > 0 && actual_size != transfer.total_bytes {
        if actual_size > transfer.total_bytes {
            let _ = tokio::fs::remove_file(part_path).await;
            anyhow::bail!(
                "下载文件大小异常：实际 {actual_size} 字节，大于预期 {} 字节，损坏临时文件已删除",
                transfer.total_bytes
            );
        }
        anyhow::bail!(
            "下载提前结束：已接收 {actual_size} 字节，预期 {} 字节",
            transfer.total_bytes
        );
    }

    if let Some(expected) = expected_sha256 {
        emit_progress(
            app,
            transfer,
            "正在校验 SHA-256",
            transfer.transferred_bytes,
            transfer.total_bytes,
        );
        let actual = hash_file(part_path).await?;
        if actual != expected {
            let _ = tokio::fs::remove_file(part_path).await;
            anyhow::bail!("下载文件校验失败：SHA-256 不一致，损坏临时文件已删除")
        }
    }

    if destination.exists() {
        tokio::fs::remove_file(destination)
            .await
            .context("无法覆盖目标文件")?;
    }
    tokio::fs::rename(part_path, destination)
        .await
        .context("无法将临时下载文件保存为目标文件")?;
    Ok(())
}

async fn request_download(
    client: &Client,
    http: &crate::notion_request::NotionHttp,
    block_id: &str,
    offset: u64,
) -> Result<Response> {
    let mut url = notion_index::resolve_file_url(http, block_id).await?;
    let mut request = client.get(&url);
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let mut response = request.send().await.context("下载请求失败")?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        url = notion_index::resolve_file_url(http, block_id).await?;
        let mut retry = client.get(&url);
        if offset > 0 {
            retry = retry.header(RANGE, format!("bytes={offset}-"));
        }
        response = retry
            .send()
            .await
            .context("刷新签名地址后下载请求失败")?;
    }
    Ok(response)
}

fn ensure_success_status(response: &Response) -> Result<()> {
    if response.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("下载失败（HTTP {}）", response.status().as_u16())
    }
}

async fn write_response(
    app: &AppHandle,
    part_path: &Path,
    response: Response,
    transfer: &mut DriveTransfer,
    offset: u64,
) -> Result<()> {
    let append = offset > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
        transfer.transferred_bytes = 0;
    }
    let mut file = options
        .open(part_path)
        .await
        .context("无法打开临时下载文件")?;
    let mut stream = response.bytes_stream();
    let mut transferred = if append { offset } else { 0 };
    let mut last_persisted = transferred;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("读取下载数据失败")?;
        file.write_all(&chunk)
            .await
            .context("写入下载文件失败")?;
        transferred += chunk.len() as u64;
        transfer.transferred_bytes = transferred;
        emit_progress(
            app,
            transfer,
            if append { "正在续传" } else { "正在下载" },
            transferred,
            transfer.total_bytes,
        );
        if transferred.saturating_sub(last_persisted) >= TRANSFER_PERSIST_INTERVAL {
            transfer.updated_at = Utc::now().to_rfc3339();
            transfer.message = Some(if append {
                "正在续传".to_string()
            } else {
                "正在下载".to_string()
            });
            storage::update_drive_transfer(app, transfer)?;
            last_persisted = transferred;
        }
    }
    file.flush().await.context("刷新下载文件失败")?;
    Ok(())
}

fn update_total_from_response(response: &Response, transfer: &mut DriveTransfer, offset: u64) {
    if let Some(total) = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range_total)
    {
        transfer.total_bytes = total;
    } else if let Some(length) = response.content_length() {
        transfer.total_bytes = offset.saturating_add(length);
    }
}

fn parse_content_range_start(value: &str) -> Option<u64> {
    let range = value.strip_prefix("bytes ")?.split('/').next()?;
    range.split('-').next()?.parse().ok()
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.parse().ok()
}

fn part_path_for(destination: &Path) -> PathBuf {
    PathBuf::from(format!("{}.part", destination.to_string_lossy()))
}

pub(super) async fn hash_file(path: &Path) -> Result<String> {
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

pub(super) fn emit_progress(
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

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut index = 0;
    while value >= 1024.0 && index + 1 < UNITS.len() {
        value /= 1024.0;
        index += 1;
    }
    if index == 0 {
        format!("{} {}", bytes, UNITS[index])
    } else {
        format!("{value:.1} {}", UNITS[index])
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_content_range_start, parse_content_range_total};

    #[test]
    fn parses_content_range_values() {
        assert_eq!(parse_content_range_start("bytes 10-19/100"), Some(10));
        assert_eq!(parse_content_range_start("bytes */100"), None);
        assert_eq!(parse_content_range_total("bytes 10-19/100"), Some(100));
        assert_eq!(parse_content_range_total("bytes */100"), Some(100));
        assert_eq!(parse_content_range_total("invalid"), None);
    }
}
