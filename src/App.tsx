import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  ArchiveRestore,
  Check,
  Clock3,
  DatabaseBackup,
  File,
  FileText,
  FolderOpen,
  HardDriveUpload,
  History,
  Image,
  LoaderCircle,
  Plus,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import type {
  AppConfig,
  BackupJob,
  BackupProgress,
  BackupResult,
  BackupSnapshot,
  RestoreResult,
  ScanResult,
} from "./types";

const DEFAULT_JOB: BackupJob = {
  id: "default",
  name: "本地文件备份",
  folderPath: "",
  rootPageId: "",
  skipHidden: true,
  includeTextPreview: true,
  autoBackupMinutes: 0,
  enabled: true,
  lastBackupAt: null,
};

const DEFAULT_CONFIG: AppConfig = { jobs: [DEFAULT_JOB], activeJobId: "default" };

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatTime(value?: string | null): string {
  if (!value) return "尚未备份";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function fileIcon(mimeType: string) {
  if (mimeType.startsWith("image/")) return <Image size={17} />;
  if (mimeType.startsWith("text/") || mimeType.includes("json") || mimeType.includes("markdown")) return <FileText size={17} />;
  return <File size={17} />;
}

export default function App() {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [backupResult, setBackupResult] = useState<BackupResult | null>(null);
  const [restoreResult, setRestoreResult] = useState<RestoreResult | null>(null);
  const [history, setHistory] = useState<BackupSnapshot[]>([]);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [busy, setBusy] = useState<"scan" | "backup" | "restore" | "test" | "save" | null>(null);
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);
  const busyRef = useRef(false);

  const activeJob = useMemo(
    () => config.jobs.find((job) => job.id === config.activeJobId) ?? config.jobs[0] ?? DEFAULT_JOB,
    [config],
  );

  useEffect(() => {
    Promise.all([invoke<AppConfig>("get_saved_config"), invoke<boolean>("has_saved_token")])
      .then(([saved, tokenSaved]) => {
        const normalized = saved.jobs?.length ? saved : DEFAULT_CONFIG;
        setConfig(normalized);
        setHasToken(tokenSaved);
      })
      .catch((error) => setMessage({ type: "error", text: String(error) }));
    const unlisten = listen<BackupProgress>("backup-progress", (event) => setProgress(event.payload));
    return () => { void unlisten.then((dispose) => dispose()); };
  }, []);

  useEffect(() => {
    setScan(null);
    setBackupResult(null);
    setRestoreResult(null);
    if (!activeJob.id) return;
    invoke<BackupSnapshot[]>("get_backup_history", { jobId: activeJob.id })
      .then(setHistory)
      .catch(() => setHistory([]));
  }, [activeJob.id]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (busyRef.current || !hasToken) return;
      const now = Date.now();
      const due = config.jobs.find((job) => {
        if (!job.enabled || job.autoBackupMinutes <= 0 || !job.folderPath || !job.rootPageId) return false;
        const last = job.lastBackupAt ? new Date(job.lastBackupAt).getTime() : 0;
        return !last || now - last >= job.autoBackupMinutes * 60_000;
      });
      if (due) void executeBackup(due, true);
    }, 60_000);
    return () => window.clearInterval(timer);
  }, [config.jobs, hasToken]);

  const progressPercent = useMemo(
    () => !progress || progress.total === 0 ? 0 : Math.round((progress.current / progress.total) * 100),
    [progress],
  );

  function updateJob(patch: Partial<BackupJob>) {
    setConfig((current) => ({
      ...current,
      jobs: current.jobs.map((job) => job.id === activeJob.id ? { ...job, ...patch } : job),
    }));
  }

  function addJob() {
    const id = `job-${Date.now()}`;
    const next: BackupJob = { ...DEFAULT_JOB, id, name: `备份任务 ${config.jobs.length + 1}` };
    setConfig((current) => ({ ...current, jobs: [...current.jobs, next], activeJobId: id }));
  }

  function deleteJob() {
    if (config.jobs.length <= 1 || !window.confirm(`删除任务“${activeJob.name}”？远端 Notion 备份不会被删除。`)) return;
    setConfig((current) => {
      const jobs = current.jobs.filter((job) => job.id !== activeJob.id);
      return { ...current, jobs, activeJobId: jobs[0]?.id ?? null };
    });
  }

  async function chooseFolder() {
    const selected = await open({ directory: true, multiple: false, title: "选择需要备份的文件夹" });
    if (typeof selected === "string") updateJob({ folderPath: selected });
  }

  async function persistToken() {
    if (!token.trim()) return;
    await invoke("save_notion_token", { token: token.trim() });
    setHasToken(true);
    setToken("");
  }

  async function saveSettings() {
    setBusyState("save");
    setMessage(null);
    try {
      await invoke("save_config", { config });
      await persistToken();
      setMessage({ type: "success", text: "备份任务和凭据已保存到本机。" });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusyState(null);
    }
  }

  async function testConnection() {
    setBusyState("test");
    setMessage(null);
    try {
      await persistToken();
      const title = await invoke<string>("test_notion_connection", { rootPageId: activeJob.rootPageId.trim() });
      setMessage({ type: "success", text: `连接成功：${title || "目标页面可访问"}` });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusyState(null);
    }
  }

  async function scanFolder(showNotice = true) {
    setBusyState("scan");
    setMessage(null);
    setBackupResult(null);
    setRestoreResult(null);
    try {
      const result = await invoke<ScanResult>("scan_backup", { job: activeJob });
      setScan(result);
      if (showNotice) setMessage({ type: "success", text: `扫描完成：${result.changedCount} 个待上传，${result.deletedCount} 个本地删除记录。` });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusyState(null);
    }
  }

  async function executeBackup(job: BackupJob, automatic = false) {
    setBusyState("backup");
    setMessage(null);
    setRestoreResult(null);
    setProgress({ current: 0, total: 0, relativePath: "", stage: automatic ? "自动备份准备中" : "备份准备中" });
    try {
      await invoke("save_config", { config });
      const result = await invoke<BackupResult>("run_backup", { job });
      setBackupResult(result);
      setMessage({
        type: result.failed > 0 ? "error" : "success",
        text: `${automatic ? "自动" : "手动"}备份完成：上传 ${result.uploaded}，未变化 ${result.unchanged}，保留删除记录 ${result.markedDeleted}，失败 ${result.failed}。`,
      });
      const completedAt = result.finishedAt;
      setConfig((current) => {
        const next = { ...current, jobs: current.jobs.map((item) => item.id === job.id ? { ...item, lastBackupAt: completedAt } : item) };
        void invoke("save_config", { config: next });
        return next;
      });
      setHistory(await invoke<BackupSnapshot[]>("get_backup_history", { jobId: job.id }));
      if (job.id === activeJob.id) setScan(await invoke<ScanResult>("scan_backup", { job: { ...job, lastBackupAt: completedAt } }));
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusyState(null);
      setProgress(null);
    }
  }

  async function restoreLatest() {
    const destination = await open({ directory: true, multiple: false, title: "选择恢复文件的目标文件夹" });
    if (typeof destination !== "string") return;
    const overwrite = window.confirm("目标目录中已有同名文件时是否覆盖？\n选择“取消”将跳过已有文件。 ");
    setBusyState("restore");
    setMessage(null);
    setBackupResult(null);
    try {
      const result = await invoke<RestoreResult>("restore_backup", {
        request: { jobId: activeJob.id, destinationPath: destination, overwrite },
      });
      setRestoreResult(result);
      setMessage({
        type: result.failed > 0 ? "error" : "success",
        text: `恢复完成：成功 ${result.restored}，跳过 ${result.skipped}，失败 ${result.failed}。`,
      });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusyState(null);
      setProgress(null);
    }
  }

  function setBusyState(value: typeof busy) {
    busyRef.current = value !== null;
    setBusy(value);
  }

  const canConnect = Boolean(activeJob.rootPageId.trim() && (hasToken || token.trim()));
  const canScan = Boolean(activeJob.folderPath.trim());
  const canBackup = Boolean(canConnect && canScan && !busy);

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-mark"><DatabaseBackup size={24} /></div>
        <div><h1>Notion Backup</h1><p>将本地文件原件增量备份到 Notion，并保留历史版本</p></div>
        <div className={`connection-badge ${hasToken ? "connected" : ""}`}><ShieldCheck size={15} />{hasToken ? "凭据已保存" : "尚未配置凭据"}</div>
      </header>

      {message && <div className={`notice ${message.type}`}>{message.type === "success" ? <Check size={18} /> : <AlertCircle size={18} />}<span>{message.text}</span></div>}

      <div className="layout">
        <aside className="sidebar panel">
          <div className="sidebar-heading"><span>备份任务</span><button className="icon-button" onClick={addJob} title="新增任务"><Plus size={17} /></button></div>
          <div className="job-list">
            {config.jobs.map((job) => <button key={job.id} className={`job-item ${job.id === activeJob.id ? "active" : ""}`} onClick={() => setConfig((current) => ({ ...current, activeJobId: job.id }))}>
              <HardDriveUpload size={17} /><span><strong>{job.name}</strong><small>{job.autoBackupMinutes > 0 ? `每 ${job.autoBackupMinutes} 分钟` : "手动备份"}</small></span>
            </button>)}
          </div>
          <div className="credential-box">
            <label><span>Notion Integration Token</span><input type="password" value={token} placeholder={hasToken ? "已保存，留空则不修改" : "ntn_..."} onChange={(event) => setToken(event.target.value)} /></label>
            <button className="secondary full" onClick={saveSettings} disabled={Boolean(busy)}>{busy === "save" ? <LoaderCircle className="spin" size={17} /> : <Save size={17} />}保存全部设置</button>
          </div>
        </aside>

        <section className="content">
          <section className="panel settings-card">
            <div className="section-header"><div><h2>{activeJob.name}</h2><p>每个文件在 Notion 中对应一个页面；修改会追加新版本，不覆盖旧备份。</p></div><button className="danger-ghost" onClick={deleteJob} disabled={config.jobs.length <= 1 || Boolean(busy)}><Trash2 size={16} />删除任务</button></div>
            <div className="form-grid">
              <label><span>任务名称</span><input value={activeJob.name} onChange={(event) => updateJob({ name: event.target.value })} /></label>
              <label><span>自动备份</span><select value={activeJob.autoBackupMinutes} onChange={(event) => updateJob({ autoBackupMinutes: Number(event.target.value) })}><option value={0}>仅手动</option><option value={15}>每 15 分钟</option><option value={30}>每 30 分钟</option><option value={60}>每小时</option><option value={180}>每 3 小时</option><option value={1440}>每天</option></select></label>
              <label className="wide"><span>目标 Notion 根页面 ID</span><input value={activeJob.rootPageId} placeholder="32 位页面 ID 或页面 URL 中的 ID" onChange={(event) => updateJob({ rootPageId: event.target.value })} /></label>
              <label className="wide"><span>本地文件夹</span><div className="input-action"><input value={activeJob.folderPath} readOnly placeholder="请选择需要备份的文件夹" /><button className="icon-button" onClick={chooseFolder}><FolderOpen size={18} /></button></div></label>
            </div>
            <div className="option-row">
              <label className="check-row"><input type="checkbox" checked={activeJob.skipHidden} onChange={(event) => updateJob({ skipHidden: event.target.checked })} /><span>忽略隐藏文件、Git、node_modules 和构建目录</span></label>
              <label className="check-row"><input type="checkbox" checked={activeJob.includeTextPreview} onChange={(event) => updateJob({ includeTextPreview: event.target.checked })} /><span>为 1 MiB 以下文本文件生成可搜索预览</span></label>
              <label className="check-row"><input type="checkbox" checked={activeJob.enabled} onChange={(event) => updateJob({ enabled: event.target.checked })} /><span>启用自动备份任务</span></label>
            </div>
            <div className="action-row">
              <button className="secondary" onClick={testConnection} disabled={!canConnect || Boolean(busy)}>{busy === "test" ? <LoaderCircle className="spin" size={17} /> : <ShieldCheck size={17} />}测试连接</button>
              <button className="secondary" onClick={() => void scanFolder()} disabled={!canScan || Boolean(busy)}>{busy === "scan" ? <LoaderCircle className="spin" size={17} /> : <RefreshCw size={17} />}扫描变化</button>
              <button className="secondary" onClick={restoreLatest} disabled={!hasToken || Boolean(busy) || history.length === 0}><ArchiveRestore size={17} />恢复最新版本</button>
              <button className="primary" onClick={() => void executeBackup(activeJob)} disabled={!canBackup}>{busy === "backup" ? <LoaderCircle className="spin" size={17} /> : <HardDriveUpload size={17} />}立即备份</button>
            </div>
          </section>

          <section className="dashboard-grid">
            <section className="panel files-panel">
              <div className="section-header compact"><div><h2>备份预览</h2><p>{activeJob.folderPath || "选择本地文件夹后扫描"}</p></div><span className="last-run"><Clock3 size={14} />{formatTime(activeJob.lastBackupAt)}</span></div>
              {scan ? <>
                <div className="stats-row"><div><strong>{scan.files.length}</strong><span>本地文件</span></div><div><strong>{formatBytes(scan.totalBytes)}</strong><span>扫描体积</span></div><div><strong>{scan.changedCount}</strong><span>待上传</span></div><div><strong>{scan.deletedCount}</strong><span>删除记录</span></div></div>
                {progress && busy && <div className="progress-card"><div className="progress-line"><span>{progress.stage}</span><span>{progress.current}/{progress.total}</span></div><div className="progress-track"><div style={{ width: `${progressPercent}%` }} /></div><small>{progress.relativePath}</small></div>}
                <div className="file-list">{scan.files.map((file) => <div className="file-row" key={file.relativePath}><div className="file-icon">{fileIcon(file.mimeType)}</div><div className="file-main"><strong>{file.relativePath}</strong><span>{file.mimeType} · {formatBytes(file.size)}</span></div><span className={`status status-${file.status}`}>{file.status === "new" ? "新增" : file.status === "modified" ? "新版本" : "未变化"}</span></div>)}</div>
              </> : <div className="empty-state"><FolderOpen size={42} /><h3>尚未扫描</h3><p>程序使用 SHA-256 判断变化，只上传新增和修改的文件。</p></div>}
            </section>

            <section className="panel history-panel">
              <div className="section-header compact"><div><h2>备份历史</h2><p>本机保留最近 100 次运行记录</p></div><History size={18} /></div>
              <div className="history-list">{history.length > 0 ? history.map((item) => <div className="history-item" key={item.id}><div><strong>{formatTime(item.finishedAt)}</strong><span>{item.totalFiles} 个文件 · {formatBytes(item.totalBytes)}</span></div><div className={item.failed ? "history-failed" : "history-ok"}>{item.failed ? `${item.failed} 失败` : `${item.uploaded} 上传`}</div></div>) : <div className="empty-history">还没有备份记录</div>}</div>
            </section>
          </section>

          {(backupResult?.failed || restoreResult?.failed) ? <section className="panel result-panel"><h2>需要处理的项目</h2>{[...(backupResult?.items ?? []), ...(restoreResult?.items ?? [])].filter((item) => item.status === "failed").map((item) => <div className="error-row" key={`${item.status}-${item.relativePath}`}><strong>{item.relativePath}</strong><span>{item.message}</span></div>)}</section> : null}
        </section>
      </div>
      <footer><span>备份方向：本地 → Notion；远端历史版本不会因本地删除而移除</span><span>支持 Notion 单段与分片文件上传</span></footer>
    </main>
  );
}
