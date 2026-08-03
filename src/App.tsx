import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  Check,
  ChevronRight,
  Clock3,
  File,
  FileText,
  FolderOpen,
  History,
  Image,
  KeyRound,
  LoaderCircle,
  MoreHorizontal,
  Play,
  RefreshCw,
  Settings2,
  ShieldCheck,
  Trash2,
  Upload,
} from "lucide-react";
import type {
  AppConfig,
  ScanResult,
  SyncProgress,
  SyncResult,
  UploadRecord,
} from "./types";

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

function pathName(path: string, fallback: string): string {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments.at(-1) || fallback;
}

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
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
  const [selectedFilePath, setSelectedFilePath] = useState("");
  const [uploadHistory, setUploadHistory] = useState<UploadRecord[]>([]);
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [progress, setProgress] = useState<SyncProgress | null>(null);
  const [busy, setBusy] = useState<"scan" | "sync" | "test" | "upload" | null>(null);
  const [message, setMessage] = useState<{
    type: "success" | "error" | "info";
    text: string;
  } | null>(null);

  useEffect(() => {
    Promise.all([
      invoke<AppConfig>("get_saved_config"),
      invoke<boolean>("has_saved_token"),
      invoke<UploadRecord[]>("get_upload_history").catch(() => []),
    ])
      .then(([saved, tokenSaved, records]) => {
        setConfig({ ...DEFAULT_CONFIG, ...saved });
        setHasToken(tokenSaved);
        setUploadHistory(records);
      })
      .catch((error) => setMessage({ type: "error", text: String(error) }));

    const unlisten = listen<SyncProgress>("sync-progress", (event) =>
      setProgress(event.payload),
    );
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const title = pathName(config.folderPath, "选择一个本地文件夹");
  const selectedFileName = pathName(selectedFilePath, "尚未选择文件");
  const hasCredentials = hasToken || Boolean(token.trim());
  const canScan = Boolean(config.folderPath.trim());
  const canSync = Boolean(hasCredentials && canScan && !busy);
  const canUpload = Boolean(hasCredentials && selectedFilePath.trim() && !busy);
  const successfulUploads = uploadHistory.filter((record) => record.status === "success").length;
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

  async function refreshUploadHistory() {
    const records = await invoke<UploadRecord[]>("get_upload_history");
    setUploadHistory(records);
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

  async function chooseSingleFile() {
    const selected = await open({
      directory: false,
      multiple: false,
      title: "选择需要上传到 Notion 的文件",
    });
    if (typeof selected !== "string") return;
    setSelectedFilePath(selected);
    setMessage({ type: "info", text: `已选择：${pathName(selected, selected)}` });
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

  async function uploadSingleFile() {
    if (!selectedFilePath.trim()) return;
    setBusy("upload");
    setMessage(null);
    try {
      await persistToken();
      await invoke("save_config", { config });
      const record = await invoke<UploadRecord>("upload_single_file", {
        request: {
          filePath: selectedFilePath,
          rootPageId: config.rootPageId.trim(),
        },
      });
      setUploadHistory((current) => [record, ...current.filter((item) => item.id !== record.id)].slice(0, 500));

      if (record.status === "success") {
        setMessage({
          type: "success",
          text: `“${record.fileName}”已上传到 Notion，并写入上传记录。`,
        });
        setSelectedFilePath("");
      } else {
        setMessage({
          type: "error",
          text: record.message || `“${record.fileName}”上传失败。`,
        });
      }
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
      await refreshUploadHistory().catch(() => undefined);
    } finally {
      setBusy(null);
    }
  }

  async function clearUploadHistory() {
    try {
      await invoke("clear_upload_history");
      setUploadHistory([]);
      setMessage({ type: "info", text: "本地上传记录已清空，不会删除 Notion 中的文件。" });
    } catch (error) {
      setMessage({ type: "error", text: String(error) });
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
            <Upload size={17} />
            单文件上传
          </button>
          <button>
            <FolderOpen size={17} />
            文件夹同步
          </button>
          <button>
            <History size={17} />
            上传记录
          </button>
          <button>
            <Settings2 size={17} />
            连接设置
          </button>
        </nav>

        <div className="sidebar-section">
          <span>当前文件夹</span>
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
            <span>{selectedFilePath ? selectedFileName : title}</span>
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
              <div className="page-icon">📄</div>
              <h1>本地文件上传</h1>
              <p className="page-description">
                支持将单个文件上传为独立 Notion 页面，也可以继续把整个文件夹整理为一篇同名文档。
                每次单文件上传都会在本地留下可查询的结果记录。
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
                    <h2>Notion 连接</h2>
                    <p>单文件上传和文件夹同步共用同一个 Token 与父页面设置。</p>
                  </div>
                  <span className="tag">系统凭据库</span>
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
                </div>
              </section>

              <section className="setup-block single-upload-block">
                <div className="block-heading">
                  <div>
                    <h2>上传单个文件</h2>
                    <p>文件会作为附件保存，并为小型文本文件生成可阅读预览。</p>
                  </div>
                  <span className="tag success">记录上传结果</span>
                </div>

                <div className="field-row">
                  <span className="field-icon">
                    <File size={17} />
                  </span>
                  <span className="field-copy">
                    <strong>{selectedFileName}</strong>
                    <small>{selectedFilePath || "支持图片、PDF、文档、代码及其他文件"}</small>
                  </span>
                  <button className="secondary compact" onClick={chooseSingleFile} disabled={Boolean(busy)}>
                    选择文件
                  </button>
                </div>

                <div className="action-row">
                  <button className="primary" onClick={uploadSingleFile} disabled={!canUpload}>
                    {busy === "upload" ? (
                      <LoaderCircle className="spin" size={16} />
                    ) : (
                      <Upload size={16} />
                    )}
                    上传到 Notion
                  </button>
                </div>
              </section>

              <section className="preview-block">
                <div className="block-heading">
                  <div>
                    <h2>上传记录</h2>
                    <p>
                      共 {uploadHistory.length} 条记录，成功 {successfulUploads} 条；最多保留最近 500 条。
                    </p>
                  </div>
                  <button
                    className="secondary"
                    onClick={clearUploadHistory}
                    disabled={uploadHistory.length === 0 || Boolean(busy)}
                  >
                    <Trash2 size={14} />
                    清空记录
                  </button>
                </div>

                {uploadHistory.length > 0 ? (
                  <div className="file-list">
                    {uploadHistory.map((record) => (
                      <div className="file-row" key={record.id}>
                        <span className="drag-handle">⋮⋮</span>
                        <span className="file-icon">{fileIcon(record.mimeType)}</span>
                        <div className="file-main">
                          <strong>{record.fileName}</strong>
                          <span>{record.filePath}</span>
                          <span>
                            {record.mimeType} · {formatBytes(record.size)} · {formatDate(record.uploadedAt)}
                          </span>
                          {record.pageUrl && <span>Notion：{record.pageUrl}</span>}
                          {record.status === "failed" && record.message && (
                            <span>失败原因：{record.message}</span>
                          )}
                        </div>
                        <span className={`status ${record.status === "success" ? "status-new" : "status-modified"}`}>
                          {record.status === "success" ? "已上传" : "失败"}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="empty-state">
                    <Clock3 size={34} />
                    <h3>还没有单文件上传记录</h3>
                    <p>选择文件并上传后，这里会记录文件、时间、状态和 Notion 页面地址。</p>
                  </div>
                )}
              </section>

              <section className="setup-block">
                <div className="block-heading">
                  <div>
                    <h2>文件夹同步</h2>
                    <p>选择文件夹后，将其中内容整理为一篇与文件夹同名的 Notion 文档。</p>
                  </div>
                  <span className="tag">单向同步</span>
                </div>

                <div className="field-row">
                  <span className="field-icon">
                    <FolderOpen size={17} />
                  </span>
                  <span className="field-copy">
                    <strong>本地文件夹</strong>
                    <small>{config.folderPath || "尚未选择文件夹"}</small>
                  </span>
                  <button className="secondary compact" onClick={chooseFolder} disabled={Boolean(busy)}>
                    选择文件夹
                  </button>
                </div>

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
                    同步文件夹
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
                    <h2>文件夹文档预览</h2>
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
