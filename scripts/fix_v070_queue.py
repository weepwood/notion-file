from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise RuntimeError(f"pattern not found in {path}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


def regex_once(path: str, pattern: str, replacement: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"regex count {count} in {path}")
    p.write_text(updated, encoding="utf-8")


# Startup-only interrupted transfer recovery.
replace_once(
    "src-tauri/src/drive/mod.rs",
    "pub fn list_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {\n    let now = Utc::now().to_rfc3339();\n    storage::mark_interrupted_transfers(app, &now)?;\n    storage::list_drive_transfers(app)\n}",
    "pub fn list_transfers(app: &AppHandle) -> Result<Vec<DriveTransfer>> {\n    storage::list_drive_transfers(app)\n}",
)
replace_once(
    "src-tauri/src/drive/mod.rs",
    "pub fn recover_queue(app: AppHandle) -> Result<()> {\n    queue::recover_and_start(app)\n}",
    "pub fn recover_queue(app: AppHandle) -> Result<()> {\n    storage::mark_interrupted_transfers(&app, &Utc::now().to_rfc3339())?;\n    queue::recover_and_start(app)\n}",
)

# Queue should wait for credentials instead of failing all pending jobs.
replace_once(
    "src-tauri/src/drive/queue.rs",
    "    loop {\n        if queue_paused(app)? {\n            break;\n        }",
    "    loop {\n        if queue_paused(app)? || drive_context(app).is_err() {\n            break;\n        }",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "    if node.notion_block_id.as_deref().is_none_or(str::is_empty) {",
    "    if node\n        .notion_block_id\n        .as_deref()\n        .map(str::is_empty)\n        .unwrap_or(true)\n    {",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "    let count: i64 = transaction.query_row(\n        \"SELECT COUNT(*) FROM drive_upload_queue",
    "    let path_text = path.to_string_lossy().to_string();\n    let count: i64 = transaction.query_row(\n        \"SELECT COUNT(*) FROM drive_upload_queue",
)
replace_once(
    "src-tauri/src/drive/queue.rs",
    "        params![path.to_string_lossy(), parent_id],",
    "        params![path_text, parent_id],",
)

# Tauri setup uses Manager::handle.
replace_once("src-tauri/src/lib.rs", "use tauri::AppHandle;", "use tauri::{AppHandle, Manager};")

# Extract complex queue rows into render functions.
replace_once(
    "src/App.tsx",
    "  const progressPercent = progress?.totalBytes",
    '''  function renderQueueJob(job: DriveQueueJob) {
    const percent = job.size
      ? Math.min(100, Math.round((job.transferredBytes / job.size) * 100))
      : 0;
    return (
      <div className={`queue-item ${job.status}`} key={job.id}>
        <i><Upload size={17} /></i>
        <div className="queue-copy">
          <strong>{job.fileName}</strong>
          <span>{job.lastError ?? job.filePath}</span>
          <div className="mini-progress"><div style={{ width: `${percent}%` }} /></div>
        </div>
        <div className="queue-size">{formatBytes(job.transferredBytes)} / {formatBytes(job.size)}</div>
        <div className="queue-attempts">尝试 {job.attempts}</div>
        <em className={`transfer-status ${job.status}`}>{queueStatusLabel(job.status)}</em>
        <div className="queue-actions">
          {(job.status === "failed" || job.status === "cancelled") && (
            <button onClick={() => retryQueueJob(job.id)}><RefreshCw size={12} />重试</button>
          )}
          {(job.status === "pending" || job.status === "failed") && (
            <button onClick={() => cancelQueueJob(job.id)}><X size={12} />取消</button>
          )}
        </div>
      </div>
    );
  }

  function renderTransferHistory(transfer: DriveTransfer) {
    const percent = transfer.totalBytes
      ? Math.min(100, Math.round((transfer.transferredBytes / transfer.totalBytes) * 100))
      : 0;
    const resumable =
      transfer.direction === "download" &&
      transfer.status === "failed" &&
      Boolean(transfer.localPath) &&
      Boolean(transfer.nodeId);
    return (
      <div className={`transfer-item ${resumable ? "has-action" : ""}`} key={transfer.id}>
        <i>{transfer.direction === "upload" ? <Upload size={17} /> : <Download size={17} />}</i>
        <div className="transfer-copy">
          <strong>{transfer.fileName}</strong>
          <span>{transfer.message ?? transfer.localPath ?? ""}</span>
          <div className="mini-progress"><div style={{ width: `${percent}%` }} /></div>
        </div>
        <div className="transfer-size">{formatBytes(transfer.transferredBytes)} / {formatBytes(transfer.totalBytes)}</div>
        <em className={`transfer-status ${transfer.status}`}>{transferLabel(transfer.status)}</em>
        {resumable && (
          <button className="transfer-retry" onClick={() => retryTransfer(transfer)} disabled={Boolean(busy)}>
            <RefreshCw size={12} />续传
          </button>
        )}
      </div>
    );
  }

  const progressPercent = progress?.totalBytes''',
)

regex_once(
    "src/App.tsx",
    r"          \{view === \"transfers\" && <section>.*?</section>\}\n\n          \{view === \"settings\"",
    '''          {view === "transfers" && (
            <section>
              <div className="section-heading">
                <div>
                  <h1>传输中心</h1>
                  <p>上传任务写入 SQLite 后由 Rust 后台顺序执行，应用重启会自动恢复未完成任务。</p>
                </div>
                <div className="queue-toolbar">
                  <button onClick={() => setQueuePaused(!uploadQueue.paused)}>
                    {uploadQueue.paused ? <Play size={15} /> : <Pause size={15} />}
                    {uploadQueue.paused ? "继续队列" : "暂停队列"}
                  </button>
                  <button onClick={clearFinishedQueue}><Trash2 size={15} />清理已完成队列</button>
                  <button onClick={clearTransfers}><Trash2 size={15} />清理传输记录</button>
                </div>
              </div>
              <div className={`queue-summary ${uploadQueue.paused ? "paused" : ""}`}>
                <div>
                  <strong>{uploadQueue.paused ? "队列已暂停" : uploadQueue.workerRunning ? "队列执行中" : "队列空闲"}</strong>
                  <span>等待 {uploadQueue.pendingCount} · 失败 {uploadQueue.failedCount}</span>
                </div>
                <small>暂停会在当前文件结束后生效；正在发送的 HTTP 请求不会被强行中断。</small>
              </div>
              <div className="queue-list">
                {uploadQueue.jobs.length === 0 ? (
                  <div className="empty-state"><Upload size={36} /><strong>暂无持久化上传任务</strong></div>
                ) : (
                  uploadQueue.jobs.map(renderQueueJob)
                )}
              </div>
              <div className="section-heading transfer-history-heading">
                <div><h2>传输历史</h2><p>下载失败会保留 .part 临时文件，可从已完成字节继续。</p></div>
              </div>
              <div className="transfer-list">
                {transfers.length === 0 ? (
                  <div className="empty-state"><History size={36} /><strong>暂无传输记录</strong></div>
                ) : (
                  transfers.map(renderTransferHistory)
                )}
              </div>
            </section>
          )}

          {view === "settings"''',
)
