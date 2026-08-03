use crate::notion_request::{parse_json_response, NotionHttp};
use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::Method;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::AsyncReadExt;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
pub(crate) const MAX_SINGLE_PART_SIZE: u64 = 20 * 1024 * 1024;
pub(crate) const MULTI_PART_SIZE: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_NOTION_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
const MULTI_PART_CONCURRENCY: usize = 2;

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

pub(crate) async fn upload_file<F>(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    mut on_part_uploaded: F,
) -> Result<String>
where
    F: FnMut(u64, u64),
{
    if size > MAX_NOTION_FILE_SIZE {
        anyhow::bail!(
            "文件“{file_name}”超过 Notion 的 5 GiB 单文件对象上限，无法作为文件夹附件上传"
        );
    }

    if size <= MAX_SINGLE_PART_SIZE {
        upload_single_part(http, path, file_name, mime_type).await
    } else {
        upload_multi_part(
            http,
            path,
            file_name,
            mime_type,
            size,
            &mut on_part_uploaded,
        )
        .await
    }
}

pub(crate) async fn upload_file_with_progress<F>(
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
    let progress_by_part = Arc::new(Mutex::new(vec![0_u64; number_of_parts as usize]));
    let mut uploads = FuturesUnordered::new();
    let mut remaining = size;
    let started = Instant::now();

    callback(FileUploadProgress {
        endpoint_url: endpoint_url.clone(),
        endpoint_host: endpoint_host.clone(),
        bytes_sent: 0,
        total_bytes: size,
        current_part: 1,
        total_parts: number_of_parts,
        elapsed_ms: 0,
    });

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个 API 分片失败"))?;
        remaining -= current_size as u64;

        let upload_http = http.clone();
        let part_upload_url = raw_upload_url.clone();
        let part_file_name = file_name.to_string();
        let part_mime_type = mime_type.to_string();
        let progress_callback = callback.clone();
        let progress_url = endpoint_url.clone();
        let progress_host = endpoint_host.clone();
        let progress_state = progress_by_part.clone();

        uploads.push(async move {
            emit_multi_part_progress(
                &progress_state,
                &progress_callback,
                &progress_url,
                &progress_host,
                part_number,
                0,
                number_of_parts,
                size,
                started,
            );

            let callback_for_body = progress_callback.clone();
            let url_for_body = progress_url.clone();
            let host_for_body = progress_host.clone();
            let state_for_body = progress_state.clone();
            let response = upload_http
                .send_multipart_with_progress(
                    part_upload_url,
                    &part_file_name,
                    &part_mime_type,
                    bytes,
                    Some(part_number),
                    move |part_sent| {
                        emit_multi_part_progress(
                            &state_for_body,
                            &callback_for_body,
                            &url_for_body,
                            &host_for_body,
                            part_number,
                            part_sent,
                            number_of_parts,
                            size,
                            started,
                        );
                    },
                )
                .await
                .with_context(|| {
                    format!("发送第 {part_number}/{number_of_parts} 个 API 分片失败")
                })?;

            parse_json_response(response, "Notion 文件分片上传请求")
                .await
                .with_context(|| {
                    format!("第 {part_number}/{number_of_parts} 个 API 分片被 Notion 拒绝")
                })?;

            emit_multi_part_progress(
                &progress_state,
                &progress_callback,
                &progress_url,
                &progress_host,
                part_number,
                current_size as u64,
                number_of_parts,
                size,
                started,
            );
            Ok::<u64, anyhow::Error>(part_number)
        });

        if uploads.len() >= MULTI_PART_CONCURRENCY {
            uploads
                .next()
                .await
                .context("并发分片上传任务意外结束")??;
        }
    }

    while let Some(result) = uploads.next().await {
        result?;
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

fn emit_multi_part_progress(
    progress_by_part: &Mutex<Vec<u64>>,
    callback: &Arc<dyn Fn(FileUploadProgress) + Send + Sync>,
    endpoint_url: &str,
    endpoint_host: &Option<String>,
    part_number: u64,
    part_sent: u64,
    number_of_parts: u64,
    total_size: u64,
    started: Instant,
) {
    let bytes_sent =
        aggregate_part_progress(progress_by_part, part_number, part_sent, total_size);
    callback(FileUploadProgress {
        endpoint_url: endpoint_url.to_string(),
        endpoint_host: endpoint_host.clone(),
        bytes_sent,
        total_bytes: total_size,
        current_part: part_number,
        total_parts: number_of_parts,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
}

fn aggregate_part_progress(
    progress_by_part: &Mutex<Vec<u64>>,
    part_number: u64,
    part_sent: u64,
    total_size: u64,
) -> u64 {
    let mut progress = progress_by_part
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(value) = progress.get_mut(part_number.saturating_sub(1) as usize) {
        *value = (*value).max(part_sent);
    }
    progress.iter().copied().sum::<u64>().min(total_size)
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

async fn upload_single_part(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
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
    let upload_url = upload_url(&created, &upload_id);

    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("无法读取待上传文件“{file_name}”"))?;
    let uploaded = parse_json_response(
        http.send_multipart(upload_url, file_name, mime_type, bytes, None)
            .await
            .with_context(|| format!("上传文件“{file_name}”失败"))?,
        "Notion 文件上传请求",
    )
    .await?;

    ensure_uploaded(&uploaded)?;
    Ok(upload_id)
}

async fn upload_multi_part<F>(
    http: &NotionHttp,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    on_part_uploaded: &mut F,
) -> Result<String>
where
    F: FnMut(u64, u64),
{
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
    let upload_url = upload_url(&created, &upload_id);
    let complete_url = created
        .get("complete_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/complete"));

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("无法打开待分片上传文件“{file_name}”"))?;
    let mut uploads = FuturesUnordered::new();
    let mut remaining = size;
    let mut completed_parts = 0_u64;

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个 API 分片失败"))?;
        remaining -= current_size as u64;

        let upload_http = http.clone();
        let part_upload_url = upload_url.clone();
        let part_file_name = file_name.to_string();
        let part_mime_type = mime_type.to_string();
        uploads.push(async move {
            let response = upload_http
                .send_multipart(
                    part_upload_url,
                    &part_file_name,
                    &part_mime_type,
                    bytes,
                    Some(part_number),
                )
                .await
                .with_context(|| {
                    format!("发送第 {part_number}/{number_of_parts} 个 API 分片失败")
                })?;
            parse_json_response(response, "Notion 文件分片上传请求")
                .await
                .with_context(|| {
                    format!("第 {part_number}/{number_of_parts} 个 API 分片被 Notion 拒绝")
                })?;
            Ok::<u64, anyhow::Error>(part_number)
        });

        if uploads.len() >= MULTI_PART_CONCURRENCY {
            uploads
                .next()
                .await
                .context("并发分片上传任务意外结束")??;
            completed_parts += 1;
            on_part_uploaded(completed_parts, number_of_parts);
        }
    }

    while let Some(result) = uploads.next().await {
        result?;
        completed_parts += 1;
        on_part_uploaded(completed_parts, number_of_parts);
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
    Ok(upload_id)
}

async fn create_upload(http: &NotionHttp, body: Value) -> Result<Value> {
    parse_json_response(
        http.request(Method::POST, format!("{NOTION_BASE_URL}/file_uploads"))
            .json(&body)
            .send()
            .await
            .context("创建 Notion 文件上传任务失败")?,
        "创建 Notion 文件上传任务",
    )
    .await
}

fn upload_id(value: &Value) -> Result<&str> {
    value
        .get("id")
        .and_then(Value::as_str)
        .context("Notion 未返回 file_upload ID")
}

fn upload_url(value: &Value, upload_id: &str) -> String {
    value
        .get("upload_url")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{NOTION_BASE_URL}/file_uploads/{upload_id}/send"))
}

fn ensure_uploaded(value: &Value) -> Result<()> {
    match value.get("status").and_then(Value::as_str) {
        Some("uploaded") => Ok(()),
        Some(status) => anyhow::bail!("文件上传完成后的状态为 {status}，不是 uploaded"),
        None => anyhow::bail!("Notion 未返回文件上传状态"),
    }
}

pub(crate) fn part_count(size: u64) -> u64 {
    size.div_ceil(MULTI_PART_SIZE)
}

#[cfg(test)]
mod tests {
    use super::{
        aggregate_part_progress, part_count, MAX_NOTION_FILE_SIZE, MAX_SINGLE_PART_SIZE,
        MULTI_PART_CONCURRENCY, MULTI_PART_SIZE,
    };
    use std::sync::Mutex;

    #[test]
    fn calculates_multi_part_count() {
        assert_eq!(part_count(MAX_SINGLE_PART_SIZE + 1), 3);
        assert_eq!(part_count(MULTI_PART_SIZE * 3), 3);
        assert_eq!(part_count(MULTI_PART_SIZE * 3 + 1), 4);
    }

    #[test]
    fn keeps_notion_object_limit_at_five_gib() {
        assert_eq!(MAX_NOTION_FILE_SIZE, 5 * 1024 * 1024 * 1024);
    }

    #[test]
    fn limits_parallel_part_uploads_to_two() {
        assert_eq!(MULTI_PART_CONCURRENCY, 2);
    }

    #[test]
    fn aggregates_parallel_progress_without_going_backwards() {
        let progress = Mutex::new(vec![0, 0, 0]);
        assert_eq!(aggregate_part_progress(&progress, 1, 5, 30), 5);
        assert_eq!(aggregate_part_progress(&progress, 2, 7, 30), 12);
        assert_eq!(aggregate_part_progress(&progress, 1, 3, 30), 12);
        assert_eq!(aggregate_part_progress(&progress, 1, 10, 30), 17);
        assert_eq!(aggregate_part_progress(&progress, 3, 20, 30), 30);
    }
}
