import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Check,
  ChevronRight,
  File,
  FileText,
  FolderOpen,
  Image,
  KeyRound,
  LoaderCircle,
  MoreHorizontal,
  Play,
  RefreshCw,
  Settings2,
  ShieldCheck,
} from "lucide-react";
import type { AppConfig, ScanResult, SyncProgress, SyncResult } from "./types";

const DEFAULT_CONFIG: AppConfig = {
  folderPath: "",
  rootPageId: "",
  archiveDeleted: false,
  skipHidden: true,
};

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function folderName(path: string): string {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments.at(-1) || "选择一个本地文件夹";
}

function fileIcon(mimeType: string) {
  if (mimeType.startsWith("image/")) return <Image size={16} />;
  if (
    mimeType.startsWith("text/") ||
    mimeType.includes("json") ||
    mimeType.includes("markdown")
  ) {
    return <FileText size={16} />;
  }
  return <File size={16} />;
}

export default function App() {
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [progress, setProgress] = useState<SyncProgress | null>(null);
  const [busy, setBusy] = useState<"scan" | "sync" | "test" | null>(null);
  const [message, setMessage] = useState<{
    type: "success" | "error" | "info";
    text: string;
  } | null>(null);

  useEffect(() => {
    Promise.all([
      invoke<AppConfig>("get_saved_config"),
      invoke<boolean>("has_saved_token"),
    ])
      .then(([saved, tokenSaved]) => {
        setConfig({ ...DEFAULT_CONFIG, ...saved });
        setHasToken(tokenSaved);
      })
      .catch((error) => setMessage({ type: "error", text: String(error) }));

    const unlisten = listen<SyncProgress>("sync-progress", (event) =>
      setProgress(event.payload),
    );
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const title = folderName(config.folderPath);
  const hasCredentials = hasToken || Boolean(token.trim());
  const canScan = Boolean(config.folderPath.trim());
  const canSync = Boolean(hasCredentials && canScan && !busy);
  const progressPercent = useMemo(
    () =>
      !progress || progress.total === 0
        ? 0
        : Math.round((progress.current / progress.total) * 100),
    [progress],
  );

  async function persistToken() {
    if (!token.trim()) return;
    await invoke("save_notion_token", { token: token.trim() });
    setHasToken(true);
    setToken("");
  }

  async function scanPath(path: string, showNotice = true) {
    setBusy("scan");
    setMessage(null);
    setSyncResult(null);
    try {
      const result = await invoke<ScanResult>("scan_folder", {
        folderPath: path,
        skipHidden: config.skipHidden,
      });
      setScan(result);
      if (showNotice) {
        setMessage({
          type: "info",
          text: `扫描完成：${result.files.length} 个文件，${result.changedCount} 个待同步。`,
        });
      }
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function chooseFolder() {
    const selected = await open({
      directory: true,
      multiple: false,
      title: "选择需要同步到 Notion 的文件夹",
    });
    if (typeof selected !== "string") return;

    const nextConfig = { ...config, folderPath: selected };
    setConfig(nextConfig);
    setScan(null);
    setSyncResult(null);
    await invoke("save_config", { config: nextConfig });
    await scanPath(selected, false);
  }

  async function testConnection() {
    setBusy("test");
    setMessage(null);
    try {
      await persistToken();
      await invoke("save_config", { config });
      const label = await invoke<string>("test_notion_connection", {
        rootPageId: config.rootPageId.trim(),
      });
      setMessage({
        type: "success",
        text: config.rootPageId.trim()
          ? `连接成功，父页面可访问：${label}`
          : `连接成功：${label}`,
      });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function startSync() {
    setBusy("sync");
    setMessage(null);
    setProgress({
      current: 0,
      total: scan?.files.length ?? 0,
      relativePath: "",
      stage: "正在创建同名文档",
    });

    try {
      await persistToken();
      await invoke("save_config", { config });
      const result = await invoke<SyncResult>("sync_folder", {
        request: {
          folderPath: config.folderPath,
          rootPageId: config.rootPageId.trim(),
          archiveDeleted: false,
          skipHidden: config.skipHidden,
        },
      });

      setSyncResult(result);
      setMessage({
        type: result.failed > 0 ? "error" : "success",
        text:
          result.failed > 0
            ? `有 ${result.failed} 个文件无法写入，原文档未被覆盖。`
            : `“${result.documentTitle}”已同步到 Notion：新增 ${result.created}，更新 ${result.updated}，移除 ${result.archived}。`,
      });

      const refreshed = await invoke<ScanResult>("scan_folder", {
        folderPath: config.folderPath,
        skipHidden: config.skipHidden,
      });
      setScan(refreshed);
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  return (
    <div className="notion-shell">
      <aside className="sidebar">
        <div className="workspace-switcher">
          <img src="/app-icon.svg" alt="" />
          <div>
            <strong>Notion File</strong>
            <span>本地文件同步</span>
          </div>
          <MoreHorizontal size={17} />
        </div>

        <nav className="sidebar-nav">
          <button className="active">
            <FolderOpen size={17} />
            同步文档
          </button>
          <button>
            <Settings2 size={17} />
            连接设置
          </button>
        </nav>

        <div className="sidebar-section">
          <span>当前文档</span>
          <button className="document-link active">
            <span className="page-emoji">📁</span>
            <span>{title}</span>
          </button>
        </div>

        <div className={`credential-status ${hasCredentials ? "ready" : ""}`}>
          <ShieldCheck size={17} />
          <div>
            <strong>{hasCredentials ? "凭据已就绪" : "尚未保存 Token"}</strong>
            <span>Token 仅保存在系统凭据库</span>
          </div>
        </div>
      </aside>

      <main className="main-view">
        <header className="topbar">
          <div className="breadcrumbs">
            <span>Notion File</span>
            <ChevronRight size={14} />
            <span>{title}</span>
          </div>
          <div className={`connection-dot ${hasCredentials ? "ready" : ""}`}>
            <span />
            {hasCredentials ? "已连接" : "未连接"}
          </div>
        </header>

        <div className="page-scroll">
          <section className="notion-page">
            <div className="page-cover" />
            <div className="page-content">
              <div className="page-icon">📁</div>
              <h1>{title}</h1>
              <p className="page-description">
                将一个本地文件夹整理为一篇同名 Notion 文档。文本文件转换为可阅读内容，图片、PDF
                和其他文件以附件块保存。
              </p>

              {message && (
                <div className={`notice ${message.type}`}>
                  {message.type === "success" ? (
                    <Check size={18} />
                  ) : message.type === "error" ? (
                    <AlertCircle size={18} />
                  ) : (
                    <RefreshCw size={18} />
                  )}
                  <span>{message.text}</span>
                  <button onClick={() => setMessage(null)}>×</button>
                </div>
              )}

              <section className="setup-block">
                <div className="block-heading">
                  <div>
                    <h2>开始同步</h2>
                    <p>必填项只有 Notion Token 和本地文件夹。</p>
                  </div>
                  <span className="tag">单向同步</span>
                </div>

                <label className="field-row">
                  <span className="field-icon">
                    <KeyRound size={17} />
                  </span>
                  <span className="field-copy">
                    <strong>Notion Token</strong>
                    <small>
                      {hasToken
                        ? "已保存；留空不会修改现有 Token"
                        : "支持个人访问令牌、OAuth Token 或内部 Integration Token"}
                    </small>
                  </span>
                  <input
                    type="password"
                    value={token}
                    placeholder={hasToken ? "已保存在系统凭据库" : "ntn_..."}
                    onChange={(event) => setToken(event.target.value)}
                  />
                </label>

                <div className="field-row">
                  <span className="field-icon">
                    <FolderOpen size={17} />
                  </span>
                  <span className="field-copy">
                    <strong>本地文件夹</strong>
                    <small>{config.folderPath || "尚未选择文件夹"}</small>
                  </span>
                  <button className="secondary compact" onClick={chooseFolder}>
                    选择文件夹
                  </button>
                </div>

                <details className="advanced-settings">
                  <summary>
                    <span>内部 Integration 兼容设置</span>
                    <small>仅内部 Token 需要</small>
                  </summary>
                  <div className="advanced-content">
                    <label>
                      <span>父页面 ID 或页面链接</span>
                      <input
                        value={config.rootPageId}
                        placeholder="可选：内部 Integration Token 必填"
                        onChange={(event) =>
                          setConfig((current) => ({
                            ...current,
                            rootPageId: event.target.value,
                          }))
                        }
                      />
                    </label>
                    <p>
                      内部 Integration 无法直接创建工作区顶层页面。请先在 Notion
                      中创建一个父页面，将该 Integration 添加为连接，再粘贴页面链接或 ID。
                    </p>
                  </div>
                </details>

                <label className="toggle-row">
                  <input
                    type="checkbox"
                    checked={config.skipHidden}
                    onChange={(event) =>
                      setConfig((current) => ({
                        ...current,
                        skipHidden: event.target.checked,
                      }))
                    }
                  />
                  <span>
                    <strong>忽略隐藏文件和构建目录</strong>
                    <small>自动排除 .git、node_modules、target、dist 等目录</small>
                  </span>
                </label>

                <div className="action-row">
                  <button
                    className="secondary"
                    onClick={testConnection}
                    disabled={!hasCredentials || Boolean(busy)}
                  >
                    {busy === "test" ? (
                      <LoaderCircle className="spin" size={16} />
                    ) : (
                      <ShieldCheck size={16} />
                    )}
                    测试连接
                  </button>
                  <button
                    className="secondary"
                    onClick={() => void scanPath(config.folderPath)}
                    disabled={!canScan || Boolean(busy)}
                  >
                    {busy === "scan" ? (
                      <LoaderCircle className="spin" size={16} />
                    ) : (
                      <RefreshCw size={16} />
                    )}
                    重新扫描
                  </button>
                  <button className="primary" onClick={startSync} disabled={!canSync}>
                    {busy === "sync" ? (
                      <LoaderCircle className="spin" size={16} />
                    ) : (
                      <Play size={16} fill="currentColor" />
                    )}
                    同步到 Notion
                  </button>
                </div>
              </section>

              {busy === "sync" && progress && (
                <section className="progress-block">
                  <div>
                    <strong>{progress.stage}</strong>
                    <span>
                      {progress.current}/{progress.total}
                    </span>
                  </div>
                  <div className="progress-track">
                    <div style={{ width: `${progressPercent}%` }} />
                  </div>
                  <small>{progress.relativePath}</small>
                </section>
              )}

              <section className="preview-block">
                <div className="block-heading">
                  <div>
                    <h2>文档预览</h2>
                    <p>
                      {scan
                        ? `${scan.files.length} 个文件将整理到“${title}”中`
                        : "选择文件夹后自动扫描内容"}
                    </p>
                  </div>
                  {syncResult?.failed === 0 && <span className="tag success">已创建同名文档</span>}
                </div>

                {scan ? (
                  <>
                    <div className="stats-row">
                      <div>
                        <strong>{scan.files.length}</strong>
                        <span>文件</span>
                      </div>
                      <div>
                        <strong>{formatBytes(scan.totalBytes)}</strong>
                        <span>总大小</span>
                      </div>
                      <div>
                        <strong>{scan.changedCount}</strong>
                        <span>待同步</span>
                      </div>
                      <div>
                        <strong>{scan.deletedCount}</strong>
                        <span>将移除</span>
                      </div>
                    </div>

                    <div className="file-list">
                      {scan.files.map((file) => (
                        <div className="file-row" key={file.relativePath}>
                          <span className="drag-handle">⋮⋮</span>
                          <span className="file-icon">{fileIcon(file.mimeType)}</span>
                          <div className="file-main">
                            <strong>{file.relativePath}</strong>
                            <span>
                              {file.mimeType} · {formatBytes(file.size)}
                            </span>
                          </div>
                          <span className={`status status-${file.status}`}>
                            {file.status === "new"
                              ? "新增"
                              : file.status === "modified"
                                ? "已修改"
                                : "未变化"}
                          </span>
                        </div>
                      ))}
                    </div>
                  </>
                ) : (
                  <div className="empty-state">
                    <div className="empty-icon">📂</div>
                    <h3>还没有选择文件夹</h3>
                    <p>选择后会显示将写入同名 Notion 文档的文件结构。</p>
                  </div>
                )}
              </section>

              {syncResult?.failed ? (
                <section className="failure-block">
                  <h2>未写入的文件</h2>
                  {syncResult.items
                    .filter((item) => item.status === "failed")
                    .map((item) => (
                      <div key={item.relativePath}>
                        <strong>{item.relativePath}</strong>
                        <span>{item.message}</span>
                      </div>
                    ))}
                </section>
              ) : null}
            </div>
          </section>
        </div>
      </main>
    </div>
  );
}
