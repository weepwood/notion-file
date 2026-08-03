use super::{drive_context, new_id, notion_index, transfer, version_store};
use crate::file_upload;
use crate::models::{
    DriveBatchItemResult, DriveDownloadRequest, DriveFolderDownloadRequest,
    DriveFolderDownloadResult, DriveNode, DriveTransfer, DriveVersion,
    DriveVersionDownloadRequest, DriveVersionUploadRequest,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

pub(super) fn list_versions(app: &AppHandle, node_id: String) -> Result<Vec<DriveVersion>> {
    let node = storage::get_drive_node(app, &node_id)?;
    if node.node_type != "file" {
        anyhow::bail!("文件夹没有文件版本历史");
    }
    version_store::ensure_current_version(app, &node)?;
    version_store::list_versions(app, &node_id)
}

pub(super) async fn upload_version(
    app: &AppHandle,
    request: DriveVersionUploadRequest,
) -> Result<DriveNode> {
    let mut node = storage::get_drive_node(app, &request.node_id)?;
    if node.node_type != "file" || !node.is_active() {
        anyhow::bail!("只能为正常状态的文件上传新版本");
    }
    let requested_path = PathBuf::from(request.file_path.trim());
    let metadata = tokio::fs::metadata(&requested_path)
        .await
        .context("无法读取新版本文件")?;
    if !metadata.is_file() {
        anyhow::bail!("所选新版本路径不是文件");
    }
    let canonical_path = std::fs::canonicalize(&requested_path).unwrap_or(requested_path);
    let size = metadata.len();
    let mime_type = mime_guess::from_path(&canonical_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let upload_name = canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(&node.name)
        .to_string();
    let now = Utc::now().to_rfc3339();
    let mut transfer_record = DriveTransfer {
        id: new_id("version-upload"),
        node_id: Some(node.id.clone()),
        direction: "upload".to_string(),
        file_name: format!("{} · v{}", node.name, node.version + 1),
        local_path: Some(canonical_path.to_string_lossy().to_string()),
        status: "running".to_string(),
        total_bytes: size,
        transferred_bytes: 0,
        message: Some("正在计算新版本 SHA-256".to_string()),
        created_at: now.clone(),
        updated_at: now,
    };
    storage::append_drive_transfer(app, &transfer_record)?;
    transfer::emit_progress(
        app,
        &transfer_record,
        "正在计算新版本 SHA-256",
        0,
        size,
    );

    let result: Result<DriveNode> = async {
        version_store::ensure_current_version(app, &node)?;
        let sha256 = transfer::hash_file(&canonical_path).await?;
        let context = drive_context(app)?;
        let reusable_upload_id = storage::find_drive_file_by_hash(app, &sha256)?
            .and_then(|item| item.file_upload_id);
        let file_upload_id = if let Some(upload_id) = reusable_upload_id {
            transfer::emit_progress(
                app,
                &transfer_record,
                "检测到重复内容，复用远端附件",
                size,
                size,
            );
            upload_id
        } else {
            let app_handle = app.clone();
            let progress_record = transfer_record.clone();
            file_upload::upload_file(
                &context.http,
                &canonical_path,
                &upload_name,
                &mime_type,
                size,
                move |part, total| {
                    let transferred = (part * file_upload::MULTI_PART_SIZE).min(size);
                    transfer::emit_progress(
                        &app_handle,
                        &progress_record,
                        &format!("正在上传版本分片 {part}/{total}"),
                        transferred,
                        size,
                    );
                },
            )
            .await?
        };

        let block_id = notion_index::append_file_block(
            &context.http,
            &node.notion_page_id,
            &file_upload_id,
            &mime_type,
        )
        .await?;
        let next_version = node.version + 1;
        let modified_at = Utc::now().to_rfc3339();
        let version = DriveVersion {
            id: version_store::version_id(&node.id, next_version),
            node_id: node.id.clone(),
            version: next_version,
            size,
            sha256: sha256.clone(),
            mime_type: mime_type.clone(),
            file_upload_id: file_upload_id.clone(),
            notion_block_id: block_id.clone(),
            original_path: Some(canonical_path.to_string_lossy().to_string()),
            created_at: modified_at.clone(),
        };

        node.version = next_version;
        node.size = size;
        node.sha256 = Some(sha256);
        node.mime_type = Some(mime_type);
        node.file_upload_id = Some(file_upload_id);
        node.notion_block_id = Some(block_id);
        node.original_path = version.original_path.clone();
        node.modified_at = modified_at;
        notion_index::patch_remote_node(&context.http, &node).await?;
        storage::insert_drive_node(app, &node)?;
        version_store::upsert_version(app, &version)?;
        Ok(node)
    }
    .await;

    match result {
        Ok(updated) => {
            transfer_record.status = "completed".to_string();
            transfer_record.transferred_bytes = size;
            transfer_record.message = Some(format!("版本 v{} 上传完成", updated.version));
            transfer_record.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer_record)?;
            transfer::emit_progress(
                app,
                &transfer_record,
                "新版本上传完成",
                size,
                size,
            );
            Ok(updated)
        }
        Err(error) => {
            transfer_record.status = "failed".to_string();
            transfer_record.message = Some(error.to_string());
            transfer_record.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer_record)?;
            Err(error)
        }
    }
}

pub(super) async fn download_version(
    app: &AppHandle,
    request: DriveVersionDownloadRequest,
) -> Result<DriveTransfer> {
    let version = version_store::get_version(app, &request.version_id)?;
    transfer::download_version(app, &version, request.destination_path).await
}

pub(super) async fn retry_transfer(
    app: &AppHandle,
    transfer_id: String,
) -> Result<DriveTransfer> {
    let record = storage::list_drive_transfers(app)?
        .into_iter()
        .find(|item| item.id == transfer_id)
        .context("找不到需要续传的下载记录")?;
    if record.file_name.starts_with("版本 v") {
        anyhow::bail!("历史版本下载请从文件详情的版本列表重新发起，避免错误续传为当前版本");
    }
    transfer::retry_download(app, transfer_id).await
}

pub(super) async fn download_folder(
    app: &AppHandle,
    request: DriveFolderDownloadRequest,
) -> Result<DriveFolderDownloadResult> {
    if request.destination_directory.trim().is_empty() {
        anyhow::bail!("请选择文件夹下载目录");
    }
    let root = storage::get_drive_node(app, &request.folder_id)?;
    if !root.is_folder() || !root.is_active() {
        anyhow::bail!("只能下载正常状态的文件夹");
    }
    let root_prefix = format!("{}/", root.logical_path.trim_end_matches('/'));
    let destination_root = PathBuf::from(request.destination_directory.trim()).join(&root.name);
    tokio::fs::create_dir_all(&destination_root)
        .await
        .context("无法创建文件夹下载目录")?;

    let mut files: Vec<DriveNode> = storage::list_drive_subtree(app, &root.id)?
        .into_iter()
        .filter(|node| node.node_type == "file" && node.is_active())
        .collect();
    files.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));

    let mut items = Vec::with_capacity(files.len());
    let mut succeeded = 0;
    let mut failed = 0;
    for node in files {
        let relative = node
            .logical_path
            .strip_prefix(&root_prefix)
            .unwrap_or(node.name.as_str());
        let destination = join_virtual_relative(&destination_root, relative);
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("无法创建批量下载子目录")?;
        }
        let result = transfer::download_file(
            app,
            DriveDownloadRequest {
                node_id: node.id.clone(),
                destination_path: destination.to_string_lossy().to_string(),
            },
        )
        .await;
        match result {
            Ok(_) => {
                succeeded += 1;
                items.push(DriveBatchItemResult {
                    node_id: node.id,
                    logical_path: node.logical_path,
                    destination_path: destination.to_string_lossy().to_string(),
                    status: "completed".to_string(),
                    message: None,
                });
            }
            Err(error) => {
                failed += 1;
                items.push(DriveBatchItemResult {
                    node_id: node.id,
                    logical_path: node.logical_path,
                    destination_path: destination.to_string_lossy().to_string(),
                    status: "failed".to_string(),
                    message: Some(error.to_string()),
                });
            }
        }
    }

    Ok(DriveFolderDownloadResult {
        folder_id: root.id,
        destination_directory: destination_root.to_string_lossy().to_string(),
        total: items.len(),
        succeeded,
        failed,
        items,
    })
}

fn join_virtual_relative(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|component| !component.is_empty() && *component != "." && *component != "..")
        .fold(base.to_path_buf(), |path, component| path.join(component))
}

#[cfg(test)]
mod tests {
    use super::join_virtual_relative;
    use std::path::Path;

    #[test]
    fn converts_virtual_paths_to_local_paths() {
        let path = join_virtual_relative(Path::new("downloads"), "docs/a.txt");
        assert!(path.ends_with(Path::new("docs").join("a.txt")));
    }
}
