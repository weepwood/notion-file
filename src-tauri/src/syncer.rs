use crate::models::{
    BackupEntry, BackupItemResult, BackupJob, BackupProgress, BackupResult, BackupSnapshot,
    RestoreRequest, RestoreResult, TaskState,
};
use crate::notion::{
    deletion_callout, file_block, metadata_callout, preview_blocks, summary_blocks,
    version_heading, NotionClient,
};
use crate::{scanner, storage};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use tauri::{AppHandle, Emitter};

const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "mdx", "txt", "log", "json", "jsonc", "yaml", "yml", "toml", "xml",
    "html", "css", "scss", "less", "js", "jsx", "ts", "tsx", "py", "rs", "go", "java",
    "kt", "kts", "c", "h", "cpp", "hpp", "cs", "sh", "bash", "ps1", "sql", "rb", "php",
    "swift", "vue", "svelte", "ini", "conf", "env", "csv",
];
const PREVIEW_SIZE_LIMIT: u64 = 1024 * 1024;

pub async fn backup(app: &AppHandle, job: BackupJob) -> Result<BackupResult> {
    validate_job(&job)?;
    let notion = NotionClient::new(storage::load_token()?)?;
    notion.get_page_title(&job.root_page_id).await?;

    let mut state = storage::load_state(app)?;
    let mut task = state.tasks.get(&job.id).cloned().unwrap_or_default();
    let scan = scanner::scan(&job.folder_path, job.skip_hidden, &task)?;
    let started_at = Utc::now().to_rfc3339();
    let total = scan.files.len() + scan.deleted_count;
    let (mut uploaded, mut unchanged, mut marked_deleted, mut failed) = (0, 0, 0, 0);
    let mut items = Vec::new();

    for (index, file) in scan.files.iter().enumerate() {
        emit_progress(app, index + 1, total, &file.relative_path, "检查文件");
        if file.status == "unchanged" {
            unchanged += 1;
            items.push(BackupItemResult {
                relative_path: file.relative_path.clone(),
                status: "unchanged".into(),
                page_id: task.entries.get(&file.relative_path).map(|entry| entry.page_id.clone()),
                message: None,
            });
            continue;
        }

        let previous = task.entries.get(&file.relative_path).cloned();
        emit_progress(app, index + 1, total, &file.relative_path, "上传到 Notion");
        match backup_one(&notion, &job, file, previous.as_ref()).await {
            Ok(entry) => {
                uploaded += 1;
                let page_id = entry.page_id.clone();
                task.entries.insert(file.relative_path.clone(), entry);
                state.tasks.insert(job.id.clone(), task.clone());
                storage::save_state(app, &state)?;
                items.push(BackupItemResult {
                    relative_path: file.relative_path.clone(),
                    status: "backed_up".into(),
                    page_id: Some(page_id),
                    message: None,
                });
            }
            Err(error) => {
                failed += 1;
                items.push(BackupItemResult {
                    relative_path: file.relative_path.clone(),
                    status: "failed".into(),
                    page_id: previous.map(|entry| entry.page_id),
                    message: Some(error.to_string()),
                });
            }
        }
    }

    let current: HashSet<&str> = scan.files.iter().map(|file| file.relative_path.as_str()).collect();
    let missing: Vec<(String, BackupEntry)> = task.entries.iter()
        .filter(|(path, entry)| !entry.deleted && !current.contains(path.as_str()))
        .map(|(path, entry)| (path.clone(), entry.clone()))
        .collect();

    for (offset, (relative_path, mut entry)) in missing.into_iter().enumerate() {
        emit_progress(app, scan.files.len() + offset + 1, total, &relative_path, "记录本地删除");
        let timestamp = Utc::now().to_rfc3339();
        match notion.append_blocks(&entry.page_id, vec![deletion_callout(&relative_path, &timestamp)]).await {
            Ok(()) => {
                marked_deleted += 1;
                entry.deleted = true;
                entry.backed_up_at = timestamp;
                task.entries.insert(relative_path.clone(), entry.clone());
                state.tasks.insert(job.id.clone(), task.clone());
                storage::save_state(app, &state)?;
                items.push(BackupItemResult {
                    relative_path,
                    status: "marked_deleted".into(),
                    page_id: Some(entry.page_id),
                    message: None,
                });
            }
            Err(error) => {
                failed += 1;
                items.push(BackupItemResult {
                    relative_path,
                    status: "failed".into(),
                    page_id: Some(entry.page_id),
                    message: Some(error.to_string()),
                });
            }
        }
    }

    let finished_at = Utc::now().to_rfc3339();
    let snapshot_page_id = if uploaded + marked_deleted + failed > 0 {
        match create_snapshot_page(
            &notion,
            &job,
            &task,
            &started_at,
            &finished_at,
            uploaded,
            unchanged,
            marked_deleted,
            failed,
            scan.total_bytes,
        ).await {
            Ok(page_id) => Some(page_id),
            Err(error) => {
                failed += 1;
                items.push(BackupItemResult {
                    relative_path: "备份记录页面".into(),
                    status: "failed".into(),
                    page_id: None,
                    message: Some(error.to_string()),
                });
                None
            }
        }
    } else {
        None
    };

    let snapshot = BackupSnapshot {
        id: format!("{}-{}", job.id, Utc::now().timestamp_millis()),
        started_at: started_at.clone(),
        finished_at: finished_at.clone(),
        summary_page_id: snapshot_page_id.clone(),
        total_files: scan.files.len(),
        total_bytes: scan.total_bytes,
        uploaded,
        unchanged,
        marked_deleted,
        failed,
    };
    task.snapshots.push(snapshot);
    if task.snapshots.len() > 100 {
        let remove_count = task.snapshots.len() - 100;
        task.snapshots.drain(0..remove_count);
    }
    state.tasks.insert(job.id.clone(), task);
    storage::save_state(app, &state)?;

    Ok(BackupResult {
        started_at,
        finished_at,
        snapshot_page_id,
        uploaded,
        unchanged,
        marked_deleted,
        failed,
        total_bytes: scan.total_bytes,
        items,
    })
}

async fn backup_one(
    notion: &NotionClient,
    job: &BackupJob,
    file: &crate::models::ScannedFile,
    previous: Option<&BackupEntry>,
) -> Result<BackupEntry> {
    let page_id = match previous {
        Some(entry) => entry.page_id.clone(),
        None => notion.create_child_page(&job.root_page_id, &format!("📄 {}", file.relative_path)).await?,
    };
    let path = Path::new(&file.absolute_path);
    let version = previous.map(|entry| entry.version.saturating_add(1)).unwrap_or(1);
    let timestamp = Utc::now().to_rfc3339();
    let upload_id = notion.upload_file(path, &file.mime_type).await?;
    let mut blocks = vec![
        version_heading(version, &timestamp),
        metadata_callout(&file.relative_path, &file.hash, file.size, file.modified_at),
        file_block(&upload_id, &file.mime_type),
    ];

    if job.include_text_preview && file.size <= PREVIEW_SIZE_LIMIT {
        let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
        if TEXT_EXTENSIONS.contains(&extension.as_str()) {
            if let Ok(content) = tokio::fs::read_to_string(path).await {
                blocks.extend(preview_blocks(&content, &extension));
            }
        }
    }
    notion.append_blocks(&page_id, blocks).await?;

    Ok(BackupEntry {
        page_id,
        hash: file.hash.clone(),
        upload_id: Some(upload_id),
        size: file.size,
        modified_at: file.modified_at,
        backed_up_at: timestamp,
        mime_type: file.mime_type.clone(),
        version,
        deleted: false,
    })
}

#[allow(clippy::too_many_arguments)]
async fn create_snapshot_page(
    notion: &NotionClient,
    job: &BackupJob,
    task: &TaskState,
    started_at: &str,
    finished_at: &str,
    uploaded: usize,
    unchanged: usize,
    marked_deleted: usize,
    failed: usize,
    total_bytes: u64,
) -> Result<String> {
    let title = format!("备份记录 · {} · {}", job.name, Utc::now().format("%Y-%m-%d %H:%M:%S UTC"));
    let page_id = notion.create_child_page(&job.root_page_id, &title).await?;
    let summary = format!(
        "任务：{}\n来源：{}\n开始：{}\n结束：{}\n上传：{}\n未变化：{}\n标记删除：{}\n失败：{}\n扫描总量：{} bytes",
        job.name, job.folder_path, started_at, finished_at, uploaded, unchanged, marked_deleted, failed, total_bytes
    );
    let mut entries: Vec<_> = task.entries.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let manifest = entries.into_iter().map(|(path, entry)| json!({
        "path": path,
        "page_id": &entry.page_id,
        "sha256": &entry.hash,
        "size": entry.size,
        "mime_type": &entry.mime_type,
        "version": entry.version,
        "deleted": entry.deleted,
        "backed_up_at": &entry.backed_up_at,
    })).collect::<Vec<_>>();
    notion.append_blocks(&page_id, summary_blocks(&summary, &serde_json::to_string_pretty(&manifest)?)).await?;
    Ok(page_id)
}

pub async fn restore(app: &AppHandle, request: RestoreRequest) -> Result<RestoreResult> {
    if request.destination_path.trim().is_empty() { anyhow::bail!("恢复目标文件夹不能为空"); }
    let state = storage::load_state(app)?;
    let task = state.tasks.get(&request.job_id).context("没有找到该备份任务的本地索引")?;
    let notion = NotionClient::new(storage::load_token()?)?;
    let destination_root = Path::new(&request.destination_path);
    tokio::fs::create_dir_all(destination_root).await?;

    let active: Vec<(String, BackupEntry)> = task.entries.iter()
        .filter(|(_, entry)| !entry.deleted)
        .map(|(path, entry)| (path.clone(), entry.clone()))
        .collect();
    let mut result = RestoreResult { restored: 0, skipped: 0, failed: 0, items: Vec::new() };
    for (index, (relative_path, entry)) in active.iter().enumerate() {
        emit_progress(app, index + 1, active.len(), relative_path, "恢复文件");
        let target = match safe_join(destination_root, relative_path) {
            Ok(path) => path,
            Err(error) => {
                result.failed += 1;
                result.items.push(BackupItemResult { relative_path: relative_path.clone(), status: "failed".into(), page_id: Some(entry.page_id.clone()), message: Some(error.to_string()) });
                continue;
            }
        };
        if target.exists() && !request.overwrite {
            result.skipped += 1;
            result.items.push(BackupItemResult { relative_path: relative_path.clone(), status: "skipped".into(), page_id: Some(entry.page_id.clone()), message: Some("目标文件已存在".into()) });
            continue;
        }
        match notion.latest_file_url(&entry.page_id).await {
            Ok(url) => match notion.download_file(&url, &target).await {
                Ok(()) => {
                    result.restored += 1;
                    result.items.push(BackupItemResult { relative_path: relative_path.clone(), status: "restored".into(), page_id: Some(entry.page_id.clone()), message: None });
                }
                Err(error) => {
                    result.failed += 1;
                    result.items.push(BackupItemResult { relative_path: relative_path.clone(), status: "failed".into(), page_id: Some(entry.page_id.clone()), message: Some(error.to_string()) });
                }
            },
            Err(error) => {
                result.failed += 1;
                result.items.push(BackupItemResult { relative_path: relative_path.clone(), status: "failed".into(), page_id: Some(entry.page_id.clone()), message: Some(error.to_string()) });
            }
        }
    }
    Ok(result)
}

pub fn history(app: &AppHandle, job_id: &str) -> Result<Vec<BackupSnapshot>> {
    let state = storage::load_state(app)?;
    Ok(state.tasks.get(job_id).map(|task| task.snapshots.iter().rev().cloned().collect()).unwrap_or_default())
}

fn validate_job(job: &BackupJob) -> Result<()> {
    if job.id.trim().is_empty() { anyhow::bail!("备份任务 ID 不能为空"); }
    if job.folder_path.trim().is_empty() { anyhow::bail!("本地文件夹不能为空"); }
    if job.root_page_id.trim().is_empty() { anyhow::bail!("目标 Notion 页面 ID 不能为空"); }
    Ok(())
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute() || path.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        anyhow::bail!("备份索引包含不安全路径：{relative}");
    }
    Ok(root.join(path))
}

fn emit_progress(app: &AppHandle, current: usize, total: usize, relative_path: &str, stage: &str) {
    let _ = app.emit("backup-progress", BackupProgress {
        current,
        total,
        relative_path: relative_path.to_string(),
        stage: stage.to_string(),
    });
}
