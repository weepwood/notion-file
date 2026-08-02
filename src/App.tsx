import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertCircle, Check, File, FileText, FolderOpen, Image, LoaderCircle, Play, RefreshCw, Save, Settings2, ShieldCheck } from "lucide-react";
import type { AppConfig, ScanResult, SyncProgress, SyncResult } from "./types";

const DEFAULT_CONFIG: AppConfig = { folderPath: "", rootPageId: "", archiveDeleted: false, skipHidden: true };

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
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
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [progress, setProgress] = useState<SyncProgress | null>(null);
  const [busy, setBusy] = useState<"scan" | "sync" | "test" | "save" | null>(null);
  const [message, setMessage] = useState<{ type: "success" | "error"; text: string } | null>(null);

  useEffect(() => {
    Promise.all([invoke<AppConfig>("get_saved_config"), invoke<boolean>("has_saved_token")])
      .then(([saved, tokenSaved]) => { setConfig(saved); setHasToken(tokenSaved); })
      .catch((error) => setMessage({ type: "error", text: String(error) }));
    const unlisten = listen<SyncProgress>("sync-progress", (event) => setProgress(event.payload));
    return () => { void unlisten.then((dispose) => dispose()); };
  }, []);

  const canConnect = Boolean(config.rootPageId.trim() && (hasToken || token.trim()));
  const canScan = Boolean(config.folderPath.trim());
  const canSync = Boolean(canConnect && canScan && !busy);
  const progressPercent = useMemo(() => !progress || progress.total === 0 ? 0 : Math.round((progress.current / progress.total) * 100), [progress]);

  async function chooseFolder() {
    const selected = await open({ directory: true, multiple: false, title: "选择需要同步的文件夹" });
    if (typeof selected === "string") {
      setConfig((current) => ({ ...current, folderPath: selected }));
      setScan(null);
    }
  }

  async function persistToken() {
    if (!token.trim()) return;
    await invoke("save_notion_token", { token: token.trim() });
    setHasToken(true);
    setToken("");
  }

  async function saveSettings() {
    setBusy("save"); setMessage(null);
    try {
      await invoke("save_config", { config });
      await persistToken();
      setMessage({ type: "success", text: "设置已保存到本机。" });
    } catch (error) { setMessage({ type: "error", text: String(error) }); }
    finally { setBusy(null); }
  }

  async function testConnection() {
    setBusy("test"); setMessage(null);
    try {
      await persistToken();
      const title = await invoke<string>("test_notion_connection", { rootPageId: config.rootPageId.trim() });
      setMessage({ type: "success", text: `连接成功：${title || "目标页面可访问"}` });
    } catch (error) { setMessage({ type: "error", text: String(error) }); }
    finally { setBusy(null); }
  }

  async function scanFolder(showNotice = true) {
    setBusy("scan"); setMessage(null); setSyncResult(null);
    try {
      const result = await invoke<ScanResult>("scan_folder", { folderPath: config.folderPath, skipHidden: config.skipHidden });
      setScan(result);
      if (showNotice) setMessage({ type: "success", text: `扫描完成，发现 ${result.files.length} 个文件。` });
    } catch (error) { setMessage({ type: "error", text: String(error) }); }
    finally { setBusy(null); }
  }

  async function startSync() {
    setBusy("sync"); setMessage(null);
    setProgress({ current: 0, total: scan?.files.length ?? 0, relativePath: "", stage: "准备同步" });
    try {
      await invoke("save_config", { config });
      const result = await invoke<SyncResult>("sync_folder", { request: {
        folderPath: config.folderPath,
        rootPageId: config.rootPageId.trim(),
        archiveDeleted: config.archiveDeleted,
        skipHidden: config.skipHidden,
      }});
      setSyncResult(result);
      setMessage({ type: result.failed > 0 ? "error" : "success", text: `同步完成：新增 ${result.created}，更新 ${result.updated}，跳过 ${result.unchanged}，失败 ${result.failed}。` });
      const refreshed = await invoke<ScanResult>("scan_folder", { folderPath: config.folderPath, skipHidden: config.skipHidden });
      setScan(refreshed);
    } catch (error) { setMessage({ type: "error", text: String(error) }); }
    finally { setBusy(null); setProgress(null); }
  }

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand-mark">N</div>
        <div><h1>Notion File</h1><p>将本地文件夹增量同步为 Notion 文档</p></div>
        <div className={`connection-badge ${hasToken ? "connected" : ""}`}><ShieldCheck size={15} />{hasToken ? "令牌已保存" : "尚未配置令牌"}</div>
      </header>

      {message && <div className={`notice ${message.type}`}>{message.type === "success" ? <Check size={18} /> : <AlertCircle size={18} />}<span>{message.text}</span></div>}

      <section className="workspace-grid">
        <aside className="panel settings-panel">
          <div className="panel-title"><Settings2 size={18} /><div><h2>同步设置</h2><p>凭据只保存在系统凭据库中</p></div></div>
          <label><span>Notion Integration Token</span><input type="password" value={token} placeholder={hasToken ? "已保存，留空则不修改" : "ntn_... 或 secret_..."} onChange={(event) => setToken(event.target.value)} /></label>
          <label><span>目标根页面 ID</span><input value={config.rootPageId} placeholder="32 位页面 ID 或带连字符 UUID" onChange={(event) => setConfig((current) => ({ ...current, rootPageId: event.target.value }))} /></label>
          <label><span>本地文件夹</span><div className="input-action"><input value={config.folderPath} readOnly placeholder="请选择文件夹" /><button className="icon-button" onClick={chooseFolder} title="选择文件夹"><FolderOpen size={18} /></button></div></label>
          <label className="check-row"><input type="checkbox" checked={config.skipHidden} onChange={(event) => setConfig((current) => ({ ...current, skipHidden: event.target.checked }))} /><span>忽略隐藏文件和常见构建目录</span></label>
          <label className="check-row warning-option"><input type="checkbox" checked={config.archiveDeleted} onChange={(event) => setConfig((current) => ({ ...current, archiveDeleted: event.target.checked }))} /><span>本地删除时归档对应 Notion 页面</span></label>
          <div className="button-row">
            <button className="secondary" onClick={saveSettings} disabled={Boolean(busy)}>{busy === "save" ? <LoaderCircle className="spin" size={17} /> : <Save size={17} />}保存</button>
            <button className="secondary" onClick={testConnection} disabled={!canConnect || Boolean(busy)}>{busy === "test" ? <LoaderCircle className="spin" size={17} /> : <ShieldCheck size={17} />}测试连接</button>
          </div>
        </aside>

        <section className="panel files-panel">
          <div className="files-header">
            <div className="panel-title"><FolderOpen size={18} /><div><h2>同步预览</h2><p>{config.folderPath || "选择文件夹后扫描变更"}</p></div></div>
            <div className="button-row">
              <button className="secondary" onClick={() => void scanFolder()} disabled={!canScan || Boolean(busy)}>{busy === "scan" ? <LoaderCircle className="spin" size={17} /> : <RefreshCw size={17} />}扫描</button>
              <button className="primary" onClick={startSync} disabled={!canSync}>{busy === "sync" ? <LoaderCircle className="spin" size={17} /> : <Play size={17} />}开始同步</button>
            </div>
          </div>

          {scan ? <>
            <div className="stats-row">
              <div><strong>{scan.files.length}</strong><span>文件</span></div><div><strong>{formatBytes(scan.totalBytes)}</strong><span>总大小</span></div><div><strong>{scan.changedCount}</strong><span>待同步</span></div><div><strong>{scan.unchangedCount}</strong><span>未变化</span></div>
            </div>
            {busy === "sync" && progress && <div className="progress-card"><div className="progress-line"><span>{progress.stage}</span><span>{progress.current}/{progress.total}</span></div><div className="progress-track"><div style={{ width: `${progressPercent}%` }} /></div><small>{progress.relativePath}</small></div>}
            <div className="file-list">{scan.files.map((file) => <div className="file-row" key={file.relativePath}><div className="file-icon">{fileIcon(file.mimeType)}</div><div className="file-main"><strong>{file.relativePath}</strong><span>{file.mimeType} · {formatBytes(file.size)}</span></div><span className={`status status-${file.status}`}>{file.status === "new" ? "新增" : file.status === "modified" ? "已修改" : file.status === "deleted" ? "待归档" : "未变化"}</span></div>)}</div>
          </> : <div className="empty-state"><FolderOpen size={42} /><h3>尚未扫描文件夹</h3><p>应用会计算 SHA-256，仅上传新增或发生变化的文件。</p></div>}
        </section>
      </section>

      {syncResult && syncResult.failed > 0 && <section className="panel result-panel"><h2>失败项目</h2>{syncResult.items.filter((item) => item.status === "failed").map((item) => <div className="error-row" key={item.relativePath}><strong>{item.relativePath}</strong><span>{item.message}</span></div>)}</section>}
      <footer><span>单向同步：本地 → Notion</span><span>单文件上传上限：20 MiB（首版）</span></footer>
    </main>
  );
}
