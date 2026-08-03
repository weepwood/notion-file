from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise RuntimeError(f"pattern not found in {path}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


# Reduce SQLite connection churn during 200ms progress events.
replace_once(
    "src-tauri/src/drive/queue.rs",
    "use rusqlite::{params, Connection, OptionalExtension, Transaction};\nuse std::path::{Path, PathBuf};\nuse std::sync::atomic::{AtomicBool, Ordering};",
    "use rusqlite::{params, Connection, OptionalExtension, Transaction};\nuse std::collections::HashMap;\nuse std::path::{Path, PathBuf};\nuse std::sync::atomic::{AtomicBool, Ordering};\nuse std::sync::{Mutex, OnceLock};",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);",
    "static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);\nstatic PROGRESS_CACHE: OnceLock<Mutex<HashMap<String, (String, u64)>>> = OnceLock::new();",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "    if !queue_paused(&app)? {\n        spawn_worker(app);\n    }\n    Ok(())\n}",
    "    start_if_ready(&app);\n    Ok(())\n}\n\npub(super) fn start_if_ready(app: &AppHandle) {\n    if queue_paused(app).unwrap_or(true) || drive_context(app).is_err() {\n        return;\n    }\n    spawn_worker(app.clone());\n}",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "pub(super) fn resume(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    set_paused(app, false)?;\n    spawn_worker(app.clone());",
    "pub(super) fn resume(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    set_paused(app, false)?;\n    start_if_ready(app);",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "pub(super) fn persist_progress(\n    app: &AppHandle,\n    node_id: &str,\n    stage: &str,\n    transferred_bytes: u64,\n    total_bytes: u64,\n) {\n    let Ok(connection) = open(app) else {\n        return;\n    };",
    "pub(super) fn persist_progress(\n    app: &AppHandle,\n    node_id: &str,\n    stage: &str,\n    transferred_bytes: u64,\n    total_bytes: u64,\n) {\n    let should_persist = {\n        let cache = PROGRESS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));\n        let mut cache = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());\n        let entry = cache\n            .entry(node_id.to_string())\n            .or_insert_with(|| (String::new(), 0));\n        let should = entry.0 != stage\n            || transferred_bytes >= total_bytes\n            || transferred_bytes.saturating_sub(entry.1) >= PROGRESS_PERSIST_INTERVAL;\n        if should {\n            *entry = (stage.to_string(), transferred_bytes);\n        }\n        should\n    };\n    if !should_persist {\n        return;\n    }\n    let Ok(connection) = open(app) else {\n        return;\n    };",
)

# Resume pending queue after credentials or drive connection becomes available.
replace_once(
    "src-tauri/src/drive/mod.rs",
    "    Ok(DriveInitResult {\n        database_id,\n        data_source_id,\n        created,\n        node_count: nodes.len(),\n    })",
    "    queue::start_if_ready(app);\n    Ok(DriveInitResult {\n        database_id,\n        data_source_id,\n        created,\n        node_count: nodes.len(),\n    })",
)
replace_once(
    "src-tauri/src/drive/mod.rs",
    "pub fn queue_snapshot(app: &AppHandle) -> Result<DriveQueueSnapshot> {",
    "pub fn start_queue_if_ready(app: &AppHandle) {\n    queue::start_if_ready(app);\n}\n\npub fn queue_snapshot(app: &AppHandle) -> Result<DriveQueueSnapshot> {",
)
replace_once(
    "src-tauri/src/lib.rs",
    "#[tauri::command]\nfn save_notion_token(token: String) -> Result<(), String> {\n    storage::save_token(&token).map_err(|error| error.to_string())\n}",
    "#[tauri::command]\nfn save_notion_token(app: AppHandle, token: String) -> Result<(), String> {\n    storage::save_token(&token).map_err(|error| error.to_string())?;\n    drive::start_queue_if_ready(&app);\n    Ok(())\n}",
)

# Queue button can start an idle pending queue without first toggling pause.
replace_once(
    "src/App.tsx",
    "                  <button onClick={() => setQueuePaused(!uploadQueue.paused)}>\n                    {uploadQueue.paused ? <Play size={15} /> : <Pause size={15} />}\n                    {uploadQueue.paused ? \"继续队列\" : \"暂停队列\"}\n                  </button>",
    "                  <button\n                    onClick={() => setQueuePaused(uploadQueue.workerRunning && !uploadQueue.paused)}\n                  >\n                    {uploadQueue.paused || (!uploadQueue.workerRunning && uploadQueue.pendingCount > 0)\n                      ? <Play size={15} />\n                      : <Pause size={15} />}\n                    {uploadQueue.paused\n                      ? \"继续队列\"\n                      : !uploadQueue.workerRunning && uploadQueue.pendingCount > 0\n                        ? \"启动队列\"\n                        : \"暂停队列\"}\n                  </button>",
)
replace_once(
    "src/App.tsx",
    "{driveReady && <button className=\"danger\" onClick={disconnect}>断开本机索引</button>}",
    "{driveReady && <button className=\"danger\" onClick={disconnect} disabled={uploadQueue.workerRunning}>断开本机索引</button>}",
)

# Document exact v0.7.0 scope.
replace_once(
    "README.md",
    "- 当前版本采用单工作线程，优先保证 Notion API 安全性、顺序一致性和可恢复性。",
    "- 当前版本采用单工作线程，优先保证 Notion API 安全性、顺序一致性和可恢复性。\n- v0.7.0 首先覆盖“新文件上传”队列；上传新版本、传统同步和下载任务仍沿用各自现有流程。",
)
