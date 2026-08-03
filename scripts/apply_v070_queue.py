from pathlib import Path
import re


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    if old not in text:
        raise RuntimeError(f"pattern not found in {path}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8")


def regex_once(path: str, pattern: str, replacement: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"regex count {count} in {path}: {pattern[:100]!r}")
    p.write_text(updated, encoding="utf-8")


# Rust models
replace_once(
    "src-tauri/src/models.rs",
    "pub struct DriveUploadRequest {\n    pub file_path: String,\n    pub parent_id: Option<String>,\n}",
    "pub struct DriveUploadRequest {\n    pub file_path: String,\n    pub parent_id: Option<String>,\n    #[serde(default)]\n    pub node_id: Option<String>,\n}\n\n#[derive(Debug, Clone, Serialize, Deserialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct DriveQueueEnqueueRequest {\n    pub file_paths: Vec<String>,\n    pub parent_id: Option<String>,\n}\n\n#[derive(Debug, Clone, Serialize, Deserialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct DriveQueueJob {\n    pub id: String,\n    pub node_id: String,\n    pub parent_id: Option<String>,\n    pub file_path: String,\n    pub file_name: String,\n    pub size: u64,\n    pub status: String,\n    pub stage: String,\n    pub transferred_bytes: u64,\n    pub attempts: i64,\n    pub last_error: Option<String>,\n    pub created_at: String,\n    pub updated_at: String,\n    pub started_at: Option<String>,\n    pub completed_at: Option<String>,\n}\n\n#[derive(Debug, Clone, Serialize, Deserialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct DriveQueueSnapshot {\n    pub paused: bool,\n    pub worker_running: bool,\n    pub running_job_id: Option<String>,\n    pub pending_count: usize,\n    pub failed_count: usize,\n    pub jobs: Vec<DriveQueueJob>,\n}",
)

# Drive module
replace_once("src-tauri/src/drive/mod.rs", "mod notion_index;\n", "mod notion_index;\nmod queue;\n")
replace_once(
    "src-tauri/src/drive/mod.rs",
    "    DriveInitResult, DriveNode, DriveTransfer, DriveUploadRequest, DriveVersion,\n",
    "    DriveInitResult, DriveNode, DriveQueueEnqueueRequest, DriveQueueSnapshot, DriveTransfer,\n    DriveUploadRequest, DriveVersion,\n",
)
replace_once(
    "src-tauri/src/drive/mod.rs",
    "pub async fn upload_file(app: &AppHandle, request: DriveUploadRequest) -> Result<DriveNode> {\n    transfer::upload_file(app, request).await\n}\n",
    "pub async fn upload_file(app: &AppHandle, request: DriveUploadRequest) -> Result<DriveNode> {\n    transfer::upload_file(app, request).await\n}\n\npub fn recover_queue(app: AppHandle) -> Result<()> {\n    queue::recover_and_start(app)\n}\n\npub fn queue_snapshot(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    queue::snapshot(app)\n}\n\npub fn enqueue_uploads(\n    app: &AppHandle,\n    request: DriveQueueEnqueueRequest,\n) -> Result<DriveQueueSnapshot> {\n    queue::enqueue(app, request)\n}\n\npub fn pause_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    queue::pause(app)\n}\n\npub fn resume_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    queue::resume(app)\n}\n\npub fn retry_queue_job(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {\n    queue::retry(app, job_id)\n}\n\npub fn cancel_queue_job(app: &AppHandle, job_id: String) -> Result<DriveQueueSnapshot> {\n    queue::cancel(app, job_id)\n}\n\npub fn clear_finished_queue(app: &AppHandle) -> Result<DriveQueueSnapshot> {\n    queue::clear_finished(app)\n}\n",
)

# Deterministic node id and queue progress persistence
replace_once(
    "src-tauri/src/drive/transfer.rs",
    "    let node_id = new_id(\"node\");",
    "    let node_id = request.node_id.clone().unwrap_or_else(|| new_id(\"node\"));",
)
replace_once(
    "src-tauri/src/drive/transfer.rs",
    "pub(super) fn emit_progress_detailed(\n    app: &AppHandle,\n    transfer: &DriveTransfer,\n    stage: &str,\n    transferred_bytes: u64,\n    total_bytes: u64,\n    details: ProgressDetails,\n) {\n    let _ = app.emit(",
    "pub(super) fn emit_progress_detailed(\n    app: &AppHandle,\n    transfer: &DriveTransfer,\n    stage: &str,\n    transferred_bytes: u64,\n    total_bytes: u64,\n    details: ProgressDetails,\n) {\n    if transfer.direction == \"upload\" {\n        if let Some(node_id) = transfer.node_id.as_deref() {\n            super::queue::persist_progress(\n                app,\n                node_id,\n                &details.stage_code,\n                transferred_bytes,\n                total_bytes,\n            );\n        }\n    }\n    let _ = app.emit(",
)

# Tauri commands and startup recovery
replace_once(
    "src-tauri/src/lib.rs",
    "    DriveInitResult, DriveNode, DriveTransfer, DriveUploadRequest, DriveVersion,\n",
    "    DriveInitResult, DriveNode, DriveQueueEnqueueRequest, DriveQueueSnapshot, DriveTransfer,\n    DriveUploadRequest, DriveVersion,\n",
)
replace_once(
    "src-tauri/src/lib.rs",
    "#[tauri::command]\nasync fn download_drive_file(",
    "#[tauri::command]\nfn get_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {\n    drive::queue_snapshot(&app).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn enqueue_drive_uploads(\n    app: AppHandle,\n    request: DriveQueueEnqueueRequest,\n) -> Result<DriveQueueSnapshot, String> {\n    drive::enqueue_uploads(&app, request).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn pause_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {\n    drive::pause_queue(&app).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn resume_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {\n    drive::resume_queue(&app).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn retry_drive_upload_job(\n    app: AppHandle,\n    job_id: String,\n) -> Result<DriveQueueSnapshot, String> {\n    drive::retry_queue_job(&app, job_id).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn cancel_drive_upload_job(\n    app: AppHandle,\n    job_id: String,\n) -> Result<DriveQueueSnapshot, String> {\n    drive::cancel_queue_job(&app, job_id).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nfn clear_finished_drive_upload_queue(app: AppHandle) -> Result<DriveQueueSnapshot, String> {\n    drive::clear_finished_queue(&app).map_err(|error| error.to_string())\n}\n\n#[tauri::command]\nasync fn download_drive_file(",
)
replace_once(
    "src-tauri/src/lib.rs",
    "    tauri::Builder::default()\n        .plugin(tauri_plugin_dialog::init())",
    "    tauri::Builder::default()\n        .plugin(tauri_plugin_dialog::init())\n        .setup(|app| {\n            if let Err(error) = drive::recover_queue(app.handle().clone()) {\n                eprintln!(\"恢复持久化上传队列失败：{error}\");\n            }\n            Ok(())\n        })",
)
replace_once(
    "src-tauri/src/lib.rs",
    "            upload_drive_file,\n            download_drive_file,",
    "            upload_drive_file,\n            get_drive_upload_queue,\n            enqueue_drive_uploads,\n            pause_drive_upload_queue,\n            resume_drive_upload_queue,\n            retry_drive_upload_job,\n            cancel_drive_upload_job,\n            clear_finished_drive_upload_queue,\n            download_drive_file,",
)

# Frontend types
replace_once(
    "src/types.ts",
    "export interface DriveTransferProgress {",
    "export interface DriveQueueJob {\n  id: string;\n  nodeId: string;\n  parentId?: string;\n  filePath: string;\n  fileName: string;\n  size: number;\n  status: \"pending\" | \"running\" | \"completed\" | \"failed\" | \"cancelled\";\n  stage: string;\n  transferredBytes: number;\n  attempts: number;\n  lastError?: string;\n  createdAt: string;\n  updatedAt: string;\n  startedAt?: string;\n  completedAt?: string;\n}\n\nexport interface DriveQueueSnapshot {\n  paused: boolean;\n  workerRunning: boolean;\n  runningJobId?: string;\n  pendingCount: number;\n  failedCount: number;\n  jobs: DriveQueueJob[];\n}\n\nexport interface DriveTransferProgress {",
)

# App imports/state/listeners/data loading
replace_once("src/App.tsx", "  Move,\n", "  Move,\n  Pause,\n  Play,\n")
replace_once(
    "src/App.tsx",
    "  DriveNode,\n  DriveTransfer,",
    "  DriveNode,\n  DriveQueueJob,\n  DriveQueueSnapshot,\n  DriveTransfer,",
)
replace_once(
    "src/App.tsx",
    "type Notice = { type: \"success\" | \"error\" | \"info\"; text: string };",
    "type Notice = { type: \"success\" | \"error\" | \"info\"; text: string };\n\nconst EMPTY_QUEUE: DriveQueueSnapshot = {\n  paused: false,\n  workerRunning: false,\n  pendingCount: 0,\n  failedCount: 0,\n  jobs: [],\n};",
)
replace_once(
    "src/App.tsx",
    "function versionDownloadName(fileName: string, version: number): string {",
    "function queueStatusLabel(status: DriveQueueJob[\"status\"]): string {\n  return {\n    pending: \"等待中\",\n    running: \"上传中\",\n    completed: \"已完成\",\n    failed: \"失败\",\n    cancelled: \"已取消\",\n  }[status];\n}\n\nfunction versionDownloadName(fileName: string, version: number): string {",
)
replace_once(
    "src/App.tsx",
    "  const [transfers, setTransfers] = useState<DriveTransfer[]>([]);",
    "  const [transfers, setTransfers] = useState<DriveTransfer[]>([]);\n  const [uploadQueue, setUploadQueue] = useState<DriveQueueSnapshot>(EMPTY_QUEUE);",
)
regex_once(
    "src/App.tsx",
    r"  useEffect\(\(\) => \{\n    void loadLocalData\(\);\n    const unlisten = listen<DriveTransferProgress>\(\"drive-transfer-progress\", \(event\) => \{\n      setProgress\(event.payload\);\n    \}\);\n    return \(\) => \{\n      void unlisten.then\(\(dispose\) => dispose\(\)\);\n    \};\n  \}, \[\]\);",
    "  useEffect(() => {\n    void loadLocalData();\n    const progressListener = listen<DriveTransferProgress>(\"drive-transfer-progress\", (event) => {\n      setProgress(event.payload);\n      if (event.payload.nodeId) {\n        setUploadQueue((current) => ({\n          ...current,\n          jobs: current.jobs.map((job) =>\n            job.nodeId === event.payload.nodeId\n              ? {\n                  ...job,\n                  stage: event.payload.stageCode || event.payload.stage,\n                  transferredBytes: event.payload.transferredBytes,\n                  size: Math.max(job.size, event.payload.totalBytes),\n                  status: \"running\",\n                }\n              : job,\n          ),\n        }));\n      }\n    });\n    const queueListener = listen<DriveQueueSnapshot>(\"drive-queue-changed\", (event) => {\n      setUploadQueue(event.payload);\n      void refreshLocal();\n    });\n    const errorListener = listen<string>(\"drive-queue-error\", (event) => {\n      setNotice({ type: \"error\", text: `上传队列异常：${event.payload}` });\n    });\n    return () => {\n      void progressListener.then((dispose) => dispose());\n      void queueListener.then((dispose) => dispose());\n      void errorListener.then((dispose) => dispose());\n    };\n  }, []);",
)
replace_once(
    "src/App.tsx",
    "      const [saved, savedToken, savedNodes, savedTransfers] = await Promise.all([\n        invoke<AppConfig>(\"get_saved_config\"),\n        invoke<boolean>(\"has_saved_token\"),\n        invoke<DriveNode[]>(\"get_drive_nodes\", { includeTrashed: true }).catch(() => []),\n        invoke<DriveTransfer[]>(\"get_drive_transfers\").catch(() => []),\n      ]);",
    "      const [saved, savedToken, savedNodes, savedTransfers, savedQueue] = await Promise.all([\n        invoke<AppConfig>(\"get_saved_config\"),\n        invoke<boolean>(\"has_saved_token\"),\n        invoke<DriveNode[]>(\"get_drive_nodes\", { includeTrashed: true }).catch(() => []),\n        invoke<DriveTransfer[]>(\"get_drive_transfers\").catch(() => []),\n        invoke<DriveQueueSnapshot>(\"get_drive_upload_queue\").catch(() => EMPTY_QUEUE),\n      ]);",
)
replace_once(
    "src/App.tsx",
    "      setTransfers(savedTransfers);",
    "      setTransfers(savedTransfers);\n      setUploadQueue(savedQueue);",
)
replace_once(
    "src/App.tsx",
    "    const [savedNodes, savedTransfers, savedConfig] = await Promise.all([\n      invoke<DriveNode[]>(\"get_drive_nodes\", { includeTrashed: true }),\n      invoke<DriveTransfer[]>(\"get_drive_transfers\"),\n      invoke<AppConfig>(\"get_saved_config\"),\n    ]);",
    "    const [savedNodes, savedTransfers, savedConfig, savedQueue] = await Promise.all([\n      invoke<DriveNode[]>(\"get_drive_nodes\", { includeTrashed: true }),\n      invoke<DriveTransfer[]>(\"get_drive_transfers\"),\n      invoke<AppConfig>(\"get_saved_config\"),\n      invoke<DriveQueueSnapshot>(\"get_drive_upload_queue\").catch(() => EMPTY_QUEUE),\n    ]);",
)
replace_once(
    "src/App.tsx",
    "    setConfig({ ...DEFAULT_CONFIG, ...savedConfig });",
    "    setConfig({ ...DEFAULT_CONFIG, ...savedConfig });\n    setUploadQueue(savedQueue);",
)

# Queue-based upload action
regex_once(
    "src/App.tsx",
    r"  async function uploadFiles\(\) \{.*?\n  \}\n\n  async function downloadSelected\(\)",
    "  async function uploadFiles() {\n    const chosen = await open({\n      directory: false,\n      multiple: true,\n      title: \"选择需要上传到 Notion Drive 的文件\",\n    });\n    const paths = Array.isArray(chosen) ? chosen : typeof chosen === \"string\" ? [chosen] : [];\n    if (!paths.length) return;\n    try {\n      const next = await invoke<DriveQueueSnapshot>(\"enqueue_drive_uploads\", {\n        request: { filePaths: paths, parentId: folderId },\n      });\n      setUploadQueue(next);\n      setView(\"transfers\");\n      setNotice({\n        type: \"success\",\n        text: `已将 ${paths.length} 个文件写入持久化队列，切换页面或重启应用后任务仍会保留。`,\n      });\n    } catch (error) {\n      setNotice({ type: \"error\", text: String(error) });\n    }\n  }\n\n  async function downloadSelected()",
)

# Queue actions before clearTransfers
replace_once(
    "src/App.tsx",
    "  async function clearTransfers() {",
    "  async function setQueuePaused(paused: boolean) {\n    try {\n      const next = await invoke<DriveQueueSnapshot>(\n        paused ? \"pause_drive_upload_queue\" : \"resume_drive_upload_queue\",\n      );\n      setUploadQueue(next);\n      setNotice({\n        type: \"info\",\n        text: paused\n          ? \"上传队列已暂停；当前正在发送的文件会完成，之后不再启动新任务。\"\n          : \"上传队列已继续执行。\",\n      });\n    } catch (error) {\n      setNotice({ type: \"error\", text: String(error) });\n    }\n  }\n\n  async function retryQueueJob(jobId: string) {\n    try {\n      const next = await invoke<DriveQueueSnapshot>(\"retry_drive_upload_job\", { jobId });\n      setUploadQueue(next);\n    } catch (error) {\n      setNotice({ type: \"error\", text: String(error) });\n    }\n  }\n\n  async function cancelQueueJob(jobId: string) {\n    try {\n      const next = await invoke<DriveQueueSnapshot>(\"cancel_drive_upload_job\", { jobId });\n      setUploadQueue(next);\n    } catch (error) {\n      setNotice({ type: \"error\", text: String(error) });\n    }\n  }\n\n  async function clearFinishedQueue() {\n    try {\n      const next = await invoke<DriveQueueSnapshot>(\"clear_finished_drive_upload_queue\");\n      setUploadQueue(next);\n    } catch (error) {\n      setNotice({ type: \"error\", text: String(error) });\n    }\n  }\n\n  async function clearTransfers() {",
)

replace_once(
    "src/App.tsx",
    "          {busy && <div className=\"busy-label\"><LoaderCircle size={15} className=\"spin\" />正在处理</div>}",
    "          {(busy || uploadQueue.workerRunning) && <div className=\"busy-label\"><LoaderCircle size={15} className=\"spin\" />{busy ? \"正在处理\" : \"后台上传中\"}</div>}",
)
replace_once(
    "src/App.tsx",
    "          {progress && busy && <div className=\"transfer-banner diagnostic-banner\">",
    "          {progress && (busy || uploadQueue.workerRunning) && <div className=\"transfer-banner diagnostic-banner\">",
)

# Replace transfer center section
regex_once(
    "src/App.tsx",
    r"          \{view === \"transfers\" && <section>.*?</section>\}\n\n          \{view === \"settings\"",
    '''          {view === "transfers" && <section>
            <div className="section-heading">
              <div><h1>传输中心</h1><p>上传任务写入 SQLite 后由 Rust 后台顺序执行，应用重启会自动恢复未完成任务。</p></div>
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
              <div><strong>{uploadQueue.paused ? "队列已暂停" : uploadQueue.workerRunning ? "队列执行中" : "队列空闲"}</strong><span>等待 {uploadQueue.pendingCount} · 失败 {uploadQueue.failedCount}</span></div>
              <small>暂停会在当前文件结束后生效；正在发送的 HTTP 请求不会被强行中断。</small>
            </div>
            <div className="queue-list">
              {uploadQueue.jobs.length === 0 ? <div className="empty-state"><Upload size={36} /><strong>暂无持久化上传任务</strong></div> : uploadQueue.jobs.map((job) => {
                const percent = job.size ? Math.min(100, Math.round(job.transferredBytes / job.size * 100)) : 0;
                return <div className={`queue-item ${job.status}`} key={job.id}>
                  <i><Upload size={17} /></i>
                  <div className="queue-copy"><strong>{job.fileName}</strong><span>{job.lastError ?? job.filePath}</span><div className="mini-progress"><div style={{ width: `${percent}%` }} /></div></div></div>
                  <div className="queue-size">{formatBytes(job.transferredBytes)} / {formatBytes(job.size)}</div>
                  <div className="queue-attempts">尝试 {job.attempts}</div>
                  <em className={`transfer-status ${job.status}`}>{queueStatusLabel(job.status)}</em>
                  <div className="queue-actions">
                    {(job.status === "failed" || job.status === "cancelled") && <button onClick={() => retryQueueJob(job.id)}><RefreshCw size={12} />重试</button>}
                    {(job.status === "pending" || job.status === "failed") && <button onClick={() => cancelQueueJob(job.id)}><X size={12} />取消</button>}
                  </div>
                </div>;
              })}
            </div>
            <div className="section-heading transfer-history-heading"><div><h2>传输历史</h2><p>下载失败会保留 .part 临时文件，可从已完成字节继续。</p></div></div>
            <div className="transfer-list">{transfers.length === 0 ? <div className="empty-state"><History size={36} /><strong>暂无传输记录</strong></div> : transfers.map((transfer) => { const percent = transfer.totalBytes ? Math.min(100, Math.round(transfer.transferredBytes / transfer.totalBytes * 100)) : 0; const resumable = transfer.direction === "download" && transfer.status === "failed" && Boolean(transfer.localPath) && Boolean(transfer.nodeId); return <div className={`transfer-item ${resumable ? "has-action" : ""}`} key={transfer.id}><i>{transfer.direction === "upload" ? <Upload size={17} /> : <Download size={17} />}</i><div className="transfer-copy"><strong>{transfer.fileName}</strong><span>{transfer.message ?? transfer.localPath ?? ""}</span><div className="mini-progress"><div style={{ width: `${percent}%` }} /></div></div><div className="transfer-size">{formatBytes(transfer.transferredBytes)} / {formatBytes(transfer.totalBytes)}</div><em className={`transfer-status ${transfer.status}`}>{transferLabel(transfer.status)}</em>{resumable && <button className="transfer-retry" onClick={() => retryTransfer(transfer)} disabled={Boolean(busy)}><RefreshCw size={12} />续传</button>}</div>; })}</div>
          </section>}

          {view === "settings"''',
)

# Queue CSS
Path("src/advanced.css").write_text(
    Path("src/advanced.css").read_text(encoding="utf-8")
    + '''\n\n/* v0.7.0 persistent upload queue */\n.queue-toolbar { display: flex; gap: 8px; flex-wrap: wrap; }\n.queue-summary { display: flex; justify-content: space-between; gap: 16px; align-items: center; padding: 14px 16px; margin-bottom: 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--surface); }\n.queue-summary.paused { border-color: #d9a441; background: rgba(217, 164, 65, 0.08); }\n.queue-summary div { display: flex; flex-direction: column; gap: 3px; }\n.queue-summary span, .queue-summary small { color: var(--muted); }\n.queue-list { display: flex; flex-direction: column; gap: 8px; margin-bottom: 26px; }\n.queue-item { display: grid; grid-template-columns: 28px minmax(240px, 1fr) 130px 72px 76px auto; gap: 12px; align-items: center; padding: 12px 14px; border: 1px solid var(--border); border-radius: 10px; background: var(--surface); }\n.queue-item.running { border-color: rgba(35, 131, 226, 0.45); }\n.queue-item.failed { border-color: rgba(235, 87, 87, 0.45); }\n.queue-copy { min-width: 0; display: flex; flex-direction: column; gap: 5px; }\n.queue-copy strong, .queue-copy span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }\n.queue-copy span, .queue-size, .queue-attempts { color: var(--muted); font-size: 12px; }\n.queue-actions { display: flex; gap: 6px; }\n.queue-actions button { padding: 6px 8px; font-size: 12px; }\n.transfer-history-heading { margin-top: 10px; }\n@media (max-width: 980px) { .queue-item { grid-template-columns: 24px 1fr auto; } .queue-size, .queue-attempts { display: none; } .queue-actions { grid-column: 2 / -1; } .queue-summary { align-items: flex-start; flex-direction: column; } }\n''',
    encoding="utf-8",
)

# Versions
for path in ["package.json", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json"]:
    p = Path(path)
    p.write_text(p.read_text(encoding="utf-8").replace('0.6.1', '0.7.0'), encoding="utf-8")
replace_once("src/App.tsx", "个人云盘 · v0.6.1", "个人云盘 · v0.7.0")

# README
replace_once(
    "README.md",
    "## v0.6.1：上传速度与瓶颈诊断",
    "## v0.7.0：持久化上传队列\n\n- 多文件上传先写入 SQLite，再由 Rust 后台工作线程顺序执行。\n- 切换页面或关闭应用不会丢失等待任务；下次启动会将中断任务恢复为等待状态。\n- 支持全局暂停/继续、单任务失败重试、等待任务取消和已完成记录清理。\n- 暂停只阻止下一项启动，当前正在发送的文件会安全完成。\n- 每个任务预先分配固定 Node ID；重试时先检查远端索引，并尝试修复已创建页面但缺少文件块的中间状态。\n- 队列进度约每 1 MiB 持久化一次，实时速度仍通过传输事件约每 200 ms 更新。\n- 当前版本采用单工作线程，优先保证 Notion API 安全性、顺序一致性和可恢复性。\n\n## v0.6.1：上传速度与瓶颈诊断",
)
