use crate::models::{DriveNode, DriveTransfer, UploadRecord};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const DATABASE_FILE_NAME: &str = "notion-file.sqlite3";
const LEGACY_MIGRATION_KEY: &str = "upload_history_json_migrated";
const MAX_UPLOAD_HISTORY: usize = 500;
const MAX_TRANSFER_HISTORY: usize = 1000;

pub fn load_upload_history(app: &AppHandle, legacy_path: &Path) -> Result<Vec<UploadRecord>> {
    let mut connection = open(app)?;
    migrate_legacy_upload_history(&mut connection, legacy_path)?;

    let mut statement = connection.prepare(
        "SELECT id, file_path, file_name, size, mime_type, sha256, uploaded_at, status,
                page_id, page_url, message, display_mode, segment_count, used_ffmpeg
         FROM upload_history
         ORDER BY uploaded_at DESC, rowid DESC
         LIMIT ?1",
    )?;

    let records = statement
        .query_map([MAX_UPLOAD_HISTORY as i64], row_to_upload_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

pub fn append_upload_record(
    app: &AppHandle,
    legacy_path: &Path,
    record: &UploadRecord,
) -> Result<()> {
    let mut connection = open(app)?;
    migrate_legacy_upload_history(&mut connection, legacy_path)?;

    let transaction = connection.transaction()?;
    insert_upload_record(&transaction, record)?;
    trim_upload_history(&transaction)?;
    transaction.commit()?;
    Ok(())
}

pub fn clear_upload_history(app: &AppHandle, legacy_path: &Path) -> Result<()> {
    let mut connection = open(app)?;
    migrate_legacy_upload_history(&mut connection, legacy_path)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM upload_history", [])?;
    transaction.commit()?;
    Ok(())
}

pub fn list_drive_nodes(app: &AppHandle, include_trashed: bool) -> Result<Vec<DriveNode>> {
    let connection = open(app)?;
    list_drive_nodes_with_connection(&connection, include_trashed)
}

pub fn get_drive_node(app: &AppHandle, node_id: &str) -> Result<DriveNode> {
    let connection = open(app)?;
    connection
        .query_row(
            "SELECT id, parent_id, node_type, name, logical_path, mime_type, size, sha256,
                    notion_page_id, notion_page_url, notion_block_id, file_upload_id, status,
                    version, original_path, created_at, modified_at
             FROM drive_nodes WHERE id = ?1",
            [node_id],
            row_to_drive_node,
        )
        .optional()?
        .with_context(|| format!("找不到云盘节点：{node_id}"))
}

pub fn insert_drive_node(app: &AppHandle, node: &DriveNode) -> Result<()> {
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    upsert_drive_node(&transaction, node)?;
    transaction.commit()?;
    Ok(())
}

pub fn update_drive_nodes(app: &AppHandle, nodes: &[DriveNode]) -> Result<()> {
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    for node in nodes {
        upsert_drive_node(&transaction, node)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn replace_drive_nodes(app: &AppHandle, nodes: &[DriveNode]) -> Result<()> {
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM drive_nodes", [])?;
    for node in nodes {
        upsert_drive_node(&transaction, node)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn find_drive_file_by_hash(app: &AppHandle, sha256: &str) -> Result<Option<DriveNode>> {
    let connection = open(app)?;
    connection
        .query_row(
            "SELECT id, parent_id, node_type, name, logical_path, mime_type, size, sha256,
                    notion_page_id, notion_page_url, notion_block_id, file_upload_id, status,
                    version, original_path, created_at, modified_at
             FROM drive_nodes
             WHERE node_type = 'file' AND status = 'active' AND sha256 = ?1
             ORDER BY modified_at DESC
             LIMIT 1",
            [sha256],
            row_to_drive_node,
        )
        .optional()
        .context("查询重复文件失败")
}

pub fn list_drive_subtree(app: &AppHandle, root_id: &str) -> Result<Vec<DriveNode>> {
    let root = get_drive_node(app, root_id)?;
    let prefix = format!("{}/", root.logical_path.trim_end_matches('/'));
    let mut nodes: Vec<DriveNode> = list_drive_nodes(app, true)?
        .into_iter()
        .filter(|node| node.id == root.id || node.logical_path.starts_with(&prefix))
        .collect();
    nodes.sort_by_key(|node| node.logical_path.matches('/').count());
    Ok(nodes)
}

pub fn append_drive_transfer(app: &AppHandle, transfer: &DriveTransfer) -> Result<()> {
    let mut connection = open(app)?;
    let transaction = connection.transaction()?;
    upsert_drive_transfer(&transaction, transfer)?;
    trim_drive_transfers(&transaction)?;
    transaction.commit()?;
    Ok(())
}

pub fn update_drive_transfer(app: &AppHandle, transfer: &DriveTransfer) -> Result<()> {
    append_drive_transfer(app, transfer)
}

pub fn list_drive_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {
    let connection = open(app)?;
    let mut statement = connection.prepare(
        "SELECT id, node_id, direction, file_name, local_path, status, total_bytes,
                transferred_bytes, message, created_at, updated_at
         FROM drive_transfers
         ORDER BY created_at DESC, rowid DESC
         LIMIT ?1",
    )?;
    let rows = statement
        .query_map([MAX_TRANSFER_HISTORY as i64], row_to_drive_transfer)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn mark_interrupted_transfers(app: &AppHandle, updated_at: &str) -> Result<usize> {
    let connection = open(app)?;
    let count = connection.execute(
        "UPDATE drive_transfers
         SET status = 'failed',
             message = COALESCE(message, '应用上次退出时传输被中断'),
             updated_at = ?1
         WHERE status IN ('queued', 'running')",
        [updated_at],
    )?;
    Ok(count)
}

pub fn clear_finished_drive_transfers(app: &AppHandle) -> Result<usize> {
    let connection = open(app)?;
    let count = connection.execute(
        "DELETE FROM drive_transfers WHERE status IN ('completed', 'failed', 'cancelled')",
        [],
    )?;
    Ok(count)
}

fn database_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join(DATABASE_FILE_NAME))
}

fn open(app: &AppHandle) -> Result<Connection> {
    let path = database_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("无法创建 SQLite 数据目录")?;
    }

    let connection = Connection::open(&path)
        .with_context(|| format!("无法打开 SQLite 数据库：{}", path.display()))?;
    initialize(&connection)?;
    Ok(connection)
}

fn initialize(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;

             CREATE TABLE IF NOT EXISTS app_meta (
                 key TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS upload_history (
                 id TEXT PRIMARY KEY NOT NULL,
                 file_path TEXT NOT NULL,
                 file_name TEXT NOT NULL,
                 size INTEGER NOT NULL CHECK (size >= 0),
                 mime_type TEXT NOT NULL,
                 sha256 TEXT NOT NULL,
                 uploaded_at TEXT NOT NULL,
                 status TEXT NOT NULL,
                 page_id TEXT,
                 page_url TEXT,
                 message TEXT,
                 display_mode TEXT NOT NULL DEFAULT 'file',
                 segment_count INTEGER NOT NULL DEFAULT 0 CHECK (segment_count >= 0),
                 used_ffmpeg INTEGER NOT NULL DEFAULT 0 CHECK (used_ffmpeg IN (0, 1))
             );

             CREATE INDEX IF NOT EXISTS idx_upload_history_uploaded_at
                 ON upload_history(uploaded_at DESC);
             CREATE INDEX IF NOT EXISTS idx_upload_history_status
                 ON upload_history(status, uploaded_at DESC);
             CREATE INDEX IF NOT EXISTS idx_upload_history_sha256
                 ON upload_history(sha256);

             CREATE TABLE IF NOT EXISTS drive_nodes (
                 id TEXT PRIMARY KEY NOT NULL,
                 parent_id TEXT,
                 node_type TEXT NOT NULL CHECK (node_type IN ('file', 'folder')),
                 name TEXT NOT NULL,
                 logical_path TEXT NOT NULL,
                 mime_type TEXT,
                 size INTEGER NOT NULL DEFAULT 0 CHECK (size >= 0),
                 sha256 TEXT,
                 notion_page_id TEXT NOT NULL UNIQUE,
                 notion_page_url TEXT,
                 notion_block_id TEXT,
                 file_upload_id TEXT,
                 status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'trashed')),
                 version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
                 original_path TEXT,
                 created_at TEXT NOT NULL,
                 modified_at TEXT NOT NULL,
                 FOREIGN KEY(parent_id) REFERENCES drive_nodes(id) DEFERRABLE INITIALLY DEFERRED
             );

             CREATE UNIQUE INDEX IF NOT EXISTS idx_drive_nodes_active_path
                 ON drive_nodes(logical_path) WHERE status = 'active';
             CREATE INDEX IF NOT EXISTS idx_drive_nodes_parent
                 ON drive_nodes(parent_id, status, name);
             CREATE INDEX IF NOT EXISTS idx_drive_nodes_sha256
                 ON drive_nodes(sha256, status);
             CREATE INDEX IF NOT EXISTS idx_drive_nodes_path
                 ON drive_nodes(logical_path);

             CREATE TABLE IF NOT EXISTS drive_transfers (
                 id TEXT PRIMARY KEY NOT NULL,
                 node_id TEXT,
                 direction TEXT NOT NULL CHECK (direction IN ('upload', 'download')),
                 file_name TEXT NOT NULL,
                 local_path TEXT,
                 status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
                 total_bytes INTEGER NOT NULL DEFAULT 0 CHECK (total_bytes >= 0),
                 transferred_bytes INTEGER NOT NULL DEFAULT 0 CHECK (transferred_bytes >= 0),
                 message TEXT,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_drive_transfers_created_at
                 ON drive_transfers(created_at DESC);
             CREATE INDEX IF NOT EXISTS idx_drive_transfers_status
                 ON drive_transfers(status, updated_at DESC);

             PRAGMA user_version = 2;",
        )
        .context("无法初始化 SQLite 数据库结构")?;
    Ok(())
}

fn list_drive_nodes_with_connection(
    connection: &Connection,
    include_trashed: bool,
) -> Result<Vec<DriveNode>> {
    let sql = if include_trashed {
        "SELECT id, parent_id, node_type, name, logical_path, mime_type, size, sha256,
                notion_page_id, notion_page_url, notion_block_id, file_upload_id, status,
                version, original_path, created_at, modified_at
         FROM drive_nodes
         ORDER BY node_type DESC, name COLLATE NOCASE"
    } else {
        "SELECT id, parent_id, node_type, name, logical_path, mime_type, size, sha256,
                notion_page_id, notion_page_url, notion_block_id, file_upload_id, status,
                version, original_path, created_at, modified_at
         FROM drive_nodes
         WHERE status = 'active'
         ORDER BY node_type DESC, name COLLATE NOCASE"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = statement
        .query_map([], row_to_drive_node)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn migrate_legacy_upload_history(connection: &mut Connection, legacy_path: &Path) -> Result<()> {
    let migrated = connection
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            [LEGACY_MIGRATION_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if migrated.as_deref() == Some("1") {
        return Ok(());
    }

    let records = if legacy_path.exists() {
        let content = std::fs::read_to_string(legacy_path)
            .context("无法读取旧版上传历史 JSON，SQLite 迁移已中止")?;
        serde_json::from_str::<Vec<UploadRecord>>(&content)
            .context("旧版上传历史 JSON 格式无效，SQLite 迁移已中止")?
    } else {
        Vec::new()
    };

    let transaction = connection.transaction()?;
    for record in &records {
        insert_upload_record(&transaction, record)?;
    }
    trim_upload_history(&transaction)?;
    transaction.execute(
        "INSERT INTO app_meta(key, value) VALUES(?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [LEGACY_MIGRATION_KEY],
    )?;
    transaction.commit()?;

    Ok(())
}

fn trim_upload_history(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute(
        "DELETE FROM upload_history
         WHERE id NOT IN (
             SELECT id FROM upload_history
             ORDER BY uploaded_at DESC, rowid DESC
             LIMIT ?1
         )",
        [MAX_UPLOAD_HISTORY as i64],
    )?;
    Ok(())
}

fn trim_drive_transfers(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute(
        "DELETE FROM drive_transfers
         WHERE id NOT IN (
             SELECT id FROM drive_transfers
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?1
         )",
        [MAX_TRANSFER_HISTORY as i64],
    )?;
    Ok(())
}

fn insert_upload_record(transaction: &Transaction<'_>, record: &UploadRecord) -> Result<()> {
    let size = i64::try_from(record.size).context("上传记录中的文件大小超过 SQLite 整数范围")?;
    let segment_count = i64::try_from(record.segment_count)
        .context("上传记录中的分段数量超过 SQLite 整数范围")?;

    transaction.execute(
        "INSERT INTO upload_history(
             id, file_path, file_name, size, mime_type, sha256, uploaded_at, status,
             page_id, page_url, message, display_mode, segment_count, used_ffmpeg
         ) VALUES(
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
         )
         ON CONFLICT(id) DO UPDATE SET
             file_path = excluded.file_path,
             file_name = excluded.file_name,
             size = excluded.size,
             mime_type = excluded.mime_type,
             sha256 = excluded.sha256,
             uploaded_at = excluded.uploaded_at,
             status = excluded.status,
             page_id = excluded.page_id,
             page_url = excluded.page_url,
             message = excluded.message,
             display_mode = excluded.display_mode,
             segment_count = excluded.segment_count,
             used_ffmpeg = excluded.used_ffmpeg",
        params![
            record.id,
            record.file_path,
            record.file_name,
            size,
            record.mime_type,
            record.sha256,
            record.uploaded_at,
            record.status,
            record.page_id,
            record.page_url,
            record.message,
            record.display_mode,
            segment_count,
            record.used_ffmpeg as i64,
        ],
    )?;
    Ok(())
}

fn upsert_drive_node(transaction: &Transaction<'_>, node: &DriveNode) -> Result<()> {
    let size = i64::try_from(node.size).context("云盘节点文件大小超过 SQLite 整数范围")?;
    transaction.execute(
        "INSERT INTO drive_nodes(
             id, parent_id, node_type, name, logical_path, mime_type, size, sha256,
             notion_page_id, notion_page_url, notion_block_id, file_upload_id, status,
             version, original_path, created_at, modified_at
         ) VALUES(
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
         )
         ON CONFLICT(id) DO UPDATE SET
             parent_id = excluded.parent_id,
             node_type = excluded.node_type,
             name = excluded.name,
             logical_path = excluded.logical_path,
             mime_type = excluded.mime_type,
             size = excluded.size,
             sha256 = excluded.sha256,
             notion_page_id = excluded.notion_page_id,
             notion_page_url = excluded.notion_page_url,
             notion_block_id = excluded.notion_block_id,
             file_upload_id = excluded.file_upload_id,
             status = excluded.status,
             version = excluded.version,
             original_path = excluded.original_path,
             created_at = excluded.created_at,
             modified_at = excluded.modified_at",
        params![
            node.id,
            node.parent_id,
            node.node_type,
            node.name,
            node.logical_path,
            node.mime_type,
            size,
            node.sha256,
            node.notion_page_id,
            node.notion_page_url,
            node.notion_block_id,
            node.file_upload_id,
            node.status,
            node.version,
            node.original_path,
            node.created_at,
            node.modified_at,
        ],
    )?;
    Ok(())
}

fn upsert_drive_transfer(transaction: &Transaction<'_>, transfer: &DriveTransfer) -> Result<()> {
    let total_bytes = i64::try_from(transfer.total_bytes)
        .context("传输任务总大小超过 SQLite 整数范围")?;
    let transferred_bytes = i64::try_from(transfer.transferred_bytes)
        .context("传输任务已完成大小超过 SQLite 整数范围")?;
    transaction.execute(
        "INSERT INTO drive_transfers(
             id, node_id, direction, file_name, local_path, status, total_bytes,
             transferred_bytes, message, created_at, updated_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
             node_id = excluded.node_id,
             direction = excluded.direction,
             file_name = excluded.file_name,
             local_path = excluded.local_path,
             status = excluded.status,
             total_bytes = excluded.total_bytes,
             transferred_bytes = excluded.transferred_bytes,
             message = excluded.message,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        params![
            transfer.id,
            transfer.node_id,
            transfer.direction,
            transfer.file_name,
            transfer.local_path,
            transfer.status,
            total_bytes,
            transferred_bytes,
            transfer.message,
            transfer.created_at,
            transfer.updated_at,
        ],
    )?;
    Ok(())
}

fn row_to_upload_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UploadRecord> {
    let size: i64 = row.get(3)?;
    let segment_count: i64 = row.get(12)?;
    let used_ffmpeg: i64 = row.get(13)?;

    Ok(UploadRecord {
        id: row.get(0)?,
        file_path: row.get(1)?,
        file_name: row.get(2)?,
        size: size.max(0) as u64,
        mime_type: row.get(4)?,
        sha256: row.get(5)?,
        uploaded_at: row.get(6)?,
        status: row.get(7)?,
        page_id: row.get(8)?,
        page_url: row.get(9)?,
        message: row.get(10)?,
        display_mode: row.get(11)?,
        segment_count: segment_count.max(0) as usize,
        used_ffmpeg: used_ffmpeg != 0,
    })
}

fn row_to_drive_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriveNode> {
    let size: i64 = row.get(6)?;
    Ok(DriveNode {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        node_type: row.get(2)?,
        name: row.get(3)?,
        logical_path: row.get(4)?,
        mime_type: row.get(5)?,
        size: size.max(0) as u64,
        sha256: row.get(7)?,
        notion_page_id: row.get(8)?,
        notion_page_url: row.get(9)?,
        notion_block_id: row.get(10)?,
        file_upload_id: row.get(11)?,
        status: row.get(12)?,
        version: row.get::<_, i64>(13)?.max(1),
        original_path: row.get(14)?,
        created_at: row.get(15)?,
        modified_at: row.get(16)?,
    })
}

fn row_to_drive_transfer(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriveTransfer> {
    let total_bytes: i64 = row.get(6)?;
    let transferred_bytes: i64 = row.get(7)?;
    Ok(DriveTransfer {
        id: row.get(0)?,
        node_id: row.get(1)?,
        direction: row.get(2)?,
        file_name: row.get(3)?,
        local_path: row.get(4)?,
        status: row.get(5)?,
        total_bytes: total_bytes.max(0) as u64,
        transferred_bytes: transferred_bytes.max(0) as u64,
        message: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        initialize, insert_upload_record, row_to_upload_record, trim_upload_history,
        upsert_drive_node, upsert_drive_transfer,
    };
    use crate::models::{DriveNode, DriveTransfer, UploadRecord};
    use rusqlite::Connection;

    fn sample_record() -> UploadRecord {
        UploadRecord {
            id: "upload-1".to_string(),
            file_path: "C:/video.mp4".to_string(),
            file_name: "video.mp4".to_string(),
            size: 5_000_000_001,
            mime_type: "video/mp4".to_string(),
            sha256: "abc".to_string(),
            uploaded_at: "2026-08-03T12:00:00+08:00".to_string(),
            status: "success".to_string(),
            page_id: Some("page-id".to_string()),
            page_url: Some("https://www.notion.so/page".to_string()),
            message: Some("ok".to_string()),
            display_mode: "video".to_string(),
            segment_count: 2,
            used_ffmpeg: true,
        }
    }

    fn sample_node() -> DriveNode {
        DriveNode {
            id: "node-1".to_string(),
            parent_id: None,
            node_type: "folder".to_string(),
            name: "资料".to_string(),
            logical_path: "/资料".to_string(),
            mime_type: None,
            size: 0,
            sha256: None,
            notion_page_id: "page-1".to_string(),
            notion_page_url: None,
            notion_block_id: None,
            file_upload_id: None,
            status: "active".to_string(),
            version: 1,
            original_path: None,
            created_at: "2026-08-03T12:00:00Z".to_string(),
            modified_at: "2026-08-03T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn stores_and_reads_upload_record() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        insert_upload_record(&transaction, &sample_record()).unwrap();
        transaction.commit().unwrap();

        let restored = connection
            .query_row(
                "SELECT id, file_path, file_name, size, mime_type, sha256, uploaded_at, status,
                        page_id, page_url, message, display_mode, segment_count, used_ffmpeg
                 FROM upload_history WHERE id = 'upload-1'",
                [],
                row_to_upload_record,
            )
            .unwrap();

        assert_eq!(restored.file_name, "video.mp4");
        assert_eq!(restored.segment_count, 2);
        assert!(restored.used_ffmpeg);
    }

    #[test]
    fn trims_history_to_configured_limit() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        for index in 0..505 {
            let mut record = sample_record();
            record.id = format!("upload-{index}");
            record.uploaded_at = format!("2026-08-03T12:{:02}:00Z", index % 60);
            insert_upload_record(&transaction, &record).unwrap();
        }
        trim_upload_history(&transaction).unwrap();
        transaction.commit().unwrap();

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM upload_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 500);
    }

    #[test]
    fn stores_drive_nodes_and_transfers() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        let transaction = connection.transaction().unwrap();
        upsert_drive_node(&transaction, &sample_node()).unwrap();
        upsert_drive_transfer(
            &transaction,
            &DriveTransfer {
                id: "transfer-1".to_string(),
                node_id: Some("node-1".to_string()),
                direction: "upload".to_string(),
                file_name: "资料".to_string(),
                local_path: None,
                status: "completed".to_string(),
                total_bytes: 0,
                transferred_bytes: 0,
                message: None,
                created_at: "2026-08-03T12:00:00Z".to_string(),
                updated_at: "2026-08-03T12:00:00Z".to_string(),
            },
        )
        .unwrap();
        transaction.commit().unwrap();

        let node_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM drive_nodes", [], |row| row.get(0))
            .unwrap();
        let transfer_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM drive_transfers", [], |row| row.get(0))
            .unwrap();
        assert_eq!(node_count, 1);
        assert_eq!(transfer_count, 1);
    }
}
