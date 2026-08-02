import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArchiveRestore,
  Check,
  ChevronRight,
  CloudUpload,
  DatabaseBackup,
  FileArchive,
  FileCheck2,
  FileClock,
  FileWarning,
  FolderOpen,
  HardDrive,
  KeyRound,
  LoaderCircle,
  Play,
  RefreshCw,
  Save,
  Settings,
  ShieldCheck,
  Square,
} from "lucide-react";
import type {
  BackupConfig,
  BackupResult,
  ProgressPayload,
  RestoreResult,
  ScanFile,
  ScanResult,
  SnapshotRecord,
  Tab,
} from "./types";

const defaultConfig: BackupConfig = {
  source_folder: "",
  root_page_id: "",
  backup_name: "本地文件备份",
  include_hidden: false,
  follow_symlinks: false,
  exclude_patterns: [".git/**", "node_modules/**", "target/**", ".DS_Store", "Thumbs.db"],
  max_file_size_mib: 5120,
  skip_unchanged_snapshot: true,
};

const formatBytes = (value: number) => {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
};

const formatDate = (value: string) => {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString("zh-CN", { hour12: false });
};

const statusLabel: Record<ScanFile["status"], string> = {
  new: "新增",
  changed: "已修改",
  unchanged: "未变化",
  skipped: "已跳过",
  error: "失败",
};

function App() {
  const [tab, setTab] = useState<Tab>("backup");
  const [config, setConfig] = useState<BackupConfig>(defaultConfig);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [connectionName, setConnectionName] = useState("");
  const [busy, setBusy] = useState<"scan" | "backup" | "restore" | "save" | "test" | null>(null);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [progress, setProgress] = useState<ProgressPayload | null>(null);
  const [backupResult, setBackupResult] = useState<BackupResult | null>(null);
  const [snapshots, setSnapshots] = useState<SnapshotRecord[]>([]);
  const [selectedSnapshot, setSelectedSnapshot] = useState<string>("");
  const [restoreFolder, setRestoreFolder] = useState("");
  const [overwriteMode, setOverwriteMode] = useState("skip");
  const [restoreResult, setRestoreResult] = useState<RestoreResult | null>(null);
  const [notice, setNotice] = useState<{ type: "success" | "error" | "info"; text: string } | null>(null);

  const changedFiles = useMemo(
    () => scan?.files.filter((file) => file.status !== "unchanged") ?? [],
    [scan],
  );

  useEffect(() => {
    const load = async () => {
      try {
        const [saved, tokenExists, history] = await Promise.all([
          invoke<BackupConfig>("get_config"),
          invoke<boolean>("has_token"),
          invoke<SnapshotRecord[]>("list_snapshots"),
        ]);
        setConfig({ ...defaultConfig, ...saved });
        setHasToken(tokenExists);
        setSnapshots(history);
        if (history[0]) setSelectedSnapshot(history[0].id);
      } catch (error) {
        setNotice({ type: "error", text: String(error) });
      }
    };
    void load();

    const unlisten = listen<ProgressPayload>("backup-progress", (event) => setProgress(event.payload));
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const chooseSource = async () => {
    const value = await open({ directory: true, multiple: false, title: "选择需要备份的本地文件夹" });
    if (typeof value === "string") {
      setConfig((current) => ({ ...current, source_folder: value }));
      setScan(null);
    }
  };

  const chooseRestoreFolder = async () => {
    const value = await open({ directory: true, multiple: false, title: "选择恢复目标文件夹" });
    if (typeof value === "string") setRestoreFolder(value);
  };

  const saveSettings = async () => {
    setBusy("save");
    setNotice(null);
    try {
      await invoke("save_config", { config });
      if (token.trim()) {
        await invoke("save_token", { token: token.trim() });
        setToken("");
        setHasToken(true);
      }
      setNotice({ type: "success", text: "设置已保存。Notion Token 仅存放在系统凭据库中。" });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const testConnection = async () => {
    setBusy("test");
    setNotice(null);
    try {
      if (token.trim()) {
        await invoke("save_token", { token: token.trim() });
        setHasToken(true);
      }
      const title = await invoke<string>("test_connection", { rootPageId: config.root_page_id });
      setConnectionName(title);
      setNotice({ type: "success", text: `连接成功：${title}` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const scanFolder = async () => {
    setBusy("scan");
    setNotice(null);
    setBackupResult(null);
    try {
      await invoke("save_config", { config });
      const result = await invoke<ScanResult>("scan_backup", { config });
      setScan(result);
      setNotice({ type: "info", text: `扫描完成：${result.new_count} 个新增，${result.changed_count} 个修改，${result.deleted_paths.length} 个本地删除。` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  };

  const runBackup = async () => {
    setBusy("backup");
    setNotice(null);
    setProgress({ phase: "prepare", current: 0, total: 0, message: "正在准备备份…" });
    try {
      await invoke("save_config", { config });
      const result = await invoke<BackupResult>("run_backup", { config });
      setBackupResult(result);
      setNotice({ type: "success", text: result.message });
      const [history, latestScan] = await Promise.all([
        invoke<SnapshotRecord[]>("list_snapshots"),
        invoke<ScanResult>("scan_backup", { config }),
      ]);
      setSnapshots(history);
      setScan(latestScan);
      if (history[0]) setSelectedSnapshot(history[0].id);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  const cancelBackup = async () => {
    await invoke("cancel_backup");
    setNotice({ type: "info", text: "已请求停止；当前文件处理完成后会中止。" });
  };

  const restoreSnapshot = async () => {
    if (!selectedSnapshot || !restoreFolder) {
      setNotice({ type: "error", text: "请选择快照和恢复目标文件夹。" });
      return;
    }
    setBusy("restore");
    setRestoreResult(null);
    setNotice(null);
    try {
      const result = await invoke<RestoreResult>("restore_snapshot", {
        request: {
          snapshot_id: selectedSnapshot,
          target_folder: restoreFolder,
          overwrite_mode: overwriteMode,
        },
      });
      setRestoreResult(result);
      setNotice({
        type: result.failed_files > 0 ? "info" : "success",
        text: `恢复完成：${result.restored_files} 个文件成功，${result.skipped_files} 个跳过，${result.failed_files} 个失败。`,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark"><DatabaseBackup size={21} /></div>
          <div><strong>Notion File</strong><span>Backup</span></div>
        </div>
        <nav>
          <button className={tab === "backup" ? "active" : ""} onClick={() => setTab("backup")}><CloudUpload size={18} />备份</button>
          <button className={tab === "restore" ? "active" : ""} onClick={() => setTab("restore")}><ArchiveRestore size={18} />恢复</button>
          <button className={tab === "settings" ? "active" : ""} onClick={() => setTab("settings")}><Settings size={18} />设置</button>
        </nav>
        <div className="sidebar-status">
          <ShieldCheck size={17} />
          <div><strong>非破坏性备份</strong><span>远端历史不会自动删除</span></div>
        </div>
      </aside>

      <main className="content">
        <header className="topbar">
          <div>
            <span className="eyebrow">LOCAL → NOTION</span>
            <h1>{tab === "backup" ? "增量备份" : tab === "restore" ? "快照恢复" : "备份设置"}</h1>
          </div>
          <div className={`connection-pill ${hasToken && config.root_page_id ? "ready" : ""}`}>
            <span />{connectionName || (hasToken ? "凭据已保存" : "尚未配置")}
          </div>
        </header>

        {notice && <div className={`notice ${notice.type}`}>{notice.text}<button onClick={() => setNotice(null)}>×</button></div>}

        {tab === "backup" && (
          <section className="page-grid">
            <div className="hero-card">
              <div>
                <span className="section-kicker">BACKUP SOURCE</span>
                <h2>{config.backup_name || "未命名备份"}</h2>
                <p>{config.source_folder || "先在设置中选择一个本地文件夹"}</p>
              </div>
              <button className="folder-button" onClick={chooseSource}><FolderOpen size={18} />更换文件夹</button>
            </div>

            <div className="metric-grid">
              <Metric icon={<HardDrive size={19} />} label="文件总数" value={scan ? String(scan.total_files) : "—"} />
              <Metric icon={<FileArchive size={19} />} label="备份体积" value={scan ? formatBytes(scan.total_bytes) : "—"} />
              <Metric icon={<FileClock size={19} />} label="需要上传" value={scan ? String(scan.new_count + scan.changed_count) : "—"} />
              <Metric icon={<FileWarning size={19} />} label="本地已删除" value={scan ? String(scan.deleted_paths.length) : "—"} />
            </div>

            <div className="panel actions-panel">
              <div>
                <h3>创建不可变快照</h3>
                <p>内容按 SHA-256 去重；未变化文件复用已上传对象，删除只记录在新清单中。</p>
              </div>
              <div className="button-row">
                <button className="secondary" disabled={busy !== null || !config.source_folder} onClick={scanFolder}>
                  {busy === "scan" ? <LoaderCircle className="spin" size={17} /> : <RefreshCw size={17} />}扫描差异
                </button>
                {busy === "backup" ? (
                  <button className="danger" onClick={cancelBackup}><Square size={15} />停止</button>
                ) : (
                  <button className="primary" disabled={busy !== null || !config.source_folder || !config.root_page_id || !hasToken} onClick={runBackup}>
                    <Play size={16} fill="currentColor" />立即备份
                  </button>
                )}
              </div>
            </div>

            {progress && (
              <div className="panel progress-panel">
                <div className="progress-copy"><span>{progress.phase}</span><strong>{progress.message}</strong><small>{progress.relative_path || ""}</small></div>
                <div className="progress-track"><div style={{ width: `${progress.total > 0 ? Math.min(100, (progress.current / progress.total) * 100) : 8}%` }} /></div>
                <span className="progress-count">{progress.total > 0 ? `${progress.current} / ${progress.total}` : "准备中"}</span>
              </div>
            )}

            {backupResult && (
              <div className="panel result-panel">
                <FileCheck2 size={24} />
                <div><h3>快照已完成</h3><p>{backupResult.message}</p></div>
                <div className="result-stats"><span><b>{backupResult.uploaded_files}</b> 新对象</span><span><b>{backupResult.reused_files}</b> 已复用</span></div>
              </div>
            )}

            <div className="panel file-panel">
              <div className="panel-heading"><div><h3>差异预览</h3><p>{scan ? `显示 ${changedFiles.length} 个需要关注的项目` : "扫描后显示新增、修改、删除和异常项目"}</p></div></div>
              {!scan ? (
                <div className="empty-state"><FileArchive size={28} /><p>尚未扫描本地目录</p></div>
              ) : changedFiles.length === 0 && scan.deleted_paths.length === 0 ? (
                <div className="empty-state"><Check size={28} /><p>本地内容与上次备份一致</p></div>
              ) : (
                <div className="file-list">
                  {changedFiles.slice(0, 200).map((file) => (
                    <div className="file-row" key={file.relative_path}>
                      <span className={`status-dot ${file.status}`} />
                      <div className="file-name"><strong>{file.relative_path}</strong><span>{file.message || formatBytes(file.size)}</span></div>
                      <span className={`status-badge ${file.status}`}>{statusLabel[file.status]}</span>
                    </div>
                  ))}
                  {scan.deleted_paths.slice(0, 100).map((path) => (
                    <div className="file-row" key={`deleted-${path}`}>
                      <span className="status-dot deleted" />
                      <div className="file-name"><strong>{path}</strong><span>只从新快照中移除，旧快照仍保留</span></div>
                      <span className="status-badge deleted">已删除</span>
                    </div>
                  ))}
                  {changedFiles.length + scan.deleted_paths.length > 300 && <div className="list-truncated">列表仅显示前 300 项</div>}
                </div>
              )}
            </div>
          </section>
        )}

        {tab === "restore" && (
          <section className="restore-layout">
            <div className="panel snapshot-panel">
              <div className="panel-heading"><div><h3>备份快照</h3><p>快照清单存放在 Notion，旧版本不会被覆盖。</p></div></div>
              {snapshots.length === 0 ? (
                <div className="empty-state"><ArchiveRestore size={28} /><p>还没有可恢复的快照</p></div>
              ) : snapshots.map((snapshot) => (
                <button key={snapshot.id} className={`snapshot-card ${selectedSnapshot === snapshot.id ? "selected" : ""}`} onClick={() => setSelectedSnapshot(snapshot.id)}>
                  <div className="snapshot-icon"><FileArchive size={19} /></div>
                  <div><strong>{formatDate(snapshot.created_at)}</strong><span>{snapshot.file_count} 个文件 · {formatBytes(snapshot.total_bytes)}</span></div>
                  <ChevronRight size={18} />
                </button>
              ))}
            </div>

            <div className="panel restore-panel">
              <span className="section-kicker">RESTORE TARGET</span>
              <h2>恢复到本地目录</h2>
              <p>恢复时重新从 Notion 获取临时下载地址，并在写入后校验 SHA-256。</p>
              <label>目标文件夹</label>
              <div className="path-control"><input value={restoreFolder} readOnly placeholder="请选择一个空目录或已有目录" /><button onClick={chooseRestoreFolder}><FolderOpen size={17} /></button></div>
              <label>同名文件处理</label>
              <select value={overwriteMode} onChange={(event) => setOverwriteMode(event.target.value)}>
                <option value="skip">跳过已有文件</option>
                <option value="overwrite">覆盖已有文件</option>
                <option value="rename">保留两份并自动重命名</option>
              </select>
              <button className="primary wide" disabled={busy !== null || !selectedSnapshot || !restoreFolder} onClick={restoreSnapshot}>
                {busy === "restore" ? <LoaderCircle className="spin" size={17} /> : <ArchiveRestore size={17} />}开始恢复
              </button>
              {restoreResult && (
                <div className="restore-result">
                  <strong>恢复结果</strong>
                  <span>{restoreResult.restored_files} 成功 · {restoreResult.skipped_files} 跳过 · {restoreResult.failed_files} 失败</span>
                  {restoreResult.errors.slice(0, 5).map((error) => <small key={error}>{error}</small>)}
                </div>
              )}
            </div>
          </section>
        )}

        {tab === "settings" && (
          <section className="settings-layout">
            <div className="panel settings-panel">
              <div className="panel-heading"><div><h3>Notion 连接</h3><p>目标页面必须共享给对应的 Notion Integration。</p></div><KeyRound size={20} /></div>
              <label>Integration Token</label>
              <input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={hasToken ? "已保存；留空表示不修改" : "ntn_..."} />
              <label>目标根页面 ID 或 URL</label>
              <input value={config.root_page_id} onChange={(event) => setConfig({ ...config, root_page_id: event.target.value })} placeholder="粘贴 Notion 页面 URL 或 32 位页面 ID" />
              <button className="secondary" disabled={busy !== null || !config.root_page_id || (!hasToken && !token)} onClick={testConnection}>
                {busy === "test" ? <LoaderCircle className="spin" size={17} /> : <ShieldCheck size={17} />}测试连接
              </button>
            </div>

            <div className="panel settings-panel">
              <div className="panel-heading"><div><h3>备份任务</h3><p>本地路径不会写入远端清单，只保存相对路径。</p></div><HardDrive size={20} /></div>
              <label>任务名称</label>
              <input value={config.backup_name} onChange={(event) => setConfig({ ...config, backup_name: event.target.value })} />
              <label>本地源文件夹</label>
              <div className="path-control"><input value={config.source_folder} readOnly placeholder="请选择文件夹" /><button onClick={chooseSource}><FolderOpen size={17} /></button></div>
              <label>排除规则（每行一个 Glob）</label>
              <textarea rows={7} value={config.exclude_patterns.join("\n")} onChange={(event) => setConfig({ ...config, exclude_patterns: event.target.value.split("\n").map((item) => item.trim()).filter(Boolean) })} />
              <label>单文件上限（MiB，0 表示不在本地限制）</label>
              <input type="number" min={0} value={config.max_file_size_mib} onChange={(event) => setConfig({ ...config, max_file_size_mib: Number(event.target.value) || 0 })} />
              <div className="toggle-row">
                <Toggle checked={config.include_hidden} onChange={(checked) => setConfig({ ...config, include_hidden: checked })} label="包含隐藏文件" />
                <Toggle checked={config.follow_symlinks} onChange={(checked) => setConfig({ ...config, follow_symlinks: checked })} label="跟随符号链接" />
                <Toggle checked={config.skip_unchanged_snapshot} onChange={(checked) => setConfig({ ...config, skip_unchanged_snapshot: checked })} label="无变化时不创建快照" />
              </div>
              <button className="primary wide" disabled={busy !== null} onClick={saveSettings}>
                {busy === "save" ? <LoaderCircle className="spin" size={17} /> : <Save size={17} />}保存设置
              </button>
            </div>
          </section>
        )}
      </main>
    </div>
  );
}

function Metric({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return <div className="metric-card"><div className="metric-icon">{icon}</div><div><span>{label}</span><strong>{value}</strong></div></div>;
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (value: boolean) => void; label: string }) {
  return <label className="toggle"><button type="button" className={checked ? "checked" : ""} onClick={() => onChange(!checked)}><span /></button>{label}</label>;
}

export default App;
