from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, content: str) -> None:
    (ROOT / path).write_text(content, encoding="utf-8")


def replace_once(text: str, old: str, new: str, path: str) -> str:
    if old not in text:
        raise RuntimeError(f"{path}: expected snippet not found: {old[:120]!r}")
    return text.replace(old, new, 1)


def replace_regex(text: str, pattern: str, replacement: str, path: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{path}: regex replacement count={count}: {pattern[:120]!r}")
    return updated


# 1. Rust progress model.
path = "src-tauri/src/models.rs"
text = read(path)
old = '''pub struct DriveTransferProgress {
    pub transfer_id: String,
    pub node_id: Option<String>,
    pub direction: String,
    pub file_name: String,
    pub stage: String,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
}'''
new = '''pub struct DriveTransferProgress {
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
}'''
text = replace_once(text, old, new, path)
write(path, text)


# 2. Streaming multipart request with body-level progress.
path = "src-tauri/src/notion_request.rs"
text = read(path)
text = replace_once(
    text,
    "use anyhow::{Context, Result};\n",
    "use anyhow::{Context, Result};\nuse futures_util::stream;\n",
    path,
)
text = replace_once(
    text,
    "use serde_json::Value;\n",
    "use serde_json::Value;\nuse std::io;\n",
    path,
)
text = replace_once(
    text,
    "const MAX_BACKOFF: Duration = Duration::from_secs(16);\n",
    "const MAX_BACKOFF: Duration = Duration::from_secs(16);\nconst UPLOAD_STREAM_CHUNK_SIZE: usize = 256 * 1024;\n",
    path,
)
marker = '''    pub async fn send_multipart(
        &self,
        url: impl Into<String>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        part_number: Option<u64>,
    ) -> Result<Response> {
        let url = url.into();
        let file_name = file_name.to_string();
        let mime_type = mime_type.to_string();
        let client = self.client.clone();
        let token = self.token.clone();

        execute_with_retry(
            move || {
                let file = multipart::Part::bytes(bytes.clone())
                    .file_name(file_name.clone())
                    .mime_str(&mime_type)?;
                let mut form = multipart::Form::new().part("file", file);
                if let Some(part_number) = part_number {
                    form = form.text("part_number", part_number.to_string());
                }
                Ok(client
                    .post(&url)
                    .bearer_auth(token.as_ref())
                    .header("Notion-Version", NOTION_VERSION)
                    .header("Accept", "application/json")
                    .multipart(form))
            },
            RetryPolicy::RateLimitOnly,
        )
        .await
    }
'''
addition = marker + '''
    pub async fn send_multipart_with_progress<F>(
        &self,
        url: impl Into<String>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        part_number: Option<u64>,
        on_progress: F,
    ) -> Result<Response>
    where
        F: Fn(u64) + Send + Sync + 'static,
    {
        let url = url.into();
        let file_name = file_name.to_string();
        let mime_type = mime_type.to_string();
        let client = self.client.clone();
        let token = self.token.clone();
        let bytes = Arc::new(bytes);
        let on_progress: Arc<dyn Fn(u64) + Send + Sync> = Arc::new(on_progress);

        execute_with_retry(
            move || {
                let stream_bytes = bytes.clone();
                let stream_progress = on_progress.clone();
                let content_length = stream_bytes.len() as u64;
                let body_stream = stream::unfold(0usize, move |offset| {
                    let bytes = stream_bytes.clone();
                    let on_progress = stream_progress.clone();
                    async move {
                        if offset >= bytes.len() {
                            return None;
                        }
                        let end = (offset + UPLOAD_STREAM_CHUNK_SIZE).min(bytes.len());
                        let chunk = bytes[offset..end].to_vec();
                        on_progress(end as u64);
                        Some((Ok::<Vec<u8>, io::Error>(chunk), end))
                    }
                });
                let body = reqwest::Body::wrap_stream(body_stream);
                let file = multipart::Part::stream_with_length(body, content_length)
                    .file_name(file_name.clone())
                    .mime_str(&mime_type)?;
                let mut form = multipart::Form::new().part("file", file);
                if let Some(part_number) = part_number {
                    form = form.text("part_number", part_number.to_string());
                }
                Ok(client
                    .post(&url)
                    .bearer_auth(token.as_ref())
                    .header("Notion-Version", NOTION_VERSION)
                    .header("Accept", "application/json")
                    .multipart(form))
            },
            RetryPolicy::RateLimitOnly,
        )
        .await
    }
'''
text = replace_once(text, marker, addition, path)
write(path, text)


# 3. Detailed Notion file-upload events.
path = "src-tauri/src/file_upload.rs"
text = read(path)
text = replace_once(
    text,
    "use std::path::Path;\n",
    "use std::path::Path;\nuse std::sync::Arc;\nuse std::time::Instant;\n",
    path,
)
insert_after = '''pub(crate) const MAX_NOTION_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
'''
addition = insert_after + '''
#[derive(Debug, Clone)]
pub(crate) struct FileUploadProgress {
    pub endpoint_url: String,
    pub endpoint_host: Option<String>,
    pub bytes_sent: u64,
    pub total_bytes: u64,
    pub current_part: u64,
    pub total_parts: u64,
    pub elapsed_ms: u64,
}
'''
text = replace_once(text, insert_after, addition, path)
marker = '''async fn upload_single_part(
'''
detailed = r'''pub(crate) async fn upload_file_with_progress<F>(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    on_progress: F,
) -> Result<String>
where
    F: Fn(FileUploadProgress) + Send + Sync + 'static,
{
    if size > MAX_NOTION_FILE_SIZE {
        anyhow::bail!(
            "文件“{file_name}”超过 Notion 的 5 GiB 单文件对象上限，无法上传"
        );
    }
    let callback: Arc<dyn Fn(FileUploadProgress) + Send + Sync> = Arc::new(on_progress);
    if size <= MAX_SINGLE_PART_SIZE {
        upload_single_part_with_progress(http, path, file_name, mime_type, size, callback).await
    } else {
        upload_multi_part_with_progress(http, path, file_name, mime_type, size, callback).await
    }
}

async fn upload_single_part_with_progress(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    callback: Arc<dyn Fn(FileUploadProgress) + Send + Sync>,
) -> Result<String> {
    let created = create_upload(
        http,
        json!({
            "mode": "single_part",
            "filename": file_name,
            "content_type": mime_type
        }),
    )
    .await?;
    let upload_id = upload_id(&created)?.to_string();
    let raw_upload_url = upload_url(&created, &upload_id);
    let endpoint_url = display_upload_url(&raw_upload_url);
    let endpoint_host = upload_host(&raw_upload_url);
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("无法读取待上传文件“{file_name}”"))?;
    let started = Instant::now();
    callback(FileUploadProgress {
        endpoint_url: endpoint_url.clone(),
        endpoint_host: endpoint_host.clone(),
        bytes_sent: 0,
        total_bytes: size,
        current_part: 1,
        total_parts: 1,
        elapsed_ms: 0,
    });
    let progress_callback = callback.clone();
    let progress_url = endpoint_url.clone();
    let progress_host = endpoint_host.clone();
    let response = http
        .send_multipart_with_progress(
            raw_upload_url,
            file_name,
            mime_type,
            bytes,
            None,
            move |sent| {
                progress_callback(FileUploadProgress {
                    endpoint_url: progress_url.clone(),
                    endpoint_host: progress_host.clone(),
                    bytes_sent: sent.min(size),
                    total_bytes: size,
                    current_part: 1,
                    total_parts: 1,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            },
        )
        .await
        .with_context(|| format!("上传文件“{file_name}”失败"))?;
    let uploaded = parse_json_response(response, "Notion 文件上传请求").await?;
    ensure_uploaded(&uploaded)?;
    callback(FileUploadProgress {
        endpoint_url,
        endpoint_host,
        bytes_sent: size,
        total_bytes: size,
        current_part: 1,
        total_parts: 1,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(upload_id)
}

async fn upload_multi_part_with_progress(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    callback: Arc<dyn Fn(FileUploadProgress) + Send + Sync>,
) -> Result<String> {
    let number_of_parts = part_count(size);
    let created = create_upload(
        http,
        json!({
            "mode": "multi_part",
            "number_of_parts": number_of_parts,
            "filename": file_name,
            "content_type": mime_type
        }),
    )
    .await?;
    let upload_id = upload_id(&created)?.to_string();
    let raw_upload_url = upload_url(&created, &upload_id);
    let endpoint_url = display_upload_url(&raw_upload_url);
    let endpoint_host = upload_host(&raw_upload_url);
    let complete_url = created
        .get("complete_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/complete"));
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("无法打开待分片上传文件“{file_name}”"))?;
    let mut remaining = size;
    let started = Instant::now();

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个 API 分片失败"))?;
        let base = (part_number - 1) * MULTI_PART_SIZE;
        callback(FileUploadProgress {
            endpoint_url: endpoint_url.clone(),
            endpoint_host: endpoint_host.clone(),
            bytes_sent: base.min(size),
            total_bytes: size,
            current_part: part_number,
            total_parts: number_of_parts,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
        let progress_callback = callback.clone();
        let progress_url = endpoint_url.clone();
        let progress_host = endpoint_host.clone();
        http.send_multipart_with_progress(
            raw_upload_url.clone(),
            file_name,
            mime_type,
            bytes,
            Some(part_number),
            move |part_sent| {
                progress_callback(FileUploadProgress {
                    endpoint_url: progress_url.clone(),
                    endpoint_host: progress_host.clone(),
                    bytes_sent: base.saturating_add(part_sent).min(size),
                    total_bytes: size,
                    current_part: part_number,
                    total_parts: number_of_parts,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            },
        )
        .await
        .with_context(|| format!("发送第 {part_number}/{number_of_parts} 个 API 分片失败"))?;
        remaining -= current_size as u64;
    }

    let completed = parse_json_response(
        http.request(Method::POST, complete_url)
            .send()
            .await
            .context("完成分片上传请求失败")?,
        "完成 Notion 文件分片上传",
    )
    .await
    .context("Notion 无法完成分片上传")?;
    ensure_uploaded(&completed)?;
    callback(FileUploadProgress {
        endpoint_url,
        endpoint_host,
        bytes_sent: size,
        total_bytes: size,
        current_part: number_of_parts,
        total_parts: number_of_parts,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    Ok(upload_id)
}

fn display_upload_url(raw: &str) -> String {
    reqwest::Url::parse(raw)
        .map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_else(|_| raw.to_string())
}

fn upload_host(raw: &str) -> Option<String> {
    reqwest::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
}

'''
text = replace_once(text, marker, detailed + marker, path)
write(path, text)


# 4. Replace drive upload path with instrumented implementation and helpers.
path = "src-tauri/src/drive/transfer.rs"
text = read(path)
text = replace_once(
    text,
    "use std::time::Duration;\n",
    "use std::sync::{Arc, Mutex};\nuse std::time::{Duration, Instant};\n",
    path,
)
constants = '''const HASH_BUFFER_SIZE: usize = 1024 * 1024;
const TRANSFER_PERSIST_INTERVAL: u64 = 8 * 1024 * 1024;
'''
helpers = constants + r'''
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
'''
text = replace_once(text, constants, helpers, path)

upload_function = r'''pub(super) async fn upload_file(
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
'''
text = replace_regex(
    text,
    r"pub\(super\) async fn upload_file\(.*?\n}\n\npub\(super\) async fn download_file",
    upload_function + "\npub(super) async fn download_file",
    path,
)

old_hash = r'''pub(super) async fn hash_file(path: &Path) -> Result<String> {
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
}'''
new_hash = r'''pub(super) async fn hash_file(path: &Path) -> Result<String> {
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
}'''
text = replace_once(text, old_hash, new_hash, path)
old_emit = r'''pub(super) fn emit_progress(
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
}'''
new_emit = r'''pub(super) fn emit_progress(
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
}'''
text = replace_once(text, old_emit, new_emit, path)
text = replace_once(
    text,
    '''fn format_bytes(bytes: u64) -> String {''',
    '''pub(super) fn format_duration(milliseconds: u64) -> String {
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

fn format_bytes(bytes: u64) -> String {''',
    path,
)
write(path, text)


# 5. Instrument version upload.
path = "src-tauri/src/drive/advanced.rs"
text = read(path)
text = replace_once(
    text,
    "use std::path::{Path, PathBuf};\n",
    "use std::path::{Path, PathBuf};\nuse std::time::Instant;\n",
    path,
)
version_function = r'''pub(super) async fn upload_version(
    app: &AppHandle,
    request: DriveVersionUploadRequest,
) -> Result<DriveNode> {
    let operation_started = Instant::now();
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
    transfer::emit_progress_detailed(
        app,
        &transfer_record,
        "正在计算新版本 SHA-256",
        0,
        size,
        transfer::ProgressDetails {
            stage_code: "hashing".to_string(),
            ..transfer::ProgressDetails::default()
        },
    );

    let result: Result<(DriveNode, transfer::UploadDiagnostics)> = async {
        version_store::ensure_current_version(app, &node)?;
        let hash_started = Instant::now();
        let app_for_hash = app.clone();
        let record_for_hash = transfer_record.clone();
        let operation_for_hash = operation_started;
        let sha256 = transfer::hash_file_with_progress(&canonical_path, move |processed, total| {
            let elapsed_ms = hash_started.elapsed().as_millis() as u64;
            let speed = processed as f64 / (elapsed_ms as f64 / 1_000.0).max(0.001);
            transfer::emit_progress_detailed(
                &app_for_hash,
                &record_for_hash,
                "正在计算新版本 SHA-256（本地处理）",
                processed,
                total,
                transfer::ProgressDetails {
                    stage_code: "hashing".to_string(),
                    current_speed_bytes_per_second: speed,
                    average_speed_bytes_per_second: speed,
                    elapsed_ms: operation_for_hash.elapsed().as_millis() as u64,
                    stage_elapsed_ms: elapsed_ms,
                    diagnostic_hint: Some("当前阶段只读取本地文件，不占用上传带宽".to_string()),
                    ..transfer::ProgressDetails::default()
                },
            );
        })
        .await?;
        let processing_ms = hash_started.elapsed().as_millis() as u64;
        let context = drive_context(app)?;
        let reusable_upload_id = storage::find_drive_file_by_hash(app, &sha256)?
            .and_then(|item| item.file_upload_id);
        let reporter = transfer::UploadProgressReporter::new(app, &transfer_record, operation_started);
        let network_started = Instant::now();
        let (file_upload_id, reused_remote_file) = if let Some(upload_id) = reusable_upload_id {
            transfer::emit_progress_detailed(
                app,
                &transfer_record,
                "检测到重复内容，复用远端附件",
                size,
                size,
                transfer::ProgressDetails {
                    stage_code: "deduplicated".to_string(),
                    elapsed_ms: operation_started.elapsed().as_millis() as u64,
                    diagnostic_hint: Some("已跳过文件网络上传，只追加新的版本区块".to_string()),
                    ..transfer::ProgressDetails::default()
                },
            );
            (upload_id, true)
        } else {
            let report = reporter.clone();
            let upload_id = file_upload::upload_file_with_progress(
                &context.http,
                &canonical_path,
                &upload_name,
                &mime_type,
                size,
                move |event| report.report(event),
            )
            .await?;
            (upload_id, false)
        };
        let network_ms = if reused_remote_file { 0 } else { network_started.elapsed().as_millis() as u64 };
        let (endpoint_url, endpoint_host, average_speed) = reporter.snapshot();
        let remote_started = Instant::now();
        transfer::emit_progress_detailed(
            app,
            &transfer_record,
            "正在写入 Notion 版本索引",
            size,
            size,
            transfer::ProgressDetails {
                stage_code: "notion_index".to_string(),
                average_speed_bytes_per_second: average_speed,
                elapsed_ms: operation_started.elapsed().as_millis() as u64,
                endpoint_url: endpoint_url.clone(),
                endpoint_host: endpoint_host.clone(),
                diagnostic_hint: Some("文件内容已上传，当前等待版本区块与远端索引更新".to_string()),
                ..transfer::ProgressDetails::default()
            },
        );
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
        let diagnostics = transfer::diagnose_upload(
            endpoint_url,
            endpoint_host,
            average_speed,
            processing_ms,
            network_ms,
            remote_started.elapsed().as_millis() as u64,
            operation_started.elapsed().as_millis() as u64,
            reused_remote_file,
        );
        Ok((node, diagnostics))
    }
    .await;

    match result {
        Ok((updated, diagnostics)) => {
            transfer_record.status = "completed".to_string();
            transfer_record.transferred_bytes = size;
            transfer_record.message = Some(format!(
                "版本 v{} · {}",
                updated.version,
                transfer::upload_summary(&diagnostics)
            ));
            transfer_record.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer_record)?;
            transfer::emit_completed_upload(
                app,
                &transfer_record,
                "新版本上传完成",
                size,
                &diagnostics,
            );
            Ok(updated)
        }
        Err(error) => {
            transfer_record.status = "failed".to_string();
            transfer_record.message = Some(format!(
                "{error} · 已耗时 {}",
                transfer::format_duration(operation_started.elapsed().as_millis() as u64)
            ));
            transfer_record.updated_at = Utc::now().to_rfc3339();
            storage::update_drive_transfer(app, &transfer_record)?;
            Err(error)
        }
    }
}
'''
text = replace_regex(
    text,
    r"pub\(super\) async fn upload_version\(.*?\n}\n\npub\(super\) async fn download_version",
    version_function + "\npub(super) async fn download_version",
    path,
)
write(path, text)


# 6. Frontend types and diagnostics banner.
path = "src/types.ts"
text = read(path)
old = '''export interface DriveTransferProgress {
  transferId: string;
  nodeId?: string;
  direction: "upload" | "download";
  fileName: string;
  stage: string;
  transferredBytes: number;
  totalBytes: number;
}'''
new = '''export interface DriveTransferProgress {
  transferId: string;
  nodeId?: string;
  direction: "upload" | "download";
  fileName: string;
  stage: string;
  stageCode: "hashing" | "uploading" | "notion_index" | "deduplicated" | "completed" | "failed" | "upload" | "download";
  transferredBytes: number;
  totalBytes: number;
  currentSpeedBytesPerSecond: number;
  averageSpeedBytesPerSecond: number;
  elapsedMs: number;
  stageElapsedMs: number;
  endpointUrl?: string;
  endpointHost?: string;
  currentPart?: number;
  totalParts?: number;
  diagnosticHint?: string;
}'''
text = replace_once(text, old, new, path)
write(path, text)

path = "src/App.tsx"
text = read(path)
text = replace_once(
    text,
    '''function formatDate(value: string): string {''',
    '''function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

function formatDuration(milliseconds: number): string {
  if (!milliseconds) return "0 ms";
  if (milliseconds < 1000) return `${Math.round(milliseconds)} ms`;
  const seconds = milliseconds / 1000;
  return seconds < 60 ? `${seconds.toFixed(1)} s` : `${(seconds / 60).toFixed(1)} min`;
}

function formatDate(value: string): string {''',
    path,
)
text = text.replace("个人云盘 · v0.6.0", "个人云盘 · v0.6.1")
old_banner = '''          {progress && busy && <div className="transfer-banner"><div>{progress.direction === "upload" ? <Upload size={18} /> : <Download size={18} />}<span><strong>{progress.fileName}</strong><small>{progress.stage}</small></span></div><div className="progress-meta">{formatBytes(progress.transferredBytes)} / {formatBytes(progress.totalBytes)}</div><div className="progress-track"><div style={{ width: `${progressPercent}%` }} /></div></div>}'''
new_banner = '''          {progress && busy && <div className="transfer-banner diagnostic-banner">
            <div className="diagnostic-title"><div>{progress.direction === "upload" ? <Upload size={18} /> : <Download size={18} />}<span><strong>{progress.fileName}</strong><small>{progress.stage}</small></span></div><div className="progress-meta">{formatBytes(progress.transferredBytes)} / {formatBytes(progress.totalBytes)}</div></div>
            <div className="progress-track"><div style={{ width: `${progressPercent}%` }} /></div>
            <div className="diagnostic-metrics">
              <div><span>{progress.stageCode === "hashing" ? "本地处理速度" : "当前网络速度"}</span><strong>{formatRate(progress.currentSpeedBytesPerSecond)}</strong></div>
              <div><span>{progress.stageCode === "hashing" ? "平均处理速度" : "平均上传速度"}</span><strong>{formatRate(progress.averageSpeedBytesPerSecond)}</strong></div>
              <div><span>当前阶段耗时</span><strong>{formatDuration(progress.stageElapsedMs)}</strong></div>
              <div><span>总耗时</span><strong>{formatDuration(progress.elapsedMs)}</strong></div>
              {progress.currentPart && progress.totalParts && <div><span>API 分片</span><strong>{progress.currentPart} / {progress.totalParts}</strong></div>}
            </div>
            {progress.endpointUrl && <div className="endpoint-row"><span>上传网址</span><code title={progress.endpointUrl}>{progress.endpointUrl}</code></div>}
            {progress.diagnosticHint && <div className="diagnostic-hint"><AlertCircle size={15} /><span>{progress.diagnosticHint}</span></div>}
          </div>}'''
text = replace_once(text, old_banner, new_banner, path)
text = text.replace(
    "下载失败会保留 .part 临时文件，可从已完成字节继续。",
    "上传记录会保留平均速度、各阶段耗时和目标网址；下载失败会保留 .part 临时文件。",
)
write(path, text)


# 7. Styles.
path = "src/advanced.css"
text = read(path)
styles = r'''

/* v0.6.1 upload diagnostics */
.diagnostic-banner {
  display: grid;
  gap: 10px;
  padding: 14px 16px;
}

.diagnostic-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

.diagnostic-title > div:first-child {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.diagnostic-title span {
  display: grid;
  gap: 2px;
  min-width: 0;
}

.diagnostic-title small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diagnostic-metrics {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
  gap: 8px;
}

.diagnostic-metrics > div {
  display: grid;
  gap: 3px;
  padding: 8px 10px;
  border: 1px solid var(--border, #e5e5e2);
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.55);
}

.diagnostic-metrics span,
.endpoint-row > span {
  color: var(--muted, #787774);
  font-size: 11px;
}

.diagnostic-metrics strong {
  font-size: 13px;
  font-variant-numeric: tabular-nums;
}

.endpoint-row {
  display: grid;
  grid-template-columns: 64px minmax(0, 1fr);
  align-items: center;
  gap: 8px;
}

.endpoint-row code {
  min-width: 0;
  overflow: hidden;
  padding: 6px 8px;
  border-radius: 6px;
  background: rgba(55, 53, 47, 0.06);
  color: #4b5563;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.diagnostic-hint {
  display: flex;
  align-items: flex-start;
  gap: 7px;
  color: #7c5c00;
  font-size: 12px;
  line-height: 1.45;
}

.diagnostic-hint svg {
  flex: 0 0 auto;
  margin-top: 1px;
}

@media (max-width: 760px) {
  .diagnostic-title {
    align-items: flex-start;
    flex-direction: column;
  }

  .endpoint-row {
    grid-template-columns: 1fr;
  }
}
'''
if "/* v0.6.1 upload diagnostics */" not in text:
    text += styles
write(path, text)


# 8. Version and docs.
for path in ["package.json", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json"]:
    text = read(path)
    text = text.replace('"version": "0.6.0"', '"version": "0.6.1"', 1)
    text = re.sub(r'(?m)^version = "0\.6\.0"$', 'version = "0.6.1"', text, count=1)
    write(path, text)

path = "README.md"
text = read(path)
section = r'''
## v0.6.1：上传速度与瓶颈诊断

上传文件和上传新版本时，客户端现在会显示：

- 当前网络上传速度与平均上传速度
- SHA-256 阶段的本地磁盘处理速度
- 实际文件上传 URL 与目标主机
- 当前 API 分片编号
- 当前阶段耗时与总耗时
- 文件上传完成后的瓶颈判断

上传正文按 256 KiB 流式发送，并约每 200 ms 刷新一次诊断数据。显示的 URL 会移除查询参数和片段，避免把未来可能出现的临时签名参数写入界面或传输历史。

传输中心会在完成记录中保留诊断摘要，将总耗时拆分为 SHA/磁盘处理、文件上传/API 等待和 Notion 页面/区块索引写入三部分。平均上传速度低于 512 KiB/s 时会提示网络链路可能是主要瓶颈；本地校验或 Notion API 阶段明显更慢时也会分别提示。

'''
if "## v0.6.1：上传速度与瓶颈诊断" not in text:
    anchor = "## v0.6.0：续传、批量下载与版本历史\n"
    text = replace_once(text, anchor, section + anchor, path)
write(path, text)

print("upload diagnostics patch applied")
