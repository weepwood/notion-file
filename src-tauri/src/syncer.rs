use crate::models::{
    SyncEntry, SyncItemResult, SyncProgress, SyncRequest, SyncResult, SyncState,
};
use crate::notion::{
    divider_block, file_block, folder_summary_callout, heading_block, metadata_callout,
    page_url_from_id, text_blocks, CreatedPage, NotionClient,
};
use crate::{scanner, storage};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tauri::{AppHandle, Emitter};

const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml",
    "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt",
    "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php", "swift",
    "vue", "svelte", "ini", "conf", "env", "csv",
];
const MAX_INLINE_TEXT_SIZE: u64 = 1024 * 1024;

pub async fn synchronize(app: &AppHandle, request: SyncRequest) -> Result<SyncResult> {
    if request.folder_path.trim().is_empty() {
        anyhow::bail!("请先选择本地文件夹");
    }

    let notion = NotionClient::new(storage::load_token()?)?;
    if !request.root_page_id.trim().is_empty() {
        notion
            .get_page_title(&request.root_page_id)
            .await
            .context("无法访问指定的 Notion 父页面")?;
    }

    let mut state = storage::load_state(app)?;
    let folder_key = Path::new(&request.folder_path)
        .to_string_lossy()
        .to_string();
    if state.folder_path != folder_key {
        state = SyncState {
            folder_path: folder_key.clone(),
            ..SyncState::default()
        };
    }

    let scan = scanner::scan(&request.folder_path, request.skip_hidden, &state)?;
    let document_title = folder_title(&request.folder_path);
    let started_at = Utc::now().to_rfc3339();

    let (page_id, page_url, created_document) =
        resolve_document_page(&notion, &request, &state, &document_title).await?;

    let created = scan.files.iter().filter(|file| file.status == "new").count();
    let updated = scan
        .files
        .iter()
        .filter(|file| file.status == "modified")
        .count();
    let unchanged = scan.unchanged_count;
    let archived = scan.deleted_count;

    if !created_document && scan.changed_count == 0 && scan.deleted_count == 0 {
        let items = scan
            .files
            .iter()
            .map(|file| SyncItemResult {
                relative_path: file.relative_path.clone(),
                status: "unchanged".into(),
                page_id: Some(page_id.clone()),
                message: None,
            })
            .collect();

        return Ok(SyncResult {
            started_at,
            finished_at: Utc::now().to_rfc3339(),
            document_title,
            page_id,
            page_url,
            created,
            updated,
            unchanged,
            archived,
            failed: 0,
            items,
        });
    }

    let mut blocks = vec![
        folder_summary_callout(
            &document_title,
            scan.files.len(),
            scan.total_bytes,
            &Utc::now().to_rfc3339(),
        ),
        divider_block(),
    ];
    let mut items = Vec::new();
    let mut failed = 0;

    for (index, file) in scan.files.iter().enumerate() {
        emit_progress(
            app,
            index + 1,
            scan.files.len(),
            &file.relative_path,
            "正在准备文档内容",
        );

        match build_file_section(&notion, file).await {
            Ok(mut section) => {
                blocks.append(&mut section);
                items.push(SyncItemResult {
                    relative_path: file.relative_path.clone(),
                    status: if file.status == "unchanged" {
                        "unchanged".into()
                    } else {
                        "synced".into()
                    },
                    page_id: Some(page_id.clone()),
                    message: None,
                });
            }
            Err(error) => {
                failed += 1;
                items.push(SyncItemResult {
                    relative_path: file.relative_path.clone(),
                    status: "failed".into(),
                    page_id: Some(page_id.clone()),
                    message: Some(error.to_string()),
                });
            }
        }
    }

    if failed > 0 {
        if created_document {
            let _ = notion.archive_page(&page_id).await;
        }
        return Ok(SyncResult {
            started_at,
            finished_at: Utc::now().to_rfc3339(),
            document_title,
            page_id,
            page_url,
            created: 0,
            updated: 0,
            unchanged,
            archived: 0,
            failed,
            items,
        });
    }

    emit_progress(
        app,
        scan.files.len(),
        scan.files.len(),
        &document_title,
        "正在写入 Notion 文档",
    );

    let (final_page_id, final_page_url) = if created_document {
        if let Err(error) = notion.append_blocks(&page_id, blocks).await {
            let _ = notion.archive_page(&page_id).await;
            return Err(error).context("写入新建的 Notion 文档失败，已将空白页面移入回收站");
        }
        (page_id, page_url)
    } else {
        replace_document_page(&notion, &request, &document_title, &page_id, blocks).await?
    };

    for item in &mut items {
        item.page_id = Some(final_page_id.clone());
    }

    let synced_at = Utc::now().to_rfc3339();
    let entries: HashMap<String, SyncEntry> = scan
        .files
        .iter()
        .map(|file| {
            (
                file.relative_path.clone(),
                SyncEntry {
                    page_id: final_page_id.clone(),
                    hash: file.hash.clone(),
                    synced_at: synced_at.clone(),
                    mime_type: file.mime_type.clone(),
                },
            )
        })
        .collect();

    state.folder_path = folder_key;
    state.document_page_id = Some(final_page_id.clone());
    state.document_page_url = final_page_url.clone();
    state.entries = entries;
    storage::save_state(app, &state)?;

    Ok(SyncResult {
        started_at,
        finished_at: Utc::now().to_rfc3339(),
        document_title,
        page_id: final_page_id,
        page_url: final_page_url,
        created,
        updated,
        unchanged,
        archived,
        failed: 0,
        items,
    })
}

async fn resolve_document_page(
    notion: &NotionClient,
    request: &SyncRequest,
    state: &SyncState,
    document_title: &str,
) -> Result<(String, Option<String>, bool)> {
    if let Some(page_id) = state.document_page_id.as_deref() {
        if notion.get_page_title(page_id).await.is_ok() {
            let page_url = state
                .document_page_url
                .clone()
                .or_else(|| Some(page_url_from_id(page_id)));
            return Ok((page_id.to_string(), page_url, false));
        }
    }

    let CreatedPage { id, url } = notion
        .create_document_page(parent_page_id(request), document_title)
        .await
        .map_err(|error| create_page_error(request, error))?;

    Ok((id, url, true))
}

async fn replace_document_page(
    notion: &NotionClient,
    request: &SyncRequest,
    document_title: &str,
    old_page_id: &str,
    blocks: Vec<Value>,
) -> Result<(String, Option<String>)> {
    let CreatedPage {
        id: replacement_id,
        url: replacement_url,
    } = notion
        .create_document_page(parent_page_id(request), document_title)
        .await
        .map_err(|error| create_page_error(request, error))?;

    if let Err(error) = notion.append_blocks(&replacement_id, blocks).await {
        let _ = notion.archive_page(&replacement_id).await;
        return Err(error).context("写入替换文档失败，原有 Notion 文档保持不变");
    }

    if let Err(error) = notion.archive_page(old_page_id).await {
        let _ = notion.archive_page(&replacement_id).await;
        return Err(error).context("替换文档已写入，但无法归档旧文档；已回滚到原有文档");
    }

    Ok((replacement_id, replacement_url))
}

fn parent_page_id(request: &SyncRequest) -> Option<&str> {
    if request.root_page_id.trim().is_empty() {
        None
    } else {
        Some(request.root_page_id.trim())
    }
}

fn create_page_error(request: &SyncRequest, error: anyhow::Error) -> anyhow::Error {
    if request.root_page_id.trim().is_empty() {
        anyhow::anyhow!(
            "无法直接在 Notion 工作区创建页面。当前 Token 很可能是内部 Integration Token；请展开“内部 Integration 兼容设置”，填写一个已共享给该 Integration 的父页面 ID。原始错误：{error}"
        )
    } else {
        anyhow::anyhow!("无法在指定父页面下创建同名文档：{error}")
    }
}

async fn build_file_section(
    notion: &NotionClient,
    file: &crate::models::ScannedFile,
) -> Result<Vec<Value>> {
    let path = Path::new(&file.absolute_path);
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut blocks = vec![
        heading_block(&file.relative_path),
        metadata_callout(
            &file.relative_path,
            &file.mime_type,
            file.size,
            &file.hash,
        ),
    ];

    if TEXT_EXTENSIONS.contains(&extension.as_str()) && file.size <= MAX_INLINE_TEXT_SIZE {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| anyhow::anyhow!("文本文件不是有效 UTF-8：{error}"))?;
        blocks.extend(text_blocks(&content, &extension));
    } else {
        let upload_id = notion.upload_file(path, &file.mime_type).await?;
        blocks.push(file_block(&upload_id, &file.mime_type));
    }

    blocks.push(divider_block());
    Ok(blocks)
}

fn folder_title(folder_path: &str) -> String {
    Path::new(folder_path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("本地文件")
        .to_string()
}

fn emit_progress(
    app: &AppHandle,
    current: usize,
    total: usize,
    relative_path: &str,
    stage: &str,
) {
    let _ = app.emit(
        "sync-progress",
        SyncProgress {
            current,
            total,
            relative_path: relative_path.to_string(),
            stage: stage.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::folder_title;

    #[test]
    fn extracts_folder_name() {
        assert_eq!(folder_title("/home/user/Documents"), "Documents");
    }
}
