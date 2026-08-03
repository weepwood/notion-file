use super::{
    drive_context, ensure_unique_path, join_path, new_id, notion_index, parent_path,
    validate_logical_path, validate_name,
};
use crate::file_upload;
use crate::models::{
    DriveDownloadRequest, DriveNode, DriveTransfer, DriveTransferProgress, DriveUploadRequest,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;

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
    let destination = PathBuf::from(request.destination_path.trim());
    let now = Utc::now().to_rfc3339();
    let mut transfer = DriveTransfer {
        id: new_id("download"),
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

    let result: Result<()> = async {
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

        let mut url = notion_index::resolve_file_url(&context.http, &block_id).await?;
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
        if matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            url = notion_index::resolve_file_url(&context.http, &block_id).await?;
            response = client
                .get(&url)
                .send()
                .await
                .context("刷新地址后下载请求失败")?;
        }
        if !response.status().is_success() {
            anyhow::bail!("下载失败（HTTP {}）", response.status().as_u16());
        }

        if let Some(response_total) = response.content_length() {
            if response_total > 0 {
                transfer.total_bytes = response_total;
            }
        }

        let mut file = tokio::fs::File::create(&part_path)
            .await
            .context("无法创建临时下载文件")?;
        let mut stream = response.bytes_stream();
        let mut transferred = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("读取下载数据失败")?;
            file.write_all(&chunk)
                .await
                .context("写入下载文件失败")?;
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
                anyhow::bail!(
                    "下载文件校验失败，临时文件已保留：{}",
                    part_path.display()
                );
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
            if transfer.transferred_bytes == 0 {
                transfer.transferred_bytes = node.size;
            }
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
