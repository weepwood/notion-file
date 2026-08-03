use crate::ffmpeg;
use crate::models::{SingleUploadRequest, UploadProgress, UploadRecord};
use crate::notion::{
    divider_block, heading_block, metadata_callout, text_blocks, CreatedPage, NotionClient,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::{multipart, Client, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;

const NOTION_BASE_URL: &str = "https://api.notion.com/v1";
const NOTION_VERSION: &str = "2026-03-11";
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml",
    "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt",
    "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php", "swift",
    "vue", "svelte", "ini", "conf", "env", "csv",
];
const VIDEO_EXTENSIONS: &[&str] = &[
    "amv", "asf", "wmv", "avi", "f4v", "flv", "gifv", "m4v", "mp4", "mkv", "webm", "mov",
    "qt", "mpeg", "mpg", "m2ts", "mts", "ts",
];
const MAX_INLINE_TEXT_SIZE: u64 = 1024 * 1024;
const MAX_SINGLE_PART_SIZE: u64 = 20 * 1024 * 1024;
const MULTI_PART_SIZE: u64 = 10 * 1024 * 1024;
const VIDEO_SPLIT_THRESHOLD_SIZE: u64 = 5_000_000_000;
const MAX_NOTION_FILE_SIZE: u64 = 5 * 1024 * 1024 * 1024;
const HASH_BUFFER_SIZE: usize = 1024 * 1024;

struct UploadAsset {
    path: PathBuf,
    file_name: String,
    mime_type: String,
    size: u64,
}

struct UploadOutcome {
    page: CreatedPage,
    segment_count: usize,
    used_ffmpeg: bool,
}

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
    let display_mode = normalize_display_mode(&request.display_mode).to_string();
    let is_video = is_video_file(&file_path, &mime_type);
    let should_split_video = is_video && size > VIDEO_SPLIT_THRESHOLD_SIZE;

    emit_progress(app, 0, 1, "正在计算文件校验值", &file_name);
    let sha256 = hash_file(&file_path).await?;
    let uploaded_at = Utc::now().to_rfc3339();
    let record_id = format!("upload-{}", Utc::now().timestamp_millis());

    let result = prepare_and_upload(
        app,
        &file_path,
        &file_name,
        &mime_type,
        size,
        &sha256,
        &request.root_page_id,
        &display_mode,
        is_video,
    )
    .await;

    let record = match result {
        Ok(outcome) => {
            let mode_label = if display_mode == "video" {
                "视频块"
            } else {
                "文件块"
            };
            let message = if outcome.used_ffmpeg {
                format!(
                    "视频已由 ffmpeg 切分为 {} 段，并以{mode_label}写入 Notion 页面",
                    outcome.segment_count
                )
            } else if size > MAX_SINGLE_PART_SIZE {
                format!(
                    "文件已通过 {} 个 API 分片上传，并以{mode_label}写入 Notion 页面",
                    part_count(size)
                )
            } else {
                format!("文件已上传，并以{mode_label}写入 Notion 页面")
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
                page_id: Some(outcome.page.id),
                page_url: outcome.page.url,
                message: Some(message),
                display_mode,
                segment_count: outcome.segment_count,
                used_ffmpeg: outcome.used_ffmpeg,
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
            display_mode,
            segment_count: 0,
            used_ffmpeg: should_split_video,
        },
    };

    storage::append_upload_record(app, record.clone())?;
    emit_progress(app, 1, 1, "上传任务结束", record.message.as_deref().unwrap_or(""));
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
async fn prepare_and_upload(
    app: &AppHandle,
    file_path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    sha256: &str,
    root_page_id: &str,
    display_mode: &str,
    is_video: bool,
) -> Result<UploadOutcome> {
    if display_mode == "video" && !is_video {
        anyhow::bail!("只有视频文件可以使用“视频块”保存模式");
    }

    if is_video && size > VIDEO_SPLIT_THRESHOLD_SIZE {
        return split_and_upload_video(
            app,
            file_path,
            file_name,
            mime_type,
            size,
            sha256,
            root_page_id,
            display_mode,
        )
        .await;
    }

    if size > MAX_NOTION_FILE_SIZE {
        anyhow::bail!("文件超过 Notion 的 5 GiB 单文件上限；当前仅视频可通过 ffmpeg 自动切分");
    }

    let asset = UploadAsset {
        path: file_path.to_path_buf(),
        file_name: file_name.to_string(),
        mime_type: mime_type.to_string(),
        size,
    };
    let page = upload_assets_to_notion(
        app,
        &[asset],
        file_path,
        file_name,
        mime_type,
        size,
        sha256,
        root_page_id,
        display_mode,
    )
    .await?;

    Ok(UploadOutcome {
        page,
        segment_count: 1,
        used_ffmpeg: false,
    })
}

#[allow(clippy::too_many_arguments)]
async fn split_and_upload_video(
    app: &AppHandle,
    file_path: &Path,
    file_name: &str,
    mime_type: &str,
    size: u64,
    sha256: &str,
    root_page_id: &str,
    display_mode: &str,
) -> Result<UploadOutcome> {
    let split = ffmpeg::split_video(app, file_path, size).await?;
    let mut assets = Vec::with_capacity(split.parts.len());

    for part in &split.parts {
        let part_metadata = tokio::fs::metadata(part)
            .await
            .context("无法读取视频分段信息")?;
        if part_metadata.len() > MAX_NOTION_FILE_SIZE {
            anyhow::bail!("视频切分结果仍有分段超过 5 GiB");
        }
        assets.push(UploadAsset {
            path: part.clone(),
            file_name: part
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("video-segment.mkv")
                .to_string(),
            mime_type: "video/x-matroska".to_string(),
            size: part_metadata.len(),
        });
    }

    let segment_count = assets.len();
    let page = upload_assets_to_notion(
        app,
        &assets,
        file_path,
        file_name,
        mime_type,
        size,
        sha256,
        root_page_id,
        display_mode,
    )
    .await?;

    Ok(UploadOutcome {
        page,
        segment_count,
        used_ffmpeg: true,
    })
}

#[allow(clippy::too_many_arguments)]
async fn upload_assets_to_notion(
    app: &AppHandle,
    assets: &[UploadAsset],
    original_path: &Path,
    page_title: &str,
    original_mime_type: &str,
    original_size: u64,
    sha256: &str,
    root_page_id: &str,
    display_mode: &str,
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
        .create_document_page(parent_page_id(root_page_id), page_title)
        .await
        .map_err(|error| create_page_error(root_page_id, error))?;

    let upload_client = Client::builder()
        .user_agent("notion-file/0.3.0")
        .build()
        .context("无法初始化文件上传客户端")?;

    let write_result = async {
        let mut blocks = vec![metadata_callout(
            &original_path.to_string_lossy(),
            original_mime_type,
            original_size,
            sha256,
        )];

        if assets.len() > 1 {
            blocks.push(divider_block());
        }

        for (index, asset) in assets.iter().enumerate() {
            emit_progress(
                app,
                index + 1,
                assets.len(),
                "正在上传文件到 Notion",
                &format!("{}（{}/{}）", asset.file_name, index + 1, assets.len()),
            );
            let upload_id = upload_asset(
                app,
                &upload_client,
                &token,
                asset,
                index + 1,
                assets.len(),
            )
            .await?;

            if assets.len() > 1 {
                blocks.push(heading_block(&format!(
                    "视频分段 {:02}/{}",
                    index + 1,
                    assets.len()
                )));
            }
            blocks.push(upload_display_block(
                &upload_id,
                &asset.mime_type,
                display_mode,
            ));
            if index + 1 < assets.len() {
                blocks.push(divider_block());
            }
        }

        if assets.len() == 1 {
            append_text_preview(&mut blocks, &assets[0]).await;
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

async fn append_text_preview(blocks: &mut Vec<Value>, asset: &UploadAsset) {
    let extension = asset
        .path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !TEXT_EXTENSIONS.contains(&extension.as_str()) || asset.size > MAX_INLINE_TEXT_SIZE {
        return;
    }
    if let Ok(content) = tokio::fs::read_to_string(&asset.path).await {
        blocks.push(divider_block());
        blocks.push(heading_block("内容预览"));
        blocks.extend(text_blocks(&content, &extension));
    }
}

async fn upload_asset(
    app: &AppHandle,
    client: &Client,
    token: &str,
    asset: &UploadAsset,
    asset_number: usize,
    asset_total: usize,
) -> Result<String> {
    if asset.size <= MAX_SINGLE_PART_SIZE {
        upload_single_part(client, token, asset).await
    } else {
        upload_multi_part(app, client, token, asset, asset_number, asset_total).await
    }
}

async fn upload_single_part(client: &Client, token: &str, asset: &UploadAsset) -> Result<String> {
    let created = create_upload(
        client,
        token,
        json!({
            "mode": "single_part",
            "filename": asset.file_name,
            "content_type": asset.mime_type
        }),
    )
    .await?;
    let upload_id = upload_id(&created)?.to_string();
    let upload_url = upload_url(&created, &upload_id);

    let bytes = tokio::fs::read(&asset.path)
        .await
        .context("无法读取待上传文件")?;
    let part = multipart::Part::bytes(bytes)
        .file_name(asset.file_name.clone())
        .mime_str(&asset.mime_type)?;
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
    app: &AppHandle,
    client: &Client,
    token: &str,
    asset: &UploadAsset,
    asset_number: usize,
    asset_total: usize,
) -> Result<String> {
    let number_of_parts = part_count(asset.size);
    let created = create_upload(
        client,
        token,
        json!({
            "mode": "multi_part",
            "number_of_parts": number_of_parts,
            "filename": asset.file_name,
            "content_type": asset.mime_type
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

    let mut file = tokio::fs::File::open(&asset.path)
        .await
        .context("无法打开待分片上传文件")?;
    let mut remaining = asset.size;

    for part_number in 1..=number_of_parts {
        let current_size = remaining.min(MULTI_PART_SIZE) as usize;
        let mut bytes = vec![0_u8; current_size];
        file.read_exact(&mut bytes)
            .await
            .with_context(|| format!("读取第 {part_number}/{number_of_parts} 个 API 分片失败"))?;

        let part = multipart::Part::bytes(bytes)
            .file_name(asset.file_name.clone())
            .mime_str(&asset.mime_type)?;
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
                    format!("发送第 {part_number}/{number_of_parts} 个 API 分片失败")
                })?,
        )
        .await
        .with_context(|| {
            format!("第 {part_number}/{number_of_parts} 个 API 分片被 Notion 拒绝")
        })?;

        remaining -= current_size as u64;
        emit_progress(
            app,
            part_number as usize,
            number_of_parts as usize,
            "正在分片上传到 Notion",
            &format!(
                "视频段 {asset_number}/{asset_total} · API 分片 {part_number}/{number_of_parts}"
            ),
        );
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

fn upload_display_block(upload_id: &str, mime_type: &str, display_mode: &str) -> Value {
    let block_type = if display_mode == "video" && mime_type.starts_with("video/") {
        "video"
    } else {
        "file"
    };
    let mut block = json!({ "object": "block", "type": block_type });
    block[block_type] = json!({
        "type": "file_upload",
        "file_upload": { "id": upload_id }
    });
    block
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

fn is_video_file(path: &Path, mime_type: &str) -> bool {
    if mime_type.starts_with("video/") || mime_type == "application/mp4" {
        return true;
    }
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| VIDEO_EXTENSIONS.contains(&value.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn normalize_display_mode(value: &str) -> &str {
    if value.trim().eq_ignore_ascii_case("video") {
        "video"
    } else {
        "file"
    }
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

fn emit_progress(app: &AppHandle, current: usize, total: usize, stage: &str, detail: &str) {
    let _ = app.emit(
        "upload-progress",
        UploadProgress {
            current,
            total,
            stage: stage.to_string(),
            detail: detail.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{
        is_video_file, normalize_display_mode, parent_page_id, part_count,
        VIDEO_SPLIT_THRESHOLD_SIZE, MAX_SINGLE_PART_SIZE, MULTI_PART_SIZE,
    };
    use std::path::Path;

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

    #[test]
    fn uses_decimal_five_gb_video_threshold() {
        assert_eq!(VIDEO_SPLIT_THRESHOLD_SIZE, 5_000_000_000);
    }

    #[test]
    fn defaults_to_file_mode() {
        assert_eq!(normalize_display_mode("video"), "video");
        assert_eq!(normalize_display_mode("anything"), "file");
    }

    #[test]
    fn detects_video_extensions() {
        assert!(is_video_file(Path::new("movie.mkv"), "application/octet-stream"));
        assert!(!is_video_file(Path::new("document.pdf"), "application/pdf"));
    }
}
