use crate::models::{SingleUploadRequest, UploadRecord};
use crate::notion::{
    divider_block, file_block, heading_block, metadata_callout, text_blocks, CreatedPage,
    NotionClient,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml",
    "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt",
    "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php", "swift",
    "vue", "svelte", "ini", "conf", "env", "csv",
];
const MAX_INLINE_TEXT_SIZE: u64 = 1024 * 1024;
const MAX_UPLOAD_SIZE: u64 = 20 * 1024 * 1024;

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
    let sha256 = if size <= MAX_UPLOAD_SIZE {
        let bytes = tokio::fs::read(&file_path)
            .await
            .context("无法读取所选文件内容")?;
        hex::encode(Sha256::digest(&bytes))
    } else {
        String::new()
    };
    let uploaded_at = Utc::now().to_rfc3339();
    let record_id = format!("upload-{}", Utc::now().timestamp_millis());

    let result = if size > MAX_UPLOAD_SIZE {
        Err(anyhow::anyhow!(
            "文件超过 20 MiB，当前版本暂不支持分片上传"
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
        Ok(page) => UploadRecord {
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
            message: Some("文件已上传并写入 Notion 页面".to_string()),
        },
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
    let notion = NotionClient::new(storage::load_token()?)?;
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
        let upload_id = notion.upload_file(path, mime_type).await?;
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
    use super::parent_page_id;

    #[test]
    fn trims_optional_parent_page_id() {
        assert_eq!(parent_page_id("  page-id  "), Some("page-id"));
        assert_eq!(parent_page_id("  "), None);
    }
}
