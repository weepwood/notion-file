import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  Archive,
  CheckCircle2,
  ChevronRight,
  Cloud,
  Database,
  Download,
  ExternalLink,
  FileText,
  Folder,
  FolderOpen,
  FolderSync,
  HardDrive,
  History,
  Home,
  KeyRound,
  Link,
  ListChecks,
  LoaderCircle,
  Move,
  Pause,
  Play,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Settings,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import type {
  AppConfig,
  DriveFolderDownloadResult,
  DriveInitResult,
  DriveNode,
  DriveQueueJob,
  DriveQueueSnapshot,
  DriveTransfer,
  DriveTransferProgress,
  DriveVersion,
  DriveView,
  ScanResult,
  SyncResult,
  UploadRecord,
} from "./types";

const DEFAULT_CONFIG: AppConfig = {
  folderPath: "",
  rootPageId: "",
  archiveDeleted: false,
  skipHidden: true,
  driveDatabaseId: "",
  driveDataSourceId: "",
  downloadDirectory: "",
};

type Notice = { type: "success" | "error" | "info"; text: string };

const EMPTY_QUEUE: DriveQueueSnapshot = {
  paused: false,
  workerRunning: false,
  pendingCount: 0,
  failedCount: 0,
  jobs: [],
};

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatRate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

function formatDuration(milliseconds: number): string {
  if (!milliseconds) return "0 ms";
  if (milliseconds < 1000) return `${Math.round(milliseconds)} ms`;
  const seconds = milliseconds / 1000;
  return seconds < 60 ? `${seconds.toFixed(1)} s` : `${(seconds / 60).toFixed(1)} min`;
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

function localName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || path;
}

function nodeIcon(node: DriveNode) {
  return node.nodeType === "folder" ? <Folder size={18} /> : <FileText size={18} />;
}

function transferLabel(status: DriveTransfer["status"]): string {
  return {
    queued: "等待中",
    running: "传输中",
    completed: "已完成",
    failed: "失败",
    cancelled: "已取消",
  }[status];
}

function queueStatusLabel(status: DriveQueueJob["status"]): string {
  return {
    pending: "等待中",
    running: "上传中",
    completed: "已完成",
    failed: "失败",
    cancelled: "已取消",
  }[status];
}

function versionDownloadName(fileName: string, version: number): string {
  const index = fileName.lastIndexOf(".");
  if (index <= 0) return `${fileName}.v${version}`;
  return `${fileName.slice(0, index)}.v${version}${fileName.slice(index)}`;
}

export default function App() {
  const [view, setView] = useState<DriveView>("drive");
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [nodes, setNodes] = useState<DriveNode[]>([]);
  const [transfers, setTransfers] = useState<DriveTransfer[]>([]);
  const [uploadQueue, setUploadQueue] = useState<DriveQueueSnapshot>(EMPTY_QUEUE);
  const [versions, setVersions] = useState<DriveVersion[]>([]);
  const [folderId, setFolderId] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [showTrash, setShowTrash] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [progress, setProgress] = useState<DriveTransferProgress | null>(null);
  const [legacyFile, setLegacyFile] = useState("");
  const [legacyScan, setLegacyScan] = useState<ScanResult | null>(null);
  const [legacyResult, setLegacyResult] = useState<SyncResult | null>(null);

  const driveReady = Boolean(config.driveDataSourceId.trim());
  const credentialsReady = hasToken || Boolean(token.trim());
  const currentFolder = nodes.find((node) => node.id === folderId) ?? null;
  const selected = nodes.find((node) => node.id === selectedId) ?? null;
  const canInitialize =
    credentialsReady &&
    (Boolean(config.rootPageId.trim()) ||
      (Boolean(config.driveDatabaseId.trim()) && Boolean(config.driveDataSourceId.trim())));

  useEffect(() => {
    void loadLocalData();
    const progressListener = listen<DriveTransferProgress>("drive-transfer-progress", (event) => {
      setProgress(event.payload);
      if (event.payload.nodeId) {
        setUploadQueue((current) => ({
          ...current,
          jobs: current.jobs.map((job) =>
            job.nodeId === event.payload.nodeId
              ? {
                  ...job,
                  stage: event.payload.stageCode || event.payload.stage,
                  transferredBytes: event.payload.transferredBytes,
                  size: Math.max(job.size, event.payload.totalBytes),
                  status: "running",
                }
              : job,
          ),
        }));
      }
    });
    const queueListener = listen<DriveQueueSnapshot>("drive-queue-changed", (event) => {
      setUploadQueue(event.payload);
      void refreshLocal();
    });
    const errorListener = listen<string>("drive-queue-error", (event) => {
      setNotice({ type: "error", text: `上传队列异常：${event.payload}` });
    });
    return () => {
      void progressListener.then((dispose) => dispose());
      void queueListener.then((dispose) => dispose());
      void errorListener.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    setVersions([]);
    if (!selectedId) return;
    const node = nodes.find((item) => item.id === selectedId);
    if (!node || node.nodeType !== "file" || node.status !== "active") return;
    void invoke<DriveVersion[]>("get_drive_versions", { nodeId: node.id })
      .then(setVersions)
      .catch((error) => setNotice({ type: "error", text: String(error) }));
  }, [selectedId, nodes]);

  async function loadLocalData() {
    try {
      const [saved, savedToken, savedNodes, savedTransfers, savedQueue] = await Promise.all([
        invoke<AppConfig>("get_saved_config"),
        invoke<boolean>("has_saved_token"),
        invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }).catch(() => []),
        invoke<DriveTransfer[]>("get_drive_transfers").catch(() => []),
        invoke<DriveQueueSnapshot>("get_drive_upload_queue").catch(() => EMPTY_QUEUE),
      ]);
      setConfig({ ...DEFAULT_CONFIG, ...saved });
      setHasToken(savedToken);
      setNodes(savedNodes);
      setTransfers(savedTransfers);
      setUploadQueue(savedQueue);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function refreshLocal() {
    const [savedNodes, savedTransfers, savedConfig, savedQueue] = await Promise.all([
      invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }),
      invoke<DriveTransfer[]>("get_drive_transfers"),
      invoke<AppConfig>("get_saved_config"),
      invoke<DriveQueueSnapshot>("get_drive_upload_queue").catch(() => EMPTY_QUEUE),
    ]);
    setNodes(savedNodes);
    setTransfers(savedTransfers);
    setConfig({ ...DEFAULT_CONFIG, ...savedConfig });
    setUploadQueue(savedQueue);
  }

  async function refreshVersions(nodeId: string) {
    const result = await invoke<DriveVersion[]>("get_drive_versions", { nodeId });
    setVersions(result);
  }

  const breadcrumbs = useMemo(() => {
    const result: DriveNode[] = [];
    let cursor: DriveNode | null = currentFolder;
    const visited = new Set<string>();
    while (cursor && !visited.has(cursor.id)) {
      visited.add(cursor.id);
      result.unshift(cursor);
      cursor = cursor.parentId
        ? nodes.find((node) => node.id === cursor?.parentId) ?? null
        : null;
    }
    return result;
  }, [currentFolder, nodes]);

  const visibleNodes = useMemo(() => {
    const text = query.trim().toLocaleLowerCase();
    return nodes
      .filter((node) => (showTrash ? node.status === "trashed" : node.status === "active"))
      .filter((node) => {
        if (text) {
          return (
            node.name.toLocaleLowerCase().includes(text) ||
            node.logicalPath.toLocaleLowerCase().includes(text)
          );
        }
        return showTrash || (node.parentId ?? null) === folderId;
      })
      .sort((left, right) => {
        if (left.nodeType !== right.nodeType) return left.nodeType === "folder" ? -1 : 1;
        return left.name.localeCompare(right.name, "zh-CN");
      });
  }, [folderId, nodes, query, showTrash]);

  const folders = useMemo(
    () => nodes.filter((node) => node.nodeType === "folder" && node.status === "active"),
    [nodes],
  );

  const stats = useMemo(() => {
    const active = nodes.filter((node) => node.status === "active");
    return {
      files: active.filter((node) => node.nodeType === "file").length,
      folders: active.filter((node) => node.nodeType === "folder").length,
      bytes: active.reduce((total, node) => total + node.size, 0),
      trashed: nodes.filter((node) => node.status === "trashed").length,
    };
  }, [nodes]);

  async function saveSettings() {
    if (token.trim()) {
      await invoke("save_notion_token", { token: token.trim() });
      setHasToken(true);
      setToken("");
    }
    await invoke("save_config", { config });
  }

  async function initializeDrive() {
    setBusy("initialize");
    setNotice(null);
    try {
      await saveSettings();
      const result = await invoke<DriveInitResult>("initialize_drive", {
        rootPageId: config.rootPageId.trim(),
      });
      await refreshLocal();
      setNotice({
        type: "success",
        text: `${result.created ? "已创建" : "已连接"} Notion Drive，恢复 ${result.nodeCount} 个节点。`,
      });
      setView("drive");
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function refreshRemote() {
    setBusy("refresh");
    try {
      const remote = await invoke<DriveNode[]>("refresh_drive_index");
      setNodes(remote);
      setSelectedId(null);
      setNotice({ type: "success", text: `已从 Notion 重建 ${remote.length} 个节点。` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function createFolder() {
    const name = window.prompt("请输入新文件夹名称");
    if (!name?.trim()) return;
    setBusy("folder");
    try {
      await invoke<DriveNode>("create_drive_folder", {
        name: name.trim(),
        parentId: folderId,
      });
      await refreshLocal();
      setNotice({ type: "success", text: `文件夹“${name.trim()}”已创建。` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function uploadFiles() {
    const chosen = await open({
      directory: false,
      multiple: true,
      title: "选择需要上传到 Notion Drive 的文件",
    });
    const paths = Array.isArray(chosen) ? chosen : typeof chosen === "string" ? [chosen] : [];
    if (!paths.length) return;
    try {
      const next = await invoke<DriveQueueSnapshot>("enqueue_drive_uploads", {
        request: { filePaths: paths, parentId: folderId },
      });
      setUploadQueue(next);
      setView("transfers");
      setNotice({
        type: "success",
        text: `已将 ${paths.length} 个文件写入持久化队列，切换页面或重启应用后任务仍会保留。`,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function downloadSelected() {
    if (!selected || selected.nodeType !== "file") return;
    const defaultPath = config.downloadDirectory
      ? `${config.downloadDirectory.replace(/[\\/]$/, "")}/${selected.name}`
      : selected.name;
    const destination = await save({ title: "保存云盘文件", defaultPath });
    if (!destination) return;

    setBusy("download");
    try {
      await invoke<DriveTransfer>("download_drive_file", {
        request: { nodeId: selected.id, destinationPath: destination },
      });
      const separator = Math.max(destination.lastIndexOf("/"), destination.lastIndexOf("\\"));
      if (separator > 0) {
        const nextConfig = { ...config, downloadDirectory: destination.slice(0, separator) };
        setConfig(nextConfig);
        await invoke("save_config", { config: nextConfig });
      }
      await refreshLocal();
      setNotice({ type: "success", text: `下载并校验完成：${destination}` });
    } catch (error) {
      await refreshLocal();
      setNotice({ type: "error", text: `${String(error)}。已保留临时文件，可在传输中心续传。` });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  async function downloadSelectedFolder() {
    if (!selected || selected.nodeType !== "folder") return;
    const destination = await open({
      directory: true,
      multiple: false,
      title: "选择文件夹下载位置",
      defaultPath: config.downloadDirectory || undefined,
    });
    if (typeof destination !== "string") return;
    setBusy("folder-download");
    try {
      const result = await invoke<DriveFolderDownloadResult>("download_drive_folder", {
        request: { folderId: selected.id, destinationDirectory: destination },
      });
      const nextConfig = { ...config, downloadDirectory: destination };
      setConfig(nextConfig);
      await invoke("save_config", { config: nextConfig });
      await refreshLocal();
      setNotice({
        type: result.failed ? "error" : "success",
        text: `文件夹下载完成：成功 ${result.succeeded}，失败 ${result.failed}，保存到 ${result.destinationDirectory}`,
      });
    } catch (error) {
      await refreshLocal();
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  async function uploadNewVersion() {
    if (!selected || selected.nodeType !== "file") return;
    const chosen = await open({
      directory: false,
      multiple: false,
      title: `选择“${selected.name}”的新版本`,
    });
    if (typeof chosen !== "string") return;
    setBusy("version-upload");
    try {
      const updated = await invoke<DriveNode>("upload_drive_version", {
        request: { nodeId: selected.id, filePath: chosen },
      });
      await refreshLocal();
      await refreshVersions(updated.id);
      setNotice({ type: "success", text: `新版本 v${updated.version} 已上传，旧版本仍可下载。` });
    } catch (error) {
      await refreshLocal();
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  async function downloadVersion(version: DriveVersion) {
    if (!selected || selected.nodeType !== "file") return;
    const defaultPath = config.downloadDirectory
      ? `${config.downloadDirectory.replace(/[\\/]$/, "")}/${versionDownloadName(selected.name, version.version)}`
      : versionDownloadName(selected.name, version.version);
    const destination = await save({ title: `保存版本 v${version.version}`, defaultPath });
    if (!destination) return;
    setBusy("version-download");
    try {
      await invoke<DriveTransfer>("download_drive_version", {
        request: { versionId: version.id, destinationPath: destination },
      });
      await refreshLocal();
      setNotice({ type: "success", text: `版本 v${version.version} 已下载并校验。` });
    } catch (error) {
      await refreshLocal();
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  async function retryTransfer(transfer: DriveTransfer) {
    setBusy(`retry-${transfer.id}`);
    try {
      await invoke<DriveTransfer>("retry_drive_transfer", { transferId: transfer.id });
      await refreshLocal();
      setNotice({ type: "success", text: `续传完成：${transfer.fileName}` });
    } catch (error) {
      await refreshLocal();
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  async function openSelectedNotionPage() {
    const url = selected?.notionPageUrl;
    if (!url) return;
    try {
      const parsed = new URL(url);
      if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
        throw new Error(`不支持打开 ${parsed.protocol} 链接`);
      }
      await openUrl(parsed.toString());
    } catch (error) {
      setNotice({
        type: "error",
        text: `无法使用系统浏览器打开 Notion 页面：${String(error)}`,
      });
    }
  }

  async function renameSelected() {
    if (!selected) return;
    const name = window.prompt("请输入新名称", selected.name);
    if (!name?.trim() || name.trim() === selected.name) return;
    setBusy("rename");
    try {
      await invoke<DriveNode>("rename_drive_node", {
        nodeId: selected.id,
        newName: name.trim(),
      });
      await refreshLocal();
      setNotice({ type: "success", text: "重命名完成。" });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function moveSelected() {
    if (!selected) return;
    const choices = folders
      .filter((folder) => folder.id !== selected.id)
      .map((folder) => folder.logicalPath)
      .sort()
      .join("\n");
    const targetPath = window.prompt(
      `输入目标文件夹路径，/ 表示根目录：\n\n${choices}`,
      currentFolder?.logicalPath ?? "/",
    );
    if (targetPath === null) return;
    const normalized = targetPath.trim() || "/";
    const target = normalized === "/" ? null : folders.find((folder) => folder.logicalPath === normalized);
    if (normalized !== "/" && !target) {
      setNotice({ type: "error", text: `找不到目标文件夹：${normalized}` });
      return;
    }

    setBusy("move");
    try {
      await invoke<DriveNode>("move_drive_node", {
        nodeId: selected.id,
        newParentId: target?.id ?? null,
      });
      await refreshLocal();
      setSelectedId(null);
      setNotice({ type: "success", text: "移动完成。" });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function setTrashed(trashed: boolean) {
    if (!selected) return;
    if (trashed && !window.confirm(`将“${selected.name}”移入回收站？`)) return;
    setBusy("trash");
    try {
      const count = await invoke<number>("set_drive_node_trashed", {
        nodeId: selected.id,
        trashed,
      });
      await refreshLocal();
      setSelectedId(null);
      setNotice({
        type: "success",
        text: trashed ? `已移入回收站：${count} 个节点。` : `已恢复：${count} 个节点。`,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function setQueuePaused(paused: boolean) {
    try {
      const next = await invoke<DriveQueueSnapshot>(
        paused ? "pause_drive_upload_queue" : "resume_drive_upload_queue",
      );
      setUploadQueue(next);
      setNotice({
        type: "info",
        text: paused
          ? "上传队列已暂停；当前正在发送的文件会完成，之后不再启动新任务。"
          : "上传队列已继续执行。",
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function retryQueueJob(jobId: string) {
    try {
      const next = await invoke<DriveQueueSnapshot>("retry_drive_upload_job", { jobId });
      setUploadQueue(next);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function cancelQueueJob(jobId: string) {
    try {
      const next = await invoke<DriveQueueSnapshot>("cancel_drive_upload_job", { jobId });
      setUploadQueue(next);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function clearFinishedQueue() {
    try {
      const next = await invoke<DriveQueueSnapshot>("clear_finished_drive_upload_queue");
      setUploadQueue(next);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function clearTransfers() {
    try {
      const count = await invoke<number>("clear_finished_drive_transfers");
      await refreshLocal();
      setNotice({ type: "success", text: `已清理 ${count} 条传输记录。` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function disconnect() {
    if (!window.confirm("只清除本机索引，不会删除 Notion 文件。确定断开？")) return;
    await invoke("disconnect_drive");
    setFolderId(null);
    setSelectedId(null);
    await refreshLocal();
    setNotice({ type: "info", text: "本机索引已断开，可重新连接远端 Data Source。" });
  }

  async function chooseLegacyFile() {
    const chosen = await open({ directory: false, multiple: false, title: "选择单个文件" });
    if (typeof chosen === "string") setLegacyFile(chosen);
  }

  async function uploadLegacyFile() {
    if (!legacyFile) return;
    setBusy("legacy-upload");
    try {
      const record = await invoke<UploadRecord>("upload_single_file", {
        request: {
          filePath: legacyFile,
          rootPageId: config.rootPageId,
          displayMode: "file",
        },
      });
      setNotice({
        type: record.status === "success" ? "success" : "error",
        text: record.message ?? record.status,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function chooseLegacyFolder() {
    const chosen = await open({ directory: true, multiple: false, title: "选择同步文件夹" });
    if (typeof chosen !== "string") return;
    const nextConfig = { ...config, folderPath: chosen };
    setConfig(nextConfig);
    await invoke("save_config", { config: nextConfig });
    setBusy("legacy-scan");
    try {
      const scan = await invoke<ScanResult>("scan_folder", {
        folderPath: chosen,
        skipHidden: nextConfig.skipHidden,
      });
      setLegacyScan(scan);
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function syncLegacyFolder() {
    if (!config.folderPath) return;
    setBusy("legacy-sync");
    try {
      const result = await invoke<SyncResult>("sync_folder", {
        request: {
          folderPath: config.folderPath,
          rootPageId: config.rootPageId,
          archiveDeleted: config.archiveDeleted,
          skipHidden: config.skipHidden,
        },
      });
      setLegacyResult(result);
      setNotice({
        type: result.failed ? "error" : "success",
        text: `同步完成：创建 ${result.created}，更新 ${result.updated}，失败 ${result.failed}。`,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  function renderQueueJob(job: DriveQueueJob) {
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

  const progressPercent = progress?.totalBytes
    ? Math.min(100, Math.round((progress.transferredBytes / progress.totalBytes) * 100))
    : 0;

  return (
    <div className="drive-shell">
      <aside className="drive-sidebar">
        <div className="brand">
          <div className="brand-icon"><Cloud size={22} /></div>
          <div><strong>Notion File</strong><span>个人云盘 · v0.7.0</span></div>
        </div>
        <nav>
          <button className={view === "drive" ? "active" : ""} onClick={() => setView("drive")}><HardDrive size={17} />我的云盘</button>
          <button className={view === "transfers" ? "active" : ""} onClick={() => setView("transfers")}><ListChecks size={17} />传输中心</button>
          <button className={view === "legacy" ? "active" : ""} onClick={() => setView("legacy")}><FolderSync size={17} />传统同步</button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}><Settings size={17} />连接设置</button>
        </nav>
        <div className={`connection-card ${driveReady && hasToken ? "ready" : ""}`}>
          {driveReady && hasToken ? <CheckCircle2 size={18} /> : <AlertCircle size={18} />}
          <div><strong>{driveReady ? "云盘已连接" : "云盘未初始化"}</strong><span>{hasToken ? "Token 已安全保存" : "需要保存 Notion Token"}</span></div>
        </div>
      </aside>

      <main className="drive-main">
        <header className="drive-topbar">
          <div><strong>{view === "drive" ? (showTrash ? "回收站" : "我的云盘") : view === "transfers" ? "传输中心" : view === "legacy" ? "传统上传与同步" : "连接设置"}</strong><span>{driveReady ? `Data Source ${config.driveDataSourceId.slice(0, 8)}…` : "使用 Notion 保存远端文件与索引"}</span></div>
          {(busy || uploadQueue.workerRunning) && <div className="busy-label"><LoaderCircle size={15} className="spin" />{busy ? "正在处理" : "后台上传中"}</div>}
        </header>

        <div className="drive-content">
          {notice && <div className={`notice ${notice.type}`}>{notice.type === "error" ? <AlertCircle size={18} /> : notice.type === "success" ? <CheckCircle2 size={18} /> : <Cloud size={18} />}<span>{notice.text}</span><button onClick={() => setNotice(null)}><X size={15} /></button></div>}
          {progress && (busy || uploadQueue.workerRunning) && <div className="transfer-banner diagnostic-banner">
            <div className="diagnostic-title"><div>{progress.direction === "upload" ? <Upload size={18} /> : <Download size={18} />}<span><strong>{progress.fileName}</strong><small>{progress.stage}</small></span></div><div className="progress-meta">{formatBytes(progress.transferredBytes)} / {formatBytes(progress.totalBytes)}</div></div>
            <div className="progress-track"><div style={{ width: `${progressPercent}%` }} /></div>
            {progress.direction === "upload" && <>
              <div className="diagnostic-metrics">
                <div><span>{progress.stageCode === "hashing" ? "本地处理速度" : "当前上传速度（估算）"}</span><strong>{formatRate(progress.currentSpeedBytesPerSecond)}</strong></div>
                <div><span>{progress.stageCode === "hashing" ? "平均处理速度" : "平均有效速度（估算）"}</span><strong>{formatRate(progress.averageSpeedBytesPerSecond)}</strong></div>
                <div><span>当前阶段耗时</span><strong>{formatDuration(progress.stageElapsedMs)}</strong></div>
                <div><span>总耗时</span><strong>{formatDuration(progress.elapsedMs)}</strong></div>
                {progress.currentPart && progress.totalParts && <div><span>API 分片</span><strong>{progress.currentPart} / {progress.totalParts}</strong></div>}
              </div>
              {progress.endpointUrl && <div className="endpoint-row"><span>上传网址</span><code title={progress.endpointUrl}>{progress.endpointUrl}</code></div>}
              {progress.diagnosticHint && <div className="diagnostic-hint"><AlertCircle size={15} /><span>{progress.diagnosticHint}</span></div>}
            </>}
          </div>}

          {view === "drive" && (
            <section>
              {!driveReady ? <div className="onboarding-card"><div className="onboarding-icon"><Cloud size={38} /></div><h1>把 Notion 变成个人文件云盘</h1><p>文件保存在 Notion，目录与传输状态由桌面客户端管理。本地索引可以随时从远端重建。</p><button className="primary" onClick={() => setView("settings")}><Settings size={16} />开始配置</button></div> : <>
                <div className="stats-grid"><div><FileText size={18} /><strong>{stats.files}</strong><span>文件</span></div><div><Folder size={18} /><strong>{stats.folders}</strong><span>文件夹</span></div><div><Database size={18} /><strong>{formatBytes(stats.bytes)}</strong><span>已索引容量</span></div><div><Archive size={18} /><strong>{stats.trashed}</strong><span>回收站节点</span></div></div>
                <div className="drive-toolbar"><div className="toolbar-actions">{!showTrash && <button className="primary" onClick={uploadFiles} disabled={Boolean(busy)}><Upload size={16} />上传文件</button>}{!showTrash && <button onClick={createFolder} disabled={Boolean(busy)}><Plus size={16} />新建文件夹</button>}<button onClick={refreshRemote} disabled={Boolean(busy)}><RefreshCw size={16} />重建索引</button><button className={showTrash ? "active" : ""} onClick={() => { setShowTrash((value) => !value); setSelectedId(null); }}><Trash2 size={16} />{showTrash ? "返回云盘" : "回收站"}</button></div><label className="search-box"><Search size={15} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索名称或路径" /></label></div>
                {!showTrash && <div className="breadcrumbs"><button onClick={() => setFolderId(null)}><Home size={15} />根目录</button>{breadcrumbs.map((folder) => <span key={folder.id}><ChevronRight size={14} /><button onClick={() => setFolderId(folder.id)}>{folder.name}</button></span>)}</div>}
                <div className="drive-workspace">
                  <div className="file-panel"><div className="file-table header-row"><span>名称</span><span>大小</span><span>修改时间</span><span>状态</span></div><div className="file-table-body">{visibleNodes.length === 0 ? <div className="empty-state"><FolderOpen size={36} /><strong>{query ? "没有匹配的文件" : showTrash ? "回收站为空" : "这个文件夹为空"}</strong></div> : visibleNodes.map((node) => <button key={node.id} className={`file-table row ${selectedId === node.id ? "selected" : ""}`} onClick={() => setSelectedId(node.id)} onDoubleClick={() => node.nodeType === "folder" && node.status === "active" && setFolderId(node.id)}><span className="file-name"><i>{nodeIcon(node)}</i><span><strong>{node.name}</strong><small>{node.logicalPath}</small></span></span><span>{node.nodeType === "folder" ? "—" : formatBytes(node.size)}</span><span>{formatDate(node.modifiedAt)}</span><span><em className={`node-status ${node.status}`}>{node.status === "active" ? "正常" : "已删除"}</em></span></button>)}</div></div>
                  <aside className="inspector">{selected ? <><div className="inspector-title"><i>{nodeIcon(selected)}</i><div><strong>{selected.name}</strong><span>{selected.logicalPath}</span></div></div><dl><div><dt>类型</dt><dd>{selected.nodeType === "folder" ? "文件夹" : selected.mimeType ?? "文件"}</dd></div><div><dt>大小</dt><dd>{formatBytes(selected.size)}</dd></div><div><dt>版本</dt><dd>v{selected.version}</dd></div><div><dt>修改时间</dt><dd>{formatDate(selected.modifiedAt)}</dd></div>{selected.sha256 && <div><dt>SHA-256</dt><dd className="hash">{selected.sha256}</dd></div>}</dl><div className="inspector-actions">{selected.status === "active" && selected.nodeType === "file" && <button className="primary" onClick={downloadSelected} disabled={Boolean(busy)}><Download size={15} />下载</button>}{selected.status === "active" && selected.nodeType === "folder" && <button className="primary" onClick={downloadSelectedFolder} disabled={Boolean(busy)}><Download size={15} />下载文件夹</button>}{selected.status === "active" && selected.nodeType === "file" && <button onClick={uploadNewVersion} disabled={Boolean(busy)}><Upload size={15} />上传新版本</button>}{selected.status === "active" && <button onClick={renameSelected} disabled={Boolean(busy)}><Pencil size={15} />重命名</button>}{selected.status === "active" && <button onClick={moveSelected} disabled={Boolean(busy)}><Move size={15} />移动</button>}{selected.notionPageUrl && <button onClick={openSelectedNotionPage}><ExternalLink size={15} />Notion 页面</button>}{selected.status === "active" ? <button className="danger" onClick={() => setTrashed(true)} disabled={Boolean(busy)}><Trash2 size={15} />移入回收站</button> : <button onClick={() => setTrashed(false)} disabled={Boolean(busy)}><RotateCcw size={15} />恢复</button>}</div>{selected.nodeType === "file" && selected.status === "active" && <div className="version-panel"><div><h3>版本历史</h3><span>{versions.length} 个版本</span></div>{versions.length ? <div className="version-list">{versions.map((version) => <div className="version-item" key={version.id}><div><strong>v{version.version} · {formatBytes(version.size)}</strong><small>{formatDate(version.createdAt)} · {version.sha256.slice(0, 12)}…</small></div><button onClick={() => downloadVersion(version)} disabled={Boolean(busy)}><Download size={13} /> 下载</button></div>)}</div> : <div className="version-empty">当前文件尚未建立版本记录</div>}<div className="capability-note">新版本会追加新的 Notion 文件块；旧版本保留并可独立下载。</div></div>}</> : <div className="empty-inspector"><HardDrive size={30} /><strong>选择一个节点</strong><span>查看文件信息并执行下载、移动或版本管理。</span></div>}</aside>
                </div>
              </>}
            </section>
          )}

          {view === "transfers" && (
            <section>
              <div className="section-heading">
                <div>
                  <h1>传输中心</h1>
                  <p>上传任务写入 SQLite 后由 Rust 后台顺序执行，应用重启会自动恢复未完成任务。</p>
                </div>
                <div className="queue-toolbar">
                  <button
                    onClick={() => setQueuePaused(uploadQueue.workerRunning && !uploadQueue.paused)}
                  >
                    {uploadQueue.paused || (!uploadQueue.workerRunning && uploadQueue.pendingCount > 0)
                      ? <Play size={15} />
                      : <Pause size={15} />}
                    {uploadQueue.paused
                      ? "继续队列"
                      : !uploadQueue.workerRunning && uploadQueue.pendingCount > 0
                        ? "启动队列"
                        : "暂停队列"}
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

          {view === "settings" && <section className="settings-page"><div className="section-heading"><div><h1>连接设置</h1><p>新建云盘需要共享父页面；连接已有 Database/Data Source 时可以不填写父页面。</p></div></div><div className="settings-card"><label><span><KeyRound size={16} />Notion Token</span><input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={hasToken ? "Token 已保存，留空表示不修改" : "secret_... 或 ntn_..."} /></label><label><span><Link size={16} />父页面链接或 ID</span><input value={config.rootPageId} onChange={(event) => setConfig({ ...config, rootPageId: event.target.value })} placeholder="新建云盘时填写" /></label><details><summary>连接已有云盘数据库</summary><label><span><Database size={16} />Database ID</span><input value={config.driveDatabaseId} onChange={(event) => setConfig({ ...config, driveDatabaseId: event.target.value })} /></label><label><span><Database size={16} />Data Source ID</span><input value={config.driveDataSourceId} onChange={(event) => setConfig({ ...config, driveDataSourceId: event.target.value })} /></label></details><div className="settings-actions"><button onClick={saveSettings} disabled={!credentialsReady || Boolean(busy)}>保存设置</button><button className="primary" onClick={initializeDrive} disabled={!canInitialize || Boolean(busy)}>{busy === "initialize" ? <LoaderCircle size={15} className="spin" /> : <Cloud size={15} />}初始化或连接云盘</button>{driveReady && <button className="danger" onClick={disconnect} disabled={uploadQueue.workerRunning}>断开本机索引</button>}</div></div></section>}

          {view === "legacy" && <section><div className="section-heading"><div><h1>传统上传与文件夹转文档</h1><p>保留 v0.4.0 的单文件页面和本地文件夹转 Notion 文档能力。</p></div></div><div className="legacy-grid"><div className="legacy-card"><h2>单文件上传</h2><p>{legacyFile || "选择文件后创建同名 Notion 页面。"}</p><div><button onClick={chooseLegacyFile}><FileText size={15} />选择文件</button><button className="primary" onClick={uploadLegacyFile} disabled={!legacyFile || Boolean(busy)}><Upload size={15} />上传</button></div></div><div className="legacy-card"><h2>文件夹转文档</h2><p>{config.folderPath || "选择本地文件夹，扫描并整理为一篇 Notion 文档。"}</p><div><button onClick={chooseLegacyFolder}><FolderOpen size={15} />选择并扫描</button><button className="primary" onClick={syncLegacyFolder} disabled={!legacyScan || Boolean(busy)}><FolderSync size={15} />开始同步</button></div>{legacyScan && <small>扫描 {legacyScan.files.length} 个文件，变化 {legacyScan.changedCount} 个，共 {formatBytes(legacyScan.totalBytes)}</small>}{legacyResult?.pageUrl && <button onClick={() => window.open(legacyResult.pageUrl, "_blank")}><ExternalLink size={14} />打开同步文档</button>}</div></div></section>}
        </div>
      </main>
    </div>
  );
}
