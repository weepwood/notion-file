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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HASH_BUFFER_SIZE: usize = 1024 * 1024;
const TRANSFER_PERSIST_INTERVAL: u64 = 8 * 1024 * 1024;

const LIVE_PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
const SLOW_UPLOAD_THRESHOLD_BPS: f64 = 512.0 * 1024.0;

#[derive(Debug, Clone, Default)]
pub(super) struct ProgressDetails {
    pub stage_code: String,
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

#[derive(Debug, Clone, Default)]
pub(super) struct UploadDiagnostics {
    pub endpoint_url: Option<String>,
    pub endpoint_host: Option<String>,
    pub average_speed_bytes_per_second: f64,
    pub processing_ms: u64,
    pub network_ms: u64,
    pub remote_api_ms: u64,
    pub total_ms: u64,
    pub reused_remote_file: bool,
    pub diagnostic_hint: String,
}

#[derive(Debug)]
struct UploadProgressState {
    endpoint_url: Option<String>,
    endpoint_host: Option<String>,
    last_bytes: u64,
    last_sample: Instant,
    last_emit: Instant,
    current_speed: f64,
    average_speed: f64,
    current_part: Option<u64>,
    total_parts: Option<u64>,
}

#[derive(Clone)]
pub(super) struct UploadProgressReporter {
    app: AppHandle,
    transfer: DriveTransfer,
    operation_started: Instant,
    state: Arc<Mutex<UploadProgressState>>,
}

impl UploadProgressReporter {
    pub(super) fn new(
        app: &AppHandle,
        transfer: &DriveTransfer,
        operation_started: Instant,
    ) -> Self {
        Self {
            app: app.clone(),
            transfer: transfer.clone(),
            operation_started,
            state: Arc::new(Mutex::new(UploadProgressState {
                endpoint_url: None,
                endpoint_host: None,
                last_bytes: 0,
                last_sample: Instant::now(),
                last_emit: Instant::now() - LIVE_PROGRESS_INTERVAL,
                current_speed: 0.0,
                average_speed: 0.0,
                current_part: None,
                total_parts: None,
            })),
        }
    }

    pub(super) fn report(&self, event: file_upload::FileUploadProgress) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if event.bytes_sent < state.last_bytes {
            state.last_bytes = 0;
            state.last_sample = now;
        }
        let sample_elapsed = now.duration_since(state.last_sample).as_secs_f64();
        if sample_elapsed > 0.0 {
            state.current_speed =
                event.bytes_sent.saturating_sub(state.last_bytes) as f64 / sample_elapsed;
        }
        let upload_elapsed = (event.elapsed_ms as f64 / 1000.0).max(0.001);
        state.average_speed = event.bytes_sent as f64 / upload_elapsed;
        state.endpoint_url = Some(event.endpoint_url.clone());
        state.endpoint_host = event.endpoint_host.clone();
        state.current_part = Some(event.current_part);
        state.total_parts = Some(event.total_parts);

        let should_emit = now.duration_since(state.last_emit) >= LIVE_PROGRESS_INTERVAL
            || event.bytes_sent >= event.total_bytes;
        if !should_emit {
            return;
        }
        state.last_bytes = event.bytes_sent;
        state.last_sample = now;
        state.last_emit = now;
        let current_speed = state.current_speed;
        let average_speed = state.average_speed;
        let hint = if event.elapsed_ms >= 3_000
            && average_speed > 0.0
            && average_speed < SLOW_UPLOAD_THRESHOLD_BPS
        {
            Some("当前网络上传速度低于 512 KiB/s，瓶颈更可能在网络链路".to_string())
        } else {
            None
        };
        drop(state);

        let stage = if event.total_parts > 1 {
            format!(
                "正在上传分片 {}/{} · {}",
                event.current_part,
                event.total_parts,
                event.endpoint_host.as_deref().unwrap_or("Notion")
            )
        } else {
            format!(
                "正在上传 · {}",
                event.endpoint_host.as_deref().unwrap_or("Notion")
            )
        };
        emit_progress_detailed(
            &self.app,
            &self.transfer,
            &stage,
            event.bytes_sent,
            event.total_bytes,
            ProgressDetails {
                stage_code: "uploading".to_string(),
                current_speed_bytes_per_second: current_speed,
                average_speed_bytes_per_second: average_speed,
                elapsed_ms: self.operation_started.elapsed().as_millis() as u64,
                stage_elapsed_ms: event.elapsed_ms,
                endpoint_url: Some(event.endpoint_url),
                endpoint_host: event.endpoint_host,
                current_part: Some(event.current_part),
                total_parts: Some(event.total_parts),
                diagnostic_hint: hint,
            },
        );
    }

    pub(super) fn snapshot(&self) -> (Option<String>, Option<String>, f64) {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state.endpoint_url.clone(),
            state.endpoint_host.clone(),
            state.average_speed,
        )
    }
}

pub(super) fn diagnose_upload(
    endpoint_url: Option<String>,
    endpoint_host: Option<String>,
    average_speed_bytes_per_second: f64,
    processing_ms: u64,
    network_ms: u64,
    remote_api_ms: u64,
    total_ms: u64,
    reused_remote_file: bool,
) -> UploadDiagnostics {
    let diagnostic_hint = if reused_remote_file {
        "命中 SHA-256 去重，没有重新传输文件内容；耗时主要来自本地校验和 Notion 索引写入"
            .to_string()
    } else if network_ms >= processing_ms.max(remote_api_ms)
        && average_speed_bytes_per_second > 0.0
        && average_speed_bytes_per_second < SLOW_UPLOAD_THRESHOLD_BPS
    {
        "瓶颈更可能是网络上传：平均速度低于 512 KiB/s".to_string()
    } else if processing_ms > network_ms && processing_ms > remote_api_ms && processing_ms >= 2_000 {
        "瓶颈更可能是本地磁盘读取或 SHA-256 计算".to_string()
    } else if remote_api_ms > network_ms && remote_api_ms >= 2_000 {
        "瓶颈更可能是 Notion API 响应、全局限流等待或远端索引写入".to_string()
    } else if network_ms >= processing_ms.max(remote_api_ms) {
        "耗时主要发生在文件上传阶段，网络速度处于可观察范围".to_string()
    } else {
        "未发现单一明显瓶颈，可结合各阶段耗时继续判断".to_string()
    };
    UploadDiagnostics {
        endpoint_url,
        endpoint_host,
        average_speed_bytes_per_second,
        processing_ms,
        network_ms,
        remote_api_ms,
        total_ms,
        reused_remote_file,
        diagnostic_hint,
    }
}

pub(super) fn upload_summary(diagnostics: &UploadDiagnostics) -> String {
    let endpoint = diagnostics
        .endpoint_url
        .as_deref()
        .or(diagnostics.endpoint_host.as_deref())
        .unwrap_or("复用已有远端附件");
    let speed = if diagnostics.reused_remote_file {
        "未发生文件网络传输".to_string()
    } else {
        format!(
            "平均 {}",
            format_rate(diagnostics.average_speed_bytes_per_second)
        )
    };
    format!(
        "上传完成 · {speed} · 总耗时 {} · SHA/磁盘 {} · 文件上传/API等待 {} · Notion索引 {} · 目标 {endpoint} · {}",
        format_duration(diagnostics.total_ms),
        format_duration(diagnostics.processing_ms),
        format_duration(diagnostics.network_ms),
        format_duration(diagnostics.remote_api_ms),
        diagnostics.diagnostic_hint
    )
}

pub(super) fn emit_completed_upload(
    app: &AppHandle,
    transfer: &DriveTransfer,
    stage: &str,
    total_bytes: u64,
    diagnostics: &UploadDiagnostics,
) {
    emit_progress_detailed(
        app,
        transfer,
        stage,
        total_bytes,
        total_bytes,
        ProgressDetails {
            stage_code: "completed".to_string(),
            current_speed_bytes_per_second: 0.0,
            average_speed_bytes_per_second: diagnostics.average_speed_bytes_per_second,
            elapsed_ms: diagnostics.total_ms,
            stage_elapsed_ms: diagnostics.remote_api_ms,
            endpoint_url: diagnostics.endpoint_url.clone(),
            endpoint_host: diagnostics.endpoint_host.clone(),
            diagnostic_hint: Some(diagnostics.diagnostic_hint.clone()),
            ..ProgressDetails::default()
        },
    );
}

pub(super) async fn upload_file(
    app: &AppHandle,
    request: DriveUploadRequest,
) -> Result<DriveNode> {
    if request.file_path.trim().is_empty() {
        anyhow::bail!("请选择需要上传到云盘的文件");
    }
    let operation_started = Instant::now();
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
    emit_progress_detailed(
        app,
        &transfer,
        "正在计算 SHA-256",
        0,
        size,
        ProgressDetails {
            stage_code: "hashing".to_string(),
            elapsed_ms: operation_started.elapsed().as_millis() as u64,
            ..ProgressDetails::default()
        },
    );

    let result: Result<(DriveNode, UploadDiagnostics)> = async {
        let hash_started = Instant::now();
        let transfer_for_hash = transfer.clone();
        let app_for_hash = app.clone();
        let operation_for_hash = operation_started;
        let sha256 = hash_file_with_progress(&canonical_path, move |processed, total| {
            let elapsed_ms = hash_started.elapsed().as_millis() as u64;
            let speed = processed as f64 / (elapsed_ms as f64 / 1000.0).max(0.001);
            emit_progress_detailed(
                &app_for_hash,
                &transfer_for_hash,
                "正在计算 SHA-256（本地处理）",
                processed,
                total,
                ProgressDetails {
                    stage_code: "hashing".to_string(),
                    current_speed_bytes_per_second: speed,
                    average_speed_bytes_per_second: speed,
                    elapsed_ms: operation_for_hash.elapsed().as_millis() as u64,
                    stage_elapsed_ms: elapsed_ms,
                    diagnostic_hint: Some("当前阶段只读取本地文件，不占用上传带宽".to_string()),
                    ..ProgressDetails::default()
                },
            );
        })
        .await?;
        let processing_ms = hash_started.elapsed().as_millis() as u64;
        let context = drive_context(app)?;
        let reusable_upload_id = storage::find_drive_file_by_hash(app, &sha256)?
            .and_then(|node| node.file_upload_id);
        let reporter = UploadProgressReporter::new(app, &transfer, operation_started);
        let network_started = Instant::now();
        let (file_upload_id, reused_remote_file) = if let Some(upload_id) = reusable_upload_id {
            emit_progress_detailed(
                app,
                &transfer,
                "检测到重复内容，复用远端文件",
                size,
                size,
                ProgressDetails {
                    stage_code: "deduplicated".to_string(),
                    elapsed_ms: operation_started.elapsed().as_millis() as u64,
                    diagnostic_hint: Some("已跳过文件网络上传，只写入新的 Notion 索引".to_string()),
                    ..ProgressDetails::default()
                },
            );
            (upload_id, true)
        } else {
            let report = reporter.clone();
            let upload_id = file_upload::upload_file_with_progress(
                &context.http,
                &canonical_path,
                &file_name,
                &mime_type,
                size,
                move |event| report.report(event),
            )
            .await?;
            (upload_id, false)
        };
        let network_ms = if reused_remote_file {
            0
        } else {
            network_started.elapsed().as_millis() as u64
        };
        let (endpoint_url, endpoint_host, average_speed) = reporter.snapshot();

        let remote_started = Instant::now();
        emit_progress_detailed(
            app,
            &transfer,
            "正在写入 Notion 文件索引",
            size,
            size,
            ProgressDetails {
                stage_code: "notion_index".to_string(),
                average_speed_bytes_per_second: average_speed,
                elapsed_ms: operation_started.elapsed().as_millis() as u64,
                endpoint_url: endpoint_url.clone(),
                endpoint_host: endpoint_host.clone(),
                diagnostic_hint: Some("文件内容已传输完成，当前等待 Notion 页面与区块写入".to_string()),
                ..ProgressDetails::default()
            },
        );
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
        if let Err(error) = notion_index::patch_remote_block_id(&context.http, &page_id, &block_id).await {
            let _ = notion_index::trash_remote_page(&context.http, &page_id).await;
            return Err(error)
                .context("写入云盘远端索引失败，未完成的文件页面已移入回收站");
        }
        storage::insert_drive_node(app, &node)?;
        version_store::ensure_current_version(app, &node)?;
        let remote_api_ms = remote_started.elapsed().as_millis() as u64;
        let diagnostics = diagnose_upload(
            endpoint_url,
            endpoint_host,
            average_speed,
            processing_ms,
            network_ms,
            remote_api_ms,
            operation_started.elapsed().as_millis() as u64,
            reused_remote_file,
        );
        Ok((node, diagnostics))
    }
    .await;

    match result {
        Ok((node, diagnostics)) => {
            transfer.status = "completed".to_string();
            transfer.transferred_bytes = size;
            transfer.message = Some(upload_summary(&diagnostics));
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_completed_upload(app, &transfer, "上传完成", size, &diagnostics);
            Ok(node)
        }
        Err(error) => {
            transfer.status = "failed".to_string();
            transfer.message = Some(format!(
                "{error} · 已耗时 {}。可根据最后显示的阶段判断是本地处理、网络上传还是 Notion API 写入失败",
                format_duration(operation_started.elapsed().as_millis() as u64)
            ));
            transfer.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer)?;
            emit_progress_detailed(
                app,
                &transfer,
                "上传失败",
                transfer.transferred_bytes,
                size,
                ProgressDetails {
                    stage_code: "failed".to_string(),
                    elapsed_ms: operation_started.elapsed().as_millis() as u64,
                    diagnostic_hint: Some(error.to_string()),
                    ..ProgressDetails::default()
                },
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
    hash_file_with_progress(path, |_, _| {}).await
}

pub(super) async fn hash_file_with_progress<F>(path: &Path, mut on_progress: F) -> Result<String>
where
    F: FnMut(u64, u64),
{
    let total = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("无法读取文件大小：{}", path.display()))?
        .len();
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("无法打开文件计算校验值：{}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];
    let mut processed = 0_u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        processed += read as u64;
        on_progress(processed, total);
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
    emit_progress_detailed(
        app,
        transfer,
        stage,
        transferred_bytes,
        total_bytes,
        ProgressDetails {
            stage_code: transfer.direction.clone(),
            ..ProgressDetails::default()
        },
    );
}

pub(super) fn emit_progress_detailed(
    app: &AppHandle,
    transfer: &DriveTransfer,
    stage: &str,
    transferred_bytes: u64,
    total_bytes: u64,
    details: ProgressDetails,
) {
    let _ = app.emit(
        "drive-transfer-progress",
        DriveTransferProgress {
            transfer_id: transfer.id.clone(),
            node_id: transfer.node_id.clone(),
            direction: transfer.direction.clone(),
            file_name: transfer.file_name.clone(),
            stage: stage.to_string(),
            stage_code: details.stage_code,
            transferred_bytes,
            total_bytes,
            current_speed_bytes_per_second: details.current_speed_bytes_per_second,
            average_speed_bytes_per_second: details.average_speed_bytes_per_second,
            elapsed_ms: details.elapsed_ms,
            stage_elapsed_ms: details.stage_elapsed_ms,
            endpoint_url: details.endpoint_url,
            endpoint_host: details.endpoint_host,
            current_part: details.current_part,
            total_parts: details.total_parts,
            diagnostic_hint: details.diagnostic_hint,
        },
    );
}

pub(super) fn format_duration(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        return format!("{milliseconds} ms");
    }
    let seconds = milliseconds as f64 / 1_000.0;
    if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{:.1} min", seconds / 60.0)
    }
}

pub(super) fn format_rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", format_bytes(bytes_per_second as u64))
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
