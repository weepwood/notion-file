use crate::models::{SyncEntry, SyncItemResult, SyncProgress, SyncRequest, SyncResult};
use crate::notion::{file_block, metadata_callout, text_blocks, NotionClient};
use crate::{scanner, storage};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;
use tauri::{AppHandle, Emitter};

const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml", "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java", "kt", "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php", "swift", "vue", "svelte", "ini", "conf", "env", "csv",
];

pub async fn synchronize(app: &AppHandle, request: SyncRequest) -> Result<SyncResult> {
    if request.root_page_id.trim().is_empty() { anyhow::bail!("目标根页面 ID 不能为空"); }
    let notion = NotionClient::new(storage::load_token()?)?;
    notion.get_page_title(&request.root_page_id).await?;
    let mut state = storage::load_state(app)?;
    let scan = scanner::scan(&request.folder_path, request.skip_hidden, &state)?;
    let started_at = Utc::now().to_rfc3339();
    let total = scan.files.len() + if request.archive_deleted { scan.deleted_count } else { 0 };
    let (mut created, mut updated, mut unchanged, mut archived, mut failed) = (0, 0, 0, 0, 0);
    let mut items = Vec::new();

    for (index, file) in scan.files.iter().enumerate() {
        emit_progress(app, index + 1, total, &file.relative_path, "正在处理");
        if file.status == "unchanged" {
            unchanged += 1;
            items.push(SyncItemResult { relative_path: file.relative_path.clone(), status: "unchanged".into(), page_id: state.entries.get(&file.relative_path).map(|entry| entry.page_id.clone()), message: None });
            continue;
        }
        let existing_page = state.entries.get(&file.relative_path).map(|entry| entry.page_id.clone());
        match sync_one(&notion, &request.root_page_id, file, existing_page.as_deref()).await {
            Ok(page_id) => {
                if existing_page.is_none() { created += 1; } else { updated += 1; }
                state.entries.insert(file.relative_path.clone(), SyncEntry { page_id: page_id.clone(), hash: file.hash.clone(), synced_at: Utc::now().to_rfc3339(), mime_type: file.mime_type.clone() });
                storage::save_state(app, &state)?;
                items.push(SyncItemResult { relative_path: file.relative_path.clone(), status: "synced".into(), page_id: Some(page_id), message: None });
            }
            Err(error) => {
                failed += 1;
                items.push(SyncItemResult { relative_path: file.relative_path.clone(), status: "failed".into(), page_id: existing_page, message: Some(error.to_string()) });
            }
        }
    }

    if request.archive_deleted {
        let current: HashSet<&str> = scan.files.iter().map(|file| file.relative_path.as_str()).collect();
        let deleted: Vec<(String, String)> = state.entries.iter().filter(|(path, _)| !current.contains(path.as_str())).map(|(path, entry)| (path.clone(), entry.page_id.clone())).collect();
        for (offset, (relative_path, page_id)) in deleted.into_iter().enumerate() {
            emit_progress(app, scan.files.len() + offset + 1, total, &relative_path, "正在归档远端页面");
            match notion.archive_page(&page_id).await {
                Ok(()) => {
                    archived += 1;
                    state.entries.remove(&relative_path);
                    storage::save_state(app, &state)?;
                    items.push(SyncItemResult { relative_path, status: "deleted".into(), page_id: Some(page_id), message: None });
                }
                Err(error) => {
                    failed += 1;
                    items.push(SyncItemResult { relative_path, status: "failed".into(), page_id: Some(page_id), message: Some(error.to_string()) });
                }
            }
        }
    }

    Ok(SyncResult { started_at, finished_at: Utc::now().to_rfc3339(), created, updated, unchanged, archived, failed, items })
}

async fn sync_one(notion: &NotionClient, root_page_id: &str, file: &crate::models::ScannedFile, existing_page_id: Option<&str>) -> Result<String> {
    let page_id = match existing_page_id {
        Some(page_id) => { notion.clear_page(page_id).await?; page_id.to_string() }
        None => notion.create_child_page(root_page_id, &file.relative_path).await?,
    };
    let path = Path::new(&file.absolute_path);
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut blocks = vec![metadata_callout(&file.relative_path, &file.hash)];
    if TEXT_EXTENSIONS.contains(&extension.as_str()) {
        let content = tokio::fs::read_to_string(path).await.map_err(|error| anyhow::anyhow!("文本文件不是有效 UTF-8：{error}"))?;
        blocks.extend(text_blocks(&content, &extension));
    } else {
        let upload_id = notion.upload_file(path, &file.mime_type).await?;
        blocks.push(file_block(&upload_id, &file.mime_type));
    }
    notion.append_blocks(&page_id, blocks).await?;
    Ok(page_id)
}

fn emit_progress(app: &AppHandle, current: usize, total: usize, relative_path: &str, stage: &str) {
    let _ = app.emit("sync-progress", SyncProgress { current, total, relative_path: relative_path.to_string(), stage: stage.to_string() });
}
