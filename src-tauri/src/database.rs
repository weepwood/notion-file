use crate::models::UploadRecord;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const DATABASE_FILE_NAME: &str = "notion-file.sqlite3";
const LEGACY_MIGRATION_KEY: &str = "upload_history_json_migrated";
const MAX_UPLOAD_HISTORY: usize = 500;

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

             PRAGMA user_version = 1;",
        )
        .context("无法初始化 SQLite 数据库结构")?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{initialize, insert_upload_record, row_to_upload_record, trim_upload_history};
    use crate::models::UploadRecord;
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
}
