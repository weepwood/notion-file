use crate::models::{DriveNode, DriveVersion};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

const DATABASE_FILE_NAME: &str = "notion-file.sqlite3";

pub(super) fn list_versions(app: &AppHandle, node_id: &str) -> Result<Vec<DriveVersion>> {
    let connection = open(app)?;
    let mut statement = connection.prepare(
        "SELECT id, node_id, version, size, sha256, mime_type, file_upload_id,
                notion_block_id, original_path, created_at
         FROM drive_versions
         WHERE node_id = ?1
         ORDER BY version DESC, created_at DESC",
    )?;
    let versions = statement
        .query_map([node_id], row_to_version)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(versions)
}

pub(super) fn get_version(app: &AppHandle, version_id: &str) -> Result<DriveVersion> {
    let connection = open(app)?;
    connection
        .query_row(
            "SELECT id, node_id, version, size, sha256, mime_type, file_upload_id,
                    notion_block_id, original_path, created_at
             FROM drive_versions WHERE id = ?1",
            [version_id],
            row_to_version,
        )
        .optional()?
        .with_context(|| format!("找不到文件版本：{version_id}"))
}

pub(super) fn upsert_version(app: &AppHandle, version: &DriveVersion) -> Result<()> {
    let connection = open(app)?;
    let size = i64::try_from(version.size).context("版本文件大小超过 SQLite 整数范围")?;
    connection.execute(
        "INSERT INTO drive_versions(
             id, node_id, version, size, sha256, mime_type, file_upload_id,
             notion_block_id, original_path, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             node_id = excluded.node_id,
             version = excluded.version,
             size = excluded.size,
             sha256 = excluded.sha256,
             mime_type = excluded.mime_type,
             file_upload_id = excluded.file_upload_id,
             notion_block_id = excluded.notion_block_id,
             original_path = excluded.original_path,
             created_at = excluded.created_at",
        params![
            version.id,
            version.node_id,
            version.version,
            size,
            version.sha256,
            version.mime_type,
            version.file_upload_id,
            version.notion_block_id,
            version.original_path,
            version.created_at,
        ],
    )?;
    Ok(())
}

pub(super) fn ensure_current_version(app: &AppHandle, node: &DriveNode) -> Result<Option<DriveVersion>> {
    if node.node_type != "file" {
        return Ok(None);
    }
    let Some(sha256) = node.sha256.clone() else {
        return Ok(None);
    };
    let Some(file_upload_id) = node.file_upload_id.clone() else {
        return Ok(None);
    };
    let Some(notion_block_id) = node.notion_block_id.clone() else {
        return Ok(None);
    };
    let version = DriveVersion {
        id: version_id(&node.id, node.version),
        node_id: node.id.clone(),
        version: node.version,
        size: node.size,
        sha256,
        mime_type: node
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        file_upload_id,
        notion_block_id,
        original_path: node.original_path.clone(),
        created_at: node.modified_at.clone(),
    };
    upsert_version(app, &version)?;
    Ok(Some(version))
}

pub(super) fn version_id(node_id: &str, version: i64) -> String {
    format!("{node_id}-v{version}")
}

fn database_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join(DATABASE_FILE_NAME))
}

fn open(app: &AppHandle) -> Result<Connection> {
    let path = database_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("无法创建版本数据库目录")?;
    }
    let connection = Connection::open(&path)
        .with_context(|| format!("无法打开版本数据库：{}", path.display()))?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS drive_versions (
             id TEXT PRIMARY KEY NOT NULL,
             node_id TEXT NOT NULL,
             version INTEGER NOT NULL CHECK (version >= 1),
             size INTEGER NOT NULL CHECK (size >= 0),
             sha256 TEXT NOT NULL,
             mime_type TEXT NOT NULL,
             file_upload_id TEXT NOT NULL,
             notion_block_id TEXT NOT NULL,
             original_path TEXT,
             created_at TEXT NOT NULL,
             UNIQUE(node_id, version)
         );
         CREATE INDEX IF NOT EXISTS idx_drive_versions_node
             ON drive_versions(node_id, version DESC);",
    )?;
    Ok(connection)
}

fn row_to_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriveVersion> {
    let size: i64 = row.get(3)?;
    Ok(DriveVersion {
        id: row.get(0)?,
        node_id: row.get(1)?,
        version: row.get(2)?,
        size: u64::try_from(size.max(0)).unwrap_or(0),
        sha256: row.get(4)?,
        mime_type: row.get(5)?,
        file_upload_id: row.get(6)?,
        notion_block_id: row.get(7)?,
        original_path: row.get(8)?,
        created_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::version_id;

    #[test]
    fn creates_stable_version_ids() {
        assert_eq!(version_id("node-1", 3), "node-1-v3");
    }
}
