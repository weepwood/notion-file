use crate::models::{SingleUploadRequest, UploadRecord};
use crate::notion::{
    divider_block, file_block, heading_block, metadata_callout, text_blocks, CreatedPage,
    NotionClient,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::{multipart, Client, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tokio::io::AsyncReadExt;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2026-03-11";
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml",
    "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt",
    "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php", "swift",
    "vue", "svelte", "ini", "conf", "env", "csv",
];
const MAX_INLINE_TEXT_SIZE: u64 = 1024 * 1024;
const MAX_SINGLE_PART_SIZE: u64 = 20 * 1024 * 1024;
const MULTI_PART_SIZE: u64 = 10 * 1024 * 1024;
const MAX_NOTION_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 1024 * 1024;

pub async fn upload(app: &AppHandle, request: SingleUploadRequest) -> Result<UploadRecord> {
    if request.file_path.trim().is_empty() {
        anyhow::bail!("请先选择一个本地文件");
    }

    let requested_path = PathBuf::from(request.file_path.trim());
    let metadata = tokio::fs::metadata(&requested_path)
        .await
        .context("无法读取所选文件")?;
    if !metadata.is_file() {
        anyhow::bail!("所选路径不是文件");
    }

    let file_path = std::fs::canonicalize(&requested_path).unwrap_or(requested_path);
    let file_name = file_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("未命名文件")
        .to_string();
    let mime_type = mime_guess::from_path(&file_path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let size = metadata.len();
    let sha256 = if size <= MAX_NOTION_FILE_SIZE {
        hash_file(&file_path).await?
    } else {
        String::new()
    };
    let uploaded_at = Utc::now().to_rfc3339();
    let record_id = format!("upload-{}", Utc::now().timestamp_millis());

    let result = if size > MAX_NOTION_FILE_SIZE {
        Err(anyhow::anyhow!(
            "文件超过 Notion 付费工作区允许的 5 GiB 上限"
        ))
    } else {
        upload_to_notion(
            &file_path,
            &file_name,
            &mime_type,
            size,
            &sha256,
            &request.root_page_id,
        )
        .await
    };

    let record = match result {
        Ok(page) => {
            let message = if size > MAX_SINGLE_PART_SIZE {
                format!(
                    "文件已通过 {} 个分片上传并写入 Notion 页面",
                    part_count(size)
                )
            } else {
                "文件已上传并写入 Notion 页面".to_string()
            };
            UploadRecord {
                id: record_id,
                file_path: file_path.to_string_lossy().to_string(),
                file_name,
                size,
                mime_type,
                sha256,
                uploaded_at,
                status: "success".to_string(),
                page_id: Some(page.id),
                page_url: page.url,
                message: Some(message),
            }
        }
        Err(error) => UploadRecord {
            id: record_id,
            file_path: file_path.to_string_lossy().to_string(),
            file_name,
            size,
            mime_type,
            sha256,
            uploaded_at,
            status: "failed".to_string(),
            page_id: None,
            page_url: None,
            message: Some(error.to_string()),
        },
    };

    storage::append_upload_record(app, record.clone())?;
    Ok(record)
}

async fn upload_to_notion(
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    sha256: &str,
    root_page_id: &str,
) -> Result<CreatedPage> {
    let token = storage::load_token()?;
    let notion = NotionClient::new(token.clone())?;
    if !root_page_id.trim().is_empty() {
        notion
            .get_page_title(root_page_id)
            .await
            .context("无法访问指定的 Notion 父页面")?;
    }

    let page = notion
        .create_document_page(parent_page_id(root_page_id), file_name)
        .await
        .map_err(|error| create_page_error(root_page_id, error))?;

    let write_result = async {
        let upload_id = upload_file(&token, path, file_name, mime_type, size).await?;
        let mut blocks = vec![
            metadata_callout(&path.to_string_lossy(), mime_type, size, sha256),
            file_block(&upload_id, mime_type),
        ];

        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if TEXT_EXTENSIONS.contains(&extension.as_str()) && size <= MAX_INLINE_TEXT_SIZE {
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                blocks.push(divider_block());
                blocks.push(heading_block("内容预览"));
                blocks.extend(text_blocks(&content, &extension));
            }
        }

        notion.append_blocks(&page.id, blocks).await
    }
    .await;

    if let Err(error) = write_result {
        let _ = notion.archive_page(&page.id).await;
        return Err(error).context("单文件上传失败，已将未完成页面移入回收站");
    }

    Ok(page)
}

async fn upload_file(
    token: &str,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
) -> Result<String> {
    let client = Client::builder()
        .user_agent("notion-file/0.3.0")
        .build()
        .context("无法初始化文件上传客户端")?;

    if size <= MAX_SINGLE_PART_SIZE {
        upload_single_part(&client, token, path, file_name, mime_type).await
    } else {
        upload_multi_part(&client, token, path, file_name, mime_type, size).await
    }
}

async fn upload_single_part(
    client: &Client,
    token: &str,
    path: &Path,
    file_name: &str,
    mime_type: &str,
) -> Result<String> {
    let created = create_upload(
        client,
        token,
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
        .context("无法读取待上传文件")?;
    let part = multipart::Part::bytes(bytes)
        .file_name(file_name.to_string())
        .mime_str(mime_type)?;
    let uploaded = parse_response(
        client
            .post(upload_url)
            .bearer_auth(token)
            .header("Notion-Version", NOTION_VERSION)
            .multipart(multipart::Form::new().part("file", part))
            .send()
            .await?,
    )
    .await?;

    ensure_uploaded(&uploaded)?;
    Ok(upload_id)
}

async fn upload_multi_part(
    client: &Client,
    token: &str,
    path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
) -> Result<String> {
    let number_of_parts = part_count(size);
    let created = create_upload(
        client,
        token,
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
        .context("无法打开待分片上传文件")?;
    let mut remaining = size;

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个分片失败"))?;

        let part = multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(mime_type)?;
        parse_response(
            client
                .post(&upload_url)
                .bearer_auth(token)
                .header("Notion-Version", NOTION_VERSION)
                .multipart(
                    multipart::Form::new()
                        .text("part_number", part_number.to_string())
                        .part("file", part),
                )
                .send()
                .await
                .with_context(|| {
                    format!("发送第 {part_number}/{number_of_parts} 个分片失败")
                })?,
        )
        .await
        .with_context(|| format!("第 {part_number}/{number_of_parts} 个分片被 Notion 拒绝"))?;

        remaining -= current_size as u64;
    }

    let completed = parse_response(
        client
            .post(complete_url)
            .bearer_auth(token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Accept", "application/json")
            .send()
            .await
            .context("完成分片上传请求失败")?,
    )
    .await
    .context("Notion 无法完成分片上传")?;

    ensure_uploaded(&completed)?;
    Ok(upload_id)
}

async fn create_upload(client: &Client, token: &str, body: Value) -> Result<Value> {
    parse_response(
        client
            .post(format!("{NOTION_BASE_URL}/file_uploads"))
            .bearer_auth(token)
            .header("Notion-Version", NOTION_VERSION)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .context("创建 Notion 文件上传任务失败")?,
    )
    .await
}

async fn parse_response(response: Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or(text);
        anyhow::bail!("Notion 文件上传请求失败（{}）：{}", status.as_u16(), message);
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).context("Notion 文件上传接口返回了无效 JSON")
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

fn part_count(size: u64) -> u64 {
    size.div_ceil(MULTI_PART_SIZE)
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .context("无法打开文件以计算 SHA-256")?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .context("计算 SHA-256 时读取文件失败")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

fn parent_page_id(value: &str) -> Option<&str> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.trim())
    }
}

fn create_page_error(root_page_id: &str, error: anyhow::Error) -> anyhow::Error {
    if root_page_id.trim().is_empty() {
        anyhow::anyhow!(
            "无法直接在 Notion 工作区创建文件页面。当前 Token 很可能是内部 Integration Token；请填写一个已共享给该 Integration 的父页面 ID。原始错误：{error}"
        )
    } else {
        anyhow::anyhow!("无法在指定父页面下创建文件页面：{error}")
    }
}

#[cfg(test)]
mod tests {
    use super::{parent_page_id, part_count, MAX_SINGLE_PART_SIZE, MULTI_PART_SIZE};

    #[test]
    fn trims_optional_parent_page_id() {
        assert_eq!(parent_page_id("  page-id  "), Some("page-id"));
        assert_eq!(parent_page_id("  "), None);
    }

    #[test]
    fn calculates_multi_part_count() {
        assert_eq!(part_count(MAX_SINGLE_PART_SIZE + 1), 3);
        assert_eq!(part_count(MULTI_PART_SIZE * 3), 3);
        assert_eq!(part_count(MULTI_PART_SIZE * 3 + 1), 4);
    }
}
