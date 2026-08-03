use crate::notion_request::{parse_json_response, NotionHttp};
use anyhow::{Context, Result};
use reqwest::Method;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncReadExt;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
pub(crate) const MAX_SINGLE_PART_SIZE: u64 = 20 * 1024 * 1024;
pub(crate) const MULTI_PART_SIZE: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_NOTION_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;

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
    let mut remaining = size;

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个 API 分片失败"))?;

        parse_json_response(
            http.send_multipart(
                upload_url.clone(),
                file_name,
                mime_type,
                bytes,
                Some(part_number),
            )
            .await
            .with_context(|| {
                format!("发送第 {part_number}/{number_of_parts} 个 API 分片失败")
            })?,
            "Notion 文件分片上传请求",
        )
        .await
        .with_context(|| {
            format!("第 {part_number}/{number_of_parts} 个 API 分片被 Notion 拒绝")
        })?;

        remaining -= current_size as u64;
        on_part_uploaded(part_number, number_of_parts);
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
    use super::{part_count, MAX_NOTION_FILE_SIZE, MAX_SINGLE_PART_SIZE, MULTI_PART_SIZE};

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
}