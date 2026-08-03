use super::{drive_context, new_id, notion_index, parent_path, transfer};
use crate::models::{
    DriveQueueEnqueueRequest, DriveQueueJob, DriveQueueSnapshot, DriveUploadRequest,
};
use crate::storage;
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

const DATABASE_FILE_NAME: &str = "notion-file.sqlite3";
const MAX_QUEUE_HISTORY: i64 = 1000;
const PROGRESS_PERSIST_INTERVAL: u64 = 1024 * 1024;
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);

pub(super) fn recover_and_start(app: AppHandle) -> Result<()> {
    let connection = open(&app)?;
    let now = Utc::now().to_rfc3339();
    connection.execute(
        "UPDATE drive_upload_queue
         SET status = 'pending',
             stage = 'recovered',
             last_error = '应用上次退出时任务被中断，已重新加入队列',
             started_at = NULL,
             updated_at = ?1
         WHERE status = 'running'",
        [now],
    )?;
    drop(connection);
    if !queue_paused(&app)? {
        spawn_worker(app);
    }
    Ok(())
}

pub(super) fn snapshot(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    let connection = open(app)?;
    let paused = read_paused(&connection)?;
    let mut statement = connection.prepare(
        "SELECT id, node_id, parent_id, file_path, file_name, size, status, stage,
                transferred_bytes, attempts, last_error, created_at, updated_at,
                started_at, completed_at
         FROM drive_upload_queue
         ORDER BY created_at DESC, rowid DESC
         LIMIT ?1",
    )?;
    let jobs = statement
        .query_map([MAX_QUEUE_HISTORY], row_to_job)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let running_job_id = jobs
        .iter()
        .find(|job| job.status == "running")
        .map(|job| job.id.clone());
    let pending_count = jobs.iter().filter(|job| job.status == "pending").count();
    let failed_count = jobs.iter().filter(|job| job.status == "failed").count();
    Ok(DriveQueueSnapshot {
        paused,
        worker_running: WORKER_RUNNING.load(Ordering::Acquire),
        running_job_id,
        pending_count,
        failed_count,
        jobs,
    })
}

pub(super) fn enqueue(
    app: &AppHandle,
    request: DriveQueueEnqueueRequest,
) -> Result<DriveQueueSnapshot> {
    if request.file_paths.is_empty() {
        anyhow::bail!("请选择需要加入上传队列的文件");
    }
    let _ = drive_context(app)?;
    let _ = parent_path(app, request.parent_id.as_deref())?;
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    for raw_path in request.file_paths {
        let raw_path = raw_path.trim();
        if raw_path.is_empty() {
            continue;
        }
        let requested = PathBuf::from(raw_path);
        let metadata = std::fs::metadata(&requested)
            .with_context(|| format!("无法读取队列文件：{}", requested.display()))?;
        if !metadata.is_file() {
            anyhow::bail!("队列路径不是文件：{}", requested.display());
        }
        let canonical = std::fs::canonicalize(&requested).unwrap_or(requested);
        let file_name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .context("无法读取队列文件名")?
            .to_string();
        if queue_contains_active_path(&transaction, &canonical, request.parent_id.as_deref())? {
            continue;
        }
        let now = Utc::now().to_rfc3339();
        let job = DriveQueueJob {
            id: new_id("queue"),
            node_id: new_id("node"),
            parent_id: request.parent_id.clone(),
            file_path: canonical.to_string_lossy().to_string(),
            file_name,
            size: metadata.len(),
            status: "pending".to_string(),
            stage: "pending".to_string(),
            transferred_bytes: 0,
            attempts: 0,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            completed_at: None,
        };
        insert_job(&transaction, &job)?;
    }
    transaction.commit()?;
    trim_history(app)?;
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    if !result.paused {
        spawn_worker(app.clone());
    }
    Ok(result)
}

pub(super) fn pause(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    set_paused(app, true)?;
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    Ok(result)
}

pub(super) fn resume(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    set_paused(app, false)?;
    spawn_worker(app.clone());
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    Ok(result)
}

pub(super) fn retry(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {
    let connection = open(app)?;
    let status: Option<String> = connection
        .query_row(
            "SELECT status FROM drive_upload_queue WHERE id = ?1",
            [&job_id],
            |row| row.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("failed") | Some("cancelled") => {}
        Some("running") => anyhow::bail!("正在执行的任务不能重试"),
        Some("pending") => anyhow::bail!("任务已经在等待队列中"),
        Some("completed") => anyhow::bail!("已完成任务无需重试"),
        Some(other) => anyhow::bail!("任务状态 {other} 不能重试"),
        None => anyhow::bail!("找不到上传队列任务"),
    }
    connection.execute(
        "UPDATE drive_upload_queue
         SET status = 'pending', stage = 'pending', transferred_bytes = 0,
             last_error = NULL, started_at = NULL, completed_at = NULL, updated_at = ?2
         WHERE id = ?1",
        params![job_id, Utc::now().to_rfc3339()],
    )?;
    if !queue_paused(app)? {
        spawn_worker(app.clone());
    }
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    Ok(result)
}

pub(super) fn cancel(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {
    let connection = open(app)?;
    let status: Option<String> = connection
        .query_row(
            "SELECT status FROM drive_upload_queue WHERE id = ?1",
            [&job_id],
            |row| row.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("pending") | Some("failed") => {}
        Some("running") => anyhow::bail!("当前任务正在发送数据，只能等待它结束后再暂停队列"),
        Some("completed") => anyhow::bail!("已完成任务不能取消"),
        Some("cancelled") => return snapshot(app),
        Some(other) => anyhow::bail!("任务状态 {other} 不能取消"),
        None => anyhow::bail!("找不到上传队列任务"),
    }
    let now = Utc::now().to_rfc3339();
    connection.execute(
        "UPDATE drive_upload_queue
         SET status = 'cancelled', stage = 'cancelled', last_error = NULL,
             completed_at = ?2, updated_at = ?2
         WHERE id = ?1",
        params![job_id, now],
    )?;
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    Ok(result)
}

pub(super) fn clear_finished(app: &AppHandle) -> Result<DriveQueueSnapshot> {
    let connection = open(app)?;
    connection.execute(
        "DELETE FROM drive_upload_queue WHERE status IN ('completed', 'cancelled')",
        [],
    )?;
    let result = snapshot(app)?;
    emit_snapshot(app, &result);
    Ok(result)
}

pub(super) fn persist_progress(
    app: &AppHandle,
    node_id: &str,
    stage: &str,
    transferred_bytes: u64,
    total_bytes: u64,
) {
    let Ok(connection) = open(app) else {
        return;
    };
    let transferred = i64::try_from(transferred_bytes).unwrap_or(i64::MAX);
    let total = i64::try_from(total_bytes).unwrap_or(i64::MAX);
    let _ = connection.execute(
        "UPDATE drive_upload_queue
         SET stage = ?2, transferred_bytes = ?3, size = MAX(size, ?4), updated_at = ?5
         WHERE node_id = ?1 AND status = 'running'
           AND (stage <> ?2 OR ?3 >= size OR ?3 - transferred_bytes >= ?6)",
        params![
            node_id,
            stage,
            transferred,
            total,
            Utc::now().to_rfc3339(),
            PROGRESS_PERSIST_INTERVAL as i64,
        ],
    );
}

fn spawn_worker(app: AppHandle) {
    if WORKER_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Err(error) = worker_loop(&app).await {
            let _ = app.emit("drive-queue-error", error.to_string());
        }
        WORKER_RUNNING.store(false, Ordering::Release);
        if let Ok(result) = snapshot(&app) {
            emit_snapshot(&app, &result);
        }
    });
}

async fn worker_loop(app: &AppHandle) -> Result<()> {
    loop {
        if queue_paused(app)? {
            break;
        }
        let Some(job) = claim_next(app)? else {
            break;
        };
        if let Ok(result) = snapshot(app) {
            emit_snapshot(app, &result);
        }
        let outcome = execute_job(app, &job).await;
        match outcome {
            Ok(node_id) => complete_job(app, &job.id, &node_id)?,
            Err(error) => fail_job(app, &job.id, &error.to_string())?,
        }
        if let Ok(result) = snapshot(app) {
            emit_snapshot(app, &result);
        }
    }
    Ok(())
}

async fn execute_job(app: &AppHandle, job: &DriveQueueJob) -> Result<String> {
    if !Path::new(&job.file_path).is_file() {
        anyhow::bail!("本地文件不存在或已被移动：{}", job.file_path);
    }
    if job.attempts > 1 {
        if let Some(node) = recover_existing_remote_node(app, job).await? {
            return Ok(node.id);
        }
    }
    let node = transfer::upload_file(
        app,
        DriveUploadRequest {
            file_path: job.file_path.clone(),
            parent_id: job.parent_id.clone(),
            node_id: Some(job.node_id.clone()),
        },
    )
    .await?;
    Ok(node.id)
}

async fn recover_existing_remote_node(
    app: &AppHandle,
    job: &DriveQueueJob,
) -> Result<Option<crate::models::DriveNode>> {
    if let Ok(node) = storage::get_drive_node(app, &job.node_id) {
        return Ok(Some(node));
    }
    let context = drive_context(app)?;
    let nodes = notion_index::fetch_remote_nodes(
        &context.http,
        &context.config.drive_data_source_id,
    )
    .await?;
    let Some(mut node) = nodes.into_iter().find(|item| item.id == job.node_id) else {
        return Ok(None);
    };
    if node.notion_block_id.as_deref().is_none_or(str::is_empty) {
        if let Some(upload_id) = node.file_upload_id.clone().filter(|value| !value.is_empty()) {
            let mime_type = node
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let block_id = notion_index::append_file_block(
                &context.http,
                &node.notion_page_id,
                &upload_id,
                &mime_type,
            )
            .await?;
            notion_index::patch_remote_block_id(
                &context.http,
                &node.notion_page_id,
                &block_id,
            )
            .await?;
            node.notion_block_id = Some(block_id);
        } else {
            let _ = notion_index::trash_remote_page(&context.http, &node.notion_page_id).await;
            return Ok(None);
        }
    }
    storage::insert_drive_node(app, &node)?;
    Ok(Some(node))
}

fn claim_next(app: &AppHandle) -> Result<Option<DriveQueueJob>> {
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    let job = transaction
        .query_row(
            "SELECT id, node_id, parent_id, file_path, file_name, size, status, stage,
                    transferred_bytes, attempts, last_error, created_at, updated_at,
                    started_at, completed_at
             FROM drive_upload_queue
             WHERE status = 'pending'
             ORDER BY created_at ASC, rowid ASC
             LIMIT 1",
            [],
            row_to_job,
        )
        .optional()?;
    let Some(mut job) = job else {
        transaction.commit()?;
        return Ok(None);
    };
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "UPDATE drive_upload_queue
         SET status = 'running', stage = 'starting', attempts = attempts + 1,
             last_error = NULL, started_at = ?2, updated_at = ?2
         WHERE id = ?1 AND status = 'pending'",
        params![job.id, now],
    )?;
    transaction.commit()?;
    job.status = "running".to_string();
    job.stage = "starting".to_string();
    job.attempts += 1;
    job.last_error = None;
    job.started_at = Some(now.clone());
    job.updated_at = now;
    Ok(Some(job))
}

fn complete_job(app: &AppHandle, job_id: &str, node_id: &str) -> Result<()> {
    let connection = open(app)?;
    let now = Utc::now().to_rfc3339();
    connection.execute(
        "UPDATE drive_upload_queue
         SET status = 'completed', stage = 'completed', transferred_bytes = size,
             node_id = ?2, last_error = NULL, completed_at = ?3, updated_at = ?3
         WHERE id = ?1",
        params![job_id, node_id, now],
    )?;
    Ok(())
}

fn fail_job(app: &AppHandle, job_id: &str, message: &str) -> Result<()> {
    let connection = open(app)?;
    connection.execute(
        "UPDATE drive_upload_queue
         SET status = 'failed', stage = 'failed', last_error = ?2, updated_at = ?3
         WHERE id = ?1",
        params![job_id, message, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn queue_contains_active_path(
    transaction: &Transaction<'_>,
    path: &Path,
    parent_id: Option<&str>,
) -> Result<bool> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM drive_upload_queue
         WHERE file_path = ?1
           AND COALESCE(parent_id, '') = COALESCE(?2, '')
           AND status IN ('pending', 'running')",
        params![path.to_string_lossy(), parent_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn insert_job(transaction: &Transaction<'_>, job: &DriveQueueJob) -> Result<()> {
    transaction.execute(
        "INSERT INTO drive_upload_queue(
             id, node_id, parent_id, file_path, file_name, size, status, stage,
             transferred_bytes, attempts, last_error, created_at, updated_at,
             started_at, completed_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            job.id,
            job.node_id,
            job.parent_id,
            job.file_path,
            job.file_name,
            i64::try_from(job.size).context("队列文件大小超过 SQLite 整数范围")?,
            job.status,
            job.stage,
            i64::try_from(job.transferred_bytes)
                .context("队列已传输大小超过 SQLite 整数范围")?,
            job.attempts,
            job.last_error,
            job.created_at,
            job.updated_at,
            job.started_at,
            job.completed_at,
        ],
    )?;
    Ok(())
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriveQueueJob> {
    let size: i64 = row.get(5)?;
    let transferred: i64 = row.get(8)?;
    Ok(DriveQueueJob {
        id: row.get(0)?,
        node_id: row.get(1)?,
        parent_id: row.get(2)?,
        file_path: row.get(3)?,
        file_name: row.get(4)?,
        size: size.max(0) as u64,
        status: row.get(6)?,
        stage: row.get(7)?,
        transferred_bytes: transferred.max(0) as u64,
        attempts: row.get::<_, i64>(9)?.max(0),
        last_error: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        started_at: row.get(13)?,
        completed_at: row.get(14)?,
    })
}

fn set_paused(app: &AppHandle, paused: bool) -> Result<()> {
    let connection = open(app)?;
    connection.execute(
        "INSERT INTO drive_queue_state(id, paused) VALUES(1, ?1)
         ON CONFLICT(id) DO UPDATE SET paused = excluded.paused",
        [paused as i64],
    )?;
    Ok(())
}

fn queue_paused(app: &AppHandle) -> Result<bool> {
    let connection = open(app)?;
    read_paused(&connection)
}

fn read_paused(connection: &Connection) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT paused FROM drive_queue_state WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0)
        != 0)
}

fn trim_history(app: &AppHandle) -> Result<()> {
    let connection = open(app)?;
    connection.execute(
        "DELETE FROM drive_upload_queue
         WHERE id NOT IN (
             SELECT id FROM drive_upload_queue
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?1
         ) AND status IN ('completed', 'cancelled')",
        [MAX_QUEUE_HISTORY],
    )?;
    Ok(())
}

fn database_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join(DATABASE_FILE_NAME))
}

fn open(app: &AppHandle) -> Result<Connection> {
    let path = database_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("无法创建队列数据库目录")?;
    }
    let connection = Connection::open(&path)
        .with_context(|| format!("无法打开上传队列数据库：{}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS drive_upload_queue (
             id TEXT PRIMARY KEY NOT NULL,
             node_id TEXT NOT NULL,
             parent_id TEXT,
             file_path TEXT NOT NULL,
             file_name TEXT NOT NULL,
             size INTEGER NOT NULL DEFAULT 0 CHECK(size >= 0),
             status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
             stage TEXT NOT NULL,
             transferred_bytes INTEGER NOT NULL DEFAULT 0 CHECK(transferred_bytes >= 0),
             attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
             last_error TEXT,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             started_at TEXT,
             completed_at TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_drive_upload_queue_status
             ON drive_upload_queue(status, created_at ASC);
         CREATE INDEX IF NOT EXISTS idx_drive_upload_queue_node
             ON drive_upload_queue(node_id);
         CREATE TABLE IF NOT EXISTS drive_queue_state (
             id INTEGER PRIMARY KEY CHECK(id = 1),
             paused INTEGER NOT NULL DEFAULT 0 CHECK(paused IN (0, 1))
         );
         INSERT OR IGNORE INTO drive_queue_state(id, paused) VALUES(1, 0);",
    )?;
    Ok(connection)
}

fn emit_snapshot(app: &AppHandle, snapshot: &DriveQueueSnapshot) {
    let _ = app.emit("drive-queue-changed", snapshot.clone());
}

#[cfg(test)]
mod tests {
    use super::row_to_job;
    use rusqlite::Connection;

    #[test]
    fn restores_queue_job_from_sqlite_row() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs(
                    id TEXT, node_id TEXT, parent_id TEXT, file_path TEXT, file_name TEXT,
                    size INTEGER, status TEXT, stage TEXT, transferred_bytes INTEGER,
                    attempts INTEGER, last_error TEXT, created_at TEXT, updated_at TEXT,
                    started_at TEXT, completed_at TEXT
                );
                INSERT INTO jobs VALUES(
                    'q1', 'n1', NULL, '/tmp/a.txt', 'a.txt', 10, 'pending', 'pending',
                    0, 0, NULL, '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z', NULL, NULL
                );",
            )
            .unwrap();
        let job = connection
            .query_row("SELECT * FROM jobs", [], row_to_job)
            .unwrap();
        assert_eq!(job.id, "q1");
        assert_eq!(job.size, 10);
        assert_eq!(job.status, "pending");
    }
}
