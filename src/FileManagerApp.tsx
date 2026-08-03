import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  AlertCircle,
  Archive,
  ArrowDownAZ,
  ArrowUpAZ,
  Check,
  CheckCircle2,
  ChevronRight,
  Clock3,
  Cloud,
  Copy,
  Database,
  Download,
  ExternalLink,
  File,
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  Files,
  Folder,
  FolderOpen,
  HardDrive,
  Home,
  Layers3,
  LoaderCircle,
  Move,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Settings2,
  Square,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import App from "./App";
import type {
  AppConfig,
  DriveFolderDownloadResult,
  DriveNode,
  DriveQueueSnapshot,
  DriveTransfer,
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

const EMPTY_QUEUE: DriveQueueSnapshot = {
  paused: false,
  workerRunning: false,
  pendingCount: 0,
  failedCount: 0,
  jobs: [],
};

const LARGE_FILE_BYTES = 100 * 1024 * 1024;
const RECENT_WINDOW_MS = 30 * 24 * 60 * 60 * 1000;

type SmartView =
  | "folder"
  | "all"
  | "recent"
  | "large"
  | "duplicates"
  | "files"
  | "folders"
  | "trash";

type FileCategory = "all" | "image" | "video" | "audio" | "document" | "archive" | "other";
type SortKey = "name" | "modified" | "size" | "type";
type SortDirection = "asc" | "desc";
type Notice = { type: "success" | "error" | "info"; text: string };

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
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

function fileExtension(name: string): string {
  const index = name.lastIndexOf(".");
  return index > 0 ? name.slice(index + 1).toLocaleLowerCase() : "";
}

function categoryOf(node: DriveNode): Exclude<FileCategory, "all"> | "folder" {
  if (node.nodeType === "folder") return "folder";
  const mime = (node.mimeType ?? "").toLocaleLowerCase();
  const extension = fileExtension(node.name);
  if (mime.startsWith("image/")) return "image";
  if (mime.startsWith("video/")) return "video";
  if (mime.startsWith("audio/")) return "audio";
  if (
    mime.startsWith("text/") ||
    mime.includes("pdf") ||
    mime.includes("document") ||
    mime.includes("spreadsheet") ||
    mime.includes("presentation") ||
    ["md", "txt", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "rtf"].includes(extension)
  ) {
    return "document";
  }
  if (
    mime.includes("zip") ||
    mime.includes("compressed") ||
    ["zip", "rar", "7z", "tar", "gz", "bz2", "xz"].includes(extension)
  ) {
    return "archive";
  }
  return "other";
}

function categoryLabel(category: ReturnType<typeof categoryOf>): string {
  return {
    folder: "文件夹",
    image: "图片",
    video: "视频",
    audio: "音频",
    document: "文档",
    archive: "压缩包",
    other: "其他",
  }[category];
}

function nodeIcon(node: DriveNode, size = 18) {
  if (node.nodeType === "folder") return <Folder size={size} />;
  const category = categoryOf(node);
  if (category === "image") return <FileImage size={size} />;
  if (category === "video") return <FileVideo size={size} />;
  if (category === "audio") return <FileAudio size={size} />;
  if (category === "archive") return <FileArchive size={size} />;
  if (category === "document") return <FileText size={size} />;
  return <File size={size} />;
}

function isRecent(node: DriveNode): boolean {
  const modified = new Date(node.modifiedAt).getTime();
  return Number.isFinite(modified) && Date.now() - modified <= RECENT_WINDOW_MS;
}

function pathDepth(path: string): number {
  return path.split("/").filter(Boolean).length;
}

function rootSelections(nodes: DriveNode[]): DriveNode[] {
  return nodes.filter(
    (candidate) =>
      !nodes.some(
        (other) =>
          other.id !== candidate.id &&
          other.nodeType === "folder" &&
          candidate.logicalPath.startsWith(`${other.logicalPath.replace(/\/$/, "")}/`),
      ),
  );
}

function safeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function compareNodes(left: DriveNode, right: DriveNode, key: SortKey): number {
  if (key === "size") return left.size - right.size;
  if (key === "modified") {
    return new Date(left.modifiedAt).getTime() - new Date(right.modifiedAt).getTime();
  }
  if (key === "type") {
    return categoryLabel(categoryOf(left)).localeCompare(categoryLabel(categoryOf(right)), "zh-CN");
  }
  return left.name.localeCompare(right.name, "zh-CN", { numeric: true, sensitivity: "base" });
}

export default function FileManagerApp() {
  const [classicMode, setClassicMode] = useState(false);
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [hasToken, setHasToken] = useState(false);
  const [nodes, setNodes] = useState<DriveNode[]>([]);
  const [queue, setQueue] = useState<DriveQueueSnapshot>(EMPTY_QUEUE);
  const [folderId, setFolderId] = useState<string | null>(null);
  const [smartView, setSmartView] = useState<SmartView>("folder");
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<FileCategory>("all");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [showInspector, setShowInspector] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  const driveReady = Boolean(config.driveDataSourceId.trim());
  const currentFolder = nodes.find((node) => node.id === folderId && node.nodeType === "folder") ?? null;
  const focused = nodes.find((node) => node.id === focusedId) ?? null;

  useEffect(() => {
    void loadLocalData();
    const queueListener = listen<DriveQueueSnapshot>("drive-queue-changed", (event) => {
      setQueue(event.payload);
      void refreshLocal();
    });
    const errorListener = listen<string>("drive-queue-error", (event) => {
      setNotice({ type: "error", text: `上传队列异常：${event.payload}` });
    });
    return () => {
      void queueListener.then((dispose) => dispose());
      void errorListener.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    const existing = new Set(nodes.map((node) => node.id));
    setSelectedIds((current) => new Set([...current].filter((id) => existing.has(id))));
    if (focusedId && !existing.has(focusedId)) setFocusedId(null);
    if (folderId && !existing.has(folderId)) setFolderId(null);
  }, [nodes, focusedId, folderId]);

  async function loadLocalData() {
    try {
      const [savedConfig, savedToken, savedNodes, savedQueue] = await Promise.all([
        invoke<AppConfig>("get_saved_config"),
        invoke<boolean>("has_saved_token"),
        invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }).catch(() => []),
        invoke<DriveQueueSnapshot>("get_drive_upload_queue").catch(() => EMPTY_QUEUE),
      ]);
      setConfig({ ...DEFAULT_CONFIG, ...savedConfig });
      setHasToken(savedToken);
      setNodes(savedNodes);
      setQueue(savedQueue);
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
    }
  }

  async function refreshLocal() {
    const [savedConfig, savedNodes, savedQueue] = await Promise.all([
      invoke<AppConfig>("get_saved_config"),
      invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }),
      invoke<DriveQueueSnapshot>("get_drive_upload_queue").catch(() => EMPTY_QUEUE),
    ]);
    setConfig({ ...DEFAULT_CONFIG, ...savedConfig });
    setNodes(savedNodes);
    setQueue(savedQueue);
  }

  const activeNodes = useMemo(() => nodes.filter((node) => node.status === "active"), [nodes]);
  const activeFiles = useMemo(
    () => activeNodes.filter((node) => node.nodeType === "file"),
    [activeNodes],
  );
  const activeFolders = useMemo(
    () => activeNodes.filter((node) => node.nodeType === "folder"),
    [activeNodes],
  );

  const duplicateGroups = useMemo(() => {
    const grouped = new Map<string, DriveNode[]>();
    for (const node of activeFiles) {
      if (!node.sha256) continue;
      const group = grouped.get(node.sha256) ?? [];
      group.push(node);
      grouped.set(node.sha256, group);
    }
    return [...grouped.values()].filter((group) => group.length > 1);
  }, [activeFiles]);

  const duplicateIds = useMemo(
    () => new Set(duplicateGroups.flatMap((group) => group.map((node) => node.id))),
    [duplicateGroups],
  );

  const duplicateCountByHash = useMemo(() => {
    const result = new Map<string, number>();
    for (const group of duplicateGroups) {
      if (group[0]?.sha256) result.set(group[0].sha256, group.length);
    }
    return result;
  }, [duplicateGroups]);

  const stats = useMemo(() => {
    const bytes = activeFiles.reduce((total, node) => total + node.size, 0);
    const duplicateWaste = duplicateGroups.reduce(
      (total, group) => total + (group[0]?.size ?? 0) * Math.max(0, group.length - 1),
      0,
    );
    return {
      bytes,
      files: activeFiles.length,
      folders: activeFolders.length,
      recent: activeNodes.filter(isRecent).length,
      large: activeFiles.filter((node) => node.size >= LARGE_FILE_BYTES).length,
      duplicateFiles: duplicateIds.size,
      duplicateWaste,
      trashed: nodes.filter((node) => node.status === "trashed").length,
    };
  }, [activeFiles, activeFolders, activeNodes, duplicateGroups, duplicateIds, nodes]);

  const breadcrumbs = useMemo(() => {
    const result: DriveNode[] = [];
    let cursor = currentFolder;
    const visited = new Set<string>();
    while (cursor && !visited.has(cursor.id)) {
      visited.add(cursor.id);
      result.unshift(cursor);
      cursor = cursor.parentId
        ? nodes.find((node) => node.id === cursor?.parentId && node.nodeType === "folder") ?? null
        : null;
    }
    return result;
  }, [currentFolder, nodes]);

  const visibleNodes = useMemo(() => {
    const text = query.trim().toLocaleLowerCase();
    let scoped: DriveNode[];

    if (smartView === "folder") {
      if (text) {
        const prefix = currentFolder ? `${currentFolder.logicalPath.replace(/\/$/, "")}/` : "/";
        scoped = activeNodes.filter((node) =>
          currentFolder ? node.logicalPath.startsWith(prefix) : node.logicalPath.startsWith("/"),
        );
      } else {
        scoped = activeNodes.filter((node) => (node.parentId ?? null) === folderId);
      }
    } else if (smartView === "all") {
      scoped = activeNodes;
    } else if (smartView === "recent") {
      scoped = activeNodes.filter(isRecent);
    } else if (smartView === "large") {
      scoped = activeFiles.filter((node) => node.size >= LARGE_FILE_BYTES);
    } else if (smartView === "duplicates") {
      scoped = activeFiles.filter((node) => duplicateIds.has(node.id));
    } else if (smartView === "files") {
      scoped = activeFiles;
    } else if (smartView === "folders") {
      scoped = activeFolders;
    } else {
      scoped = nodes.filter((node) => node.status === "trashed");
    }

    return scoped
      .filter((node) => {
        if (!text) return true;
        return [node.name, node.logicalPath, node.mimeType ?? "", node.sha256 ?? ""]
          .some((value) => value.toLocaleLowerCase().includes(text));
      })
      .filter((node) => category === "all" || categoryOf(node) === category)
      .sort((left, right) => {
        if (smartView === "duplicates" && left.sha256 !== right.sha256) {
          return (left.sha256 ?? "").localeCompare(right.sha256 ?? "");
        }
        if (left.nodeType !== right.nodeType && smartView === "folder") {
          return left.nodeType === "folder" ? -1 : 1;
        }
        const result = compareNodes(left, right, sortKey);
        return sortDirection === "asc" ? result : -result;
      });
  }, [
    activeFiles,
    activeFolders,
    activeNodes,
    category,
    currentFolder,
    duplicateIds,
    folderId,
    nodes,
    query,
    smartView,
    sortDirection,
    sortKey,
  ]);

  const selectedNodes = useMemo(
    () => nodes.filter((node) => selectedIds.has(node.id)),
    [nodes, selectedIds],
  );

  const selectedSize = useMemo(
    () => selectedNodes.reduce((total, node) => total + node.size, 0),
    [selectedNodes],
  );

  const viewCopy: Record<SmartView, { title: string; description: string }> = {
    folder: {
      title: currentFolder?.name ?? "我的云盘",
      description: currentFolder?.logicalPath ?? "根目录",
    },
    all: { title: "全部项目", description: "跨目录查看所有正常文件和文件夹" },
    recent: { title: "最近修改", description: "最近 30 天内修改的项目" },
    large: { title: "大文件", description: `单个文件不小于 ${formatBytes(LARGE_FILE_BYTES)}` },
    duplicates: { title: "重复文件", description: "按 SHA-256 识别内容完全相同的文件" },
    files: { title: "全部文件", description: "仅显示文件，不包含文件夹" },
    folders: { title: "全部文件夹", description: "集中检查目录结构" },
    trash: { title: "回收站", description: "已删除项目仍保留在 Notion 远端索引中" },
  };

  function switchView(next: SmartView) {
    setSmartView(next);
    setSelectedIds(new Set());
    setFocusedId(null);
    if (next !== "folder") setFolderId(null);
  }

  function openFolder(node: DriveNode | null) {
    setSmartView("folder");
    setFolderId(node?.id ?? null);
    setQuery("");
    setSelectedIds(new Set());
    setFocusedId(null);
  }

  function toggleSelected(nodeId: string, checked?: boolean) {
    setSelectedIds((current) => {
      const next = new Set(current);
      const shouldSelect = checked ?? !next.has(nodeId);
      if (shouldSelect) next.add(nodeId);
      else next.delete(nodeId);
      return next;
    });
  }

  function selectVisible() {
    const visibleIds = visibleNodes.map((node) => node.id);
    const allSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedIds.has(id));
    setSelectedIds((current) => {
      const next = new Set(current);
      for (const id of visibleIds) {
        if (allSelected) next.delete(id);
        else next.add(id);
      }
      return next;
    });
  }

  async function refreshRemote() {
    setBusy("refresh");
    try {
      const remote = await invoke<DriveNode[]>("refresh_drive_index");
      setNodes(remote);
      setSelectedIds(new Set());
      setFocusedId(null);
      setNotice({ type: "success", text: `已从 Notion 重建 ${remote.length} 个节点。` });
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
    } finally {
      setBusy(null);
    }
  }

  async function createFolder() {
    const name = window.prompt("请输入新文件夹名称");
    if (!name?.trim()) return;
    setBusy("create-folder");
    try {
      await invoke<DriveNode>("create_drive_folder", {
        name: name.trim(),
        parentId: smartView === "folder" ? folderId : null,
      });
      await refreshLocal();
      setNotice({ type: "success", text: `文件夹“${name.trim()}”已创建。` });
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
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
        request: {
          filePaths: paths,
          parentId: smartView === "folder" ? folderId : null,
        },
      });
      setQueue(next);
      setNotice({ type: "success", text: `已将 ${paths.length} 个文件加入持久化上传队列。` });
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
    }
  }

  async function downloadNode(node: DriveNode) {
    if (node.status !== "active") return;
    if (node.nodeType === "folder") {
      const destination = await open({
        directory: true,
        multiple: false,
        title: `选择“${node.name}”的下载位置`,
        defaultPath: config.downloadDirectory || undefined,
      });
      if (typeof destination !== "string") return;
      setBusy(`download-${node.id}`);
      try {
        const result = await invoke<DriveFolderDownloadResult>("download_drive_folder", {
          request: { folderId: node.id, destinationDirectory: destination },
        });
        const nextConfig = { ...config, downloadDirectory: destination };
        setConfig(nextConfig);
        await invoke("save_config", { config: nextConfig });
        setNotice({
          type: result.failed ? "error" : "success",
          text: `文件夹下载完成：成功 ${result.succeeded}，失败 ${result.failed}。`,
        });
      } catch (error) {
        setNotice({ type: "error", text: safeError(error) });
      } finally {
        setBusy(null);
      }
      return;
    }

    const defaultPath = config.downloadDirectory
      ? `${config.downloadDirectory.replace(/[\\/]$/, "")}/${node.name}`
      : node.name;
    const destination = await save({ title: "保存云盘文件", defaultPath });
    if (!destination) return;
    setBusy(`download-${node.id}`);
    try {
      await invoke<DriveTransfer>("download_drive_file", {
        request: { nodeId: node.id, destinationPath: destination },
      });
      const separator = Math.max(destination.lastIndexOf("/"), destination.lastIndexOf("\\"));
      if (separator > 0) {
        const nextConfig = { ...config, downloadDirectory: destination.slice(0, separator) };
        setConfig(nextConfig);
        await invoke("save_config", { config: nextConfig });
      }
      setNotice({ type: "success", text: `下载并校验完成：${destination}` });
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
    } finally {
      setBusy(null);
    }
  }

  async function renameNode(node: DriveNode) {
    const name = window.prompt("请输入新名称", node.name);
    if (!name?.trim() || name.trim() === node.name) return;
    setBusy(`rename-${node.id}`);
    try {
      await invoke<DriveNode>("rename_drive_node", {
        nodeId: node.id,
        newName: name.trim(),
      });
      await refreshLocal();
      setNotice({ type: "success", text: "重命名完成。" });
    } catch (error) {
      setNotice({ type: "error", text: safeError(error) });
    } finally {
      setBusy(null);
    }
  }

  function chooseTargetFolder(nodesToMove: DriveNode[]): DriveNode | null | undefined {
    const choices = activeFolders
      .filter((folder) => !nodesToMove.some((node) => node.id === folder.id))
      .map((folder) => folder.logicalPath)
      .sort((left, right) => left.localeCompare(right, "zh-CN"))
      .join("\n");
    const targetPath = window.prompt(
      `输入目标文件夹路径，/ 表示根目录：\n\n${choices}`,
      currentFolder?.logicalPath ?? "/",
    );
    if (targetPath === null) return undefined;
    const normalized = targetPath.trim() || "/";
    if (normalized === "/") return null;
    const target = activeFolders.find((folder) => folder.logicalPath === normalized);
    if (!target) {
      setNotice({ type: "error", text: `找不到目标文件夹：${normalized}` });
      return undefined;
    }
    return target;
  }

  async function moveSelection(nodesToMove: DriveNode[]) {
    const roots = rootSelections(nodesToMove.filter((node) => node.status === "active"));
    if (!roots.length) return;
    const target = chooseTargetFolder(roots);
    if (target === undefined) return;
    setBusy("batch-move");
    const failures: string[] = [];
    try {
      for (const node of roots) {
        try {
          await invoke<DriveNode>("move_drive_node", {
            nodeId: node.id,
            newParentId: target?.id ?? null,
          });
        } catch (error) {
          failures.push(`${node.name}：${safeError(error)}`);
        }
      }
      await refreshLocal();
      setSelectedIds(new Set());
      setFocusedId(null);
      setNotice({
        type: failures.length ? "error" : "success",
        text: failures.length
          ? `已移动 ${roots.length - failures.length} 项，失败 ${failures.length} 项：${failures.join("；")}`
          : `已移动 ${roots.length} 项。`,
      });
    } finally {
      setBusy(null);
    }
  }

  async function setSelectionTrashed(nodesToChange: DriveNode[], trashed: boolean) {
    const candidates = nodesToChange.filter((node) => (trashed ? node.status === "active" : node.status === "trashed"));
    const roots = rootSelections(candidates).sort((left, right) => {
      const delta = pathDepth(left.logicalPath) - pathDepth(right.logicalPath);
      return trashed ? -delta : delta;
    });
    if (!roots.length) return;
    if (trashed && !window.confirm(`将选中的 ${roots.length} 个根节点移入回收站？文件夹会包含其全部子项。`)) {
      return;
    }
    setBusy(trashed ? "batch-trash" : "batch-restore");
    const failures: string[] = [];
    let changed = 0;
    try {
      for (const node of roots) {
        try {
          changed += await invoke<number>("set_drive_node_trashed", {
            nodeId: node.id,
            trashed,
          });
        } catch (error) {
          failures.push(`${node.name}：${safeError(error)}`);
        }
      }
      await refreshLocal();
      setSelectedIds(new Set());
      setFocusedId(null);
      setNotice({
        type: failures.length ? "error" : "success",
        text: failures.length
          ? `已处理 ${changed} 个节点，失败 ${failures.length} 项：${failures.join("；")}`
          : trashed
            ? `已将 ${changed} 个节点移入回收站。`
            : `已恢复 ${changed} 个节点。`,
      });
    } finally {
      setBusy(null);
    }
  }

  async function openNotionPage(node: DriveNode) {
    if (!node.notionPageUrl) return;
    try {
      const url = new URL(node.notionPageUrl);
      if (!(["https:", "http:"] as string[]).includes(url.protocol)) {
        throw new Error(`不支持打开 ${url.protocol} 链接`);
      }
      await openUrl(url.toString());
    } catch (error) {
      setNotice({ type: "error", text: `无法打开 Notion 页面：${safeError(error)}` });
    }
  }

  if (classicMode) {
    return (
      <div className="fm-classic-host">
        <button className="fm-return-button" onClick={() => setClassicMode(false)}>
          <Layers3 size={16} />返回文件管理中心
        </button>
        <App />
      </div>
    );
  }

  const allVisibleSelected =
    visibleNodes.length > 0 && visibleNodes.every((node) => selectedIds.has(node.id));
  const queueBadge = queue.pendingCount + queue.failedCount + (queue.workerRunning ? 1 : 0);

  return (
    <div className="fm-shell">
      <aside className="fm-sidebar">
        <div className="fm-brand">
          <div className="fm-brand-mark"><Cloud size={21} /></div>
          <div><strong>Notion File</strong><span>文件管理中心</span></div>
        </div>

        <nav className="fm-nav">
          <span className="fm-nav-label">位置</span>
          <button className={smartView === "folder" ? "active" : ""} onClick={() => openFolder(null)}>
            <HardDrive size={17} /><span>我的云盘</span><em>{stats.files + stats.folders}</em>
          </button>
          <button className={smartView === "all" ? "active" : ""} onClick={() => switchView("all")}>
            <Files size={17} /><span>全部项目</span><em>{stats.files + stats.folders}</em>
          </button>
          <button className={smartView === "files" ? "active" : ""} onClick={() => switchView("files")}>
            <FileText size={17} /><span>全部文件</span><em>{stats.files}</em>
          </button>
          <button className={smartView === "folders" ? "active" : ""} onClick={() => switchView("folders")}>
            <Folder size={17} /><span>全部文件夹</span><em>{stats.folders}</em>
          </button>

          <span className="fm-nav-label">智能分类</span>
          <button className={smartView === "recent" ? "active" : ""} onClick={() => switchView("recent")}>
            <Clock3 size={17} /><span>最近修改</span><em>{stats.recent}</em>
          </button>
          <button className={smartView === "large" ? "active" : ""} onClick={() => switchView("large")}>
            <Database size={17} /><span>大文件</span><em>{stats.large}</em>
          </button>
          <button className={smartView === "duplicates" ? "active" : ""} onClick={() => switchView("duplicates")}>
            <Copy size={17} /><span>重复文件</span><em>{stats.duplicateFiles}</em>
          </button>
          <button className={smartView === "trash" ? "active" : ""} onClick={() => switchView("trash")}>
            <Trash2 size={17} /><span>回收站</span><em>{stats.trashed}</em>
          </button>
        </nav>

        <div className="fm-sidebar-footer">
          <div className={`fm-connection ${driveReady && hasToken ? "ready" : ""}`}>
            {driveReady && hasToken ? <CheckCircle2 size={16} /> : <AlertCircle size={16} />}
            <div><strong>{driveReady ? "云盘已连接" : "尚未初始化"}</strong><span>{hasToken ? "Token 已保存" : "需要配置 Token"}</span></div>
          </div>
          <button className="fm-classic-button" onClick={() => setClassicMode(true)}>
            <Settings2 size={16} />连接、传输与传统同步
            {queueBadge > 0 && <em>{queueBadge}</em>}
          </button>
        </div>
      </aside>

      <main className="fm-main">
        <header className="fm-topbar">
          <div className="fm-heading">
            <div className="fm-heading-icon">{smartView === "folder" ? <FolderOpen size={20} /> : <Layers3 size={20} />}</div>
            <div><h1>{viewCopy[smartView].title}</h1><p>{viewCopy[smartView].description}</p></div>
          </div>
          <div className="fm-top-actions">
            <label className="fm-search"><Search size={16} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索名称、路径、类型或哈希" />{query && <button onClick={() => setQuery("")}><X size={14} /></button>}</label>
            <button onClick={refreshRemote} disabled={!driveReady || Boolean(busy)} title="从 Notion 重建索引">
              <RefreshCw size={16} className={busy === "refresh" ? "spin" : ""} />
            </button>
            <button className="primary" onClick={uploadFiles} disabled={!driveReady || smartView === "trash" || Boolean(busy)}><Upload size={16} />上传</button>
            <button onClick={createFolder} disabled={!driveReady || smartView === "trash" || Boolean(busy)}><Plus size={16} />新建文件夹</button>
          </div>
        </header>

        <div className="fm-content">
          {notice && <div className={`fm-notice ${notice.type}`}>{notice.type === "error" ? <AlertCircle size={17} /> : notice.type === "success" ? <CheckCircle2 size={17} /> : <Cloud size={17} />}<span>{notice.text}</span><button onClick={() => setNotice(null)}><X size={14} /></button></div>}

          {!driveReady ? (
            <section className="fm-onboarding">
              <div><Cloud size={42} /></div>
              <h2>先连接 Notion Drive</h2>
              <p>文件管理中心依赖现有的 Notion 远端索引。连接后即可使用智能分类、重复文件识别和批量管理。</p>
              <button className="primary" onClick={() => setClassicMode(true)}><Settings2 size={16} />打开连接设置</button>
            </section>
          ) : (
            <>
              <section className="fm-stats">
                <article><div><HardDrive size={17} /></div><span>已索引容量</span><strong>{formatBytes(stats.bytes)}</strong><small>{stats.files} 个文件</small></article>
                <article><div><Folder size={17} /></div><span>目录结构</span><strong>{stats.folders}</strong><small>个文件夹</small></article>
                <article className={stats.duplicateFiles ? "warning" : ""}><div><Copy size={17} /></div><span>重复占用</span><strong>{formatBytes(stats.duplicateWaste)}</strong><small>{stats.duplicateFiles} 个重复文件</small></article>
                <article className={queue.workerRunning ? "running" : ""}><div>{queue.workerRunning ? <LoaderCircle className="spin" size={17} /> : <Upload size={17} />}</div><span>上传队列</span><strong>{queue.pendingCount}</strong><small>{queue.workerRunning ? "正在后台上传" : queue.failedCount ? `${queue.failedCount} 个失败任务` : "队列空闲"}</small></article>
              </section>

              {smartView === "folder" && (
                <div className="fm-breadcrumbs">
                  <button onClick={() => openFolder(null)}><Home size={15} />根目录</button>
                  {breadcrumbs.map((folder) => <span key={folder.id}><ChevronRight size={14} /><button onClick={() => openFolder(folder)}>{folder.name}</button></span>)}
                </div>
              )}

              <section className="fm-toolbar">
                <div className="fm-filter-group">
                  <select value={category} onChange={(event) => setCategory(event.target.value as FileCategory)}>
                    <option value="all">全部类型</option>
                    <option value="image">图片</option>
                    <option value="video">视频</option>
                    <option value="audio">音频</option>
                    <option value="document">文档</option>
                    <option value="archive">压缩包</option>
                    <option value="other">其他文件</option>
                  </select>
                  <select value={sortKey} onChange={(event) => setSortKey(event.target.value as SortKey)}>
                    <option value="name">按名称排序</option>
                    <option value="modified">按修改时间排序</option>
                    <option value="size">按大小排序</option>
                    <option value="type">按类型排序</option>
                  </select>
                  <button onClick={() => setSortDirection((value) => value === "asc" ? "desc" : "asc")} title="切换排序方向">
                    {sortDirection === "asc" ? <ArrowDownAZ size={16} /> : <ArrowUpAZ size={16} />}
                    {sortDirection === "asc" ? "升序" : "降序"}
                  </button>
                </div>
                <div className="fm-result-summary">
                  <span>{visibleNodes.length} 项</span>
                  {query && <em>搜索范围：{smartView === "folder" ? "当前目录及子目录" : viewCopy[smartView].title}</em>}
                  <button onClick={() => setShowInspector((value) => !value)} title="显示或隐藏详情面板">
                    {showInspector ? <PanelRightClose size={16} /> : <PanelRightOpen size={16} />}
                  </button>
                </div>
              </section>

              <section className={`fm-workspace ${showInspector ? "with-inspector" : ""}`}>
                <div className="fm-list-panel">
                  <div className="fm-table-header">
                    <button className="fm-check" onClick={selectVisible} title={allVisibleSelected ? "取消全选" : "选择当前列表"}>
                      {allVisibleSelected ? <Check size={14} /> : <Square size={14} />}
                    </button>
                    <span>名称</span><span>类型</span><span>大小</span><span>修改时间</span>
                  </div>
                  <div className="fm-table-body">
                    {visibleNodes.length === 0 ? (
                      <div className="fm-empty"><FolderOpen size={38} /><strong>{query ? "没有匹配的项目" : "当前分类为空"}</strong><span>可以调整搜索、类型过滤或切换其他智能分类。</span></div>
                    ) : visibleNodes.map((node) => {
                      const selected = selectedIds.has(node.id);
                      const duplicateCount = node.sha256 ? duplicateCountByHash.get(node.sha256) : undefined;
                      return (
                        <div
                          className={`fm-row ${focusedId === node.id ? "focused" : ""} ${selected ? "selected" : ""}`}
                          key={node.id}
                          role="button"
                          tabIndex={0}
                          onClick={() => setFocusedId(node.id)}
                          onDoubleClick={() => node.nodeType === "folder" && node.status === "active" ? openFolder(node) : undefined}
                          onKeyDown={(event) => {
                            if (event.key === "Enter" && node.nodeType === "folder" && node.status === "active") openFolder(node);
                            if (event.key === " ") { event.preventDefault(); toggleSelected(node.id); }
                          }}
                        >
                          <button className="fm-check" onClick={(event) => { event.stopPropagation(); toggleSelected(node.id); }}>
                            {selected ? <Check size={14} /> : <Square size={14} />}
                          </button>
                          <div className="fm-name-cell"><i className={categoryOf(node)}>{nodeIcon(node, 19)}</i><div><strong>{node.name}</strong><small>{node.logicalPath}</small></div>{duplicateCount && <em className="fm-duplicate-badge">重复 {duplicateCount}</em>}</div>
                          <span className="fm-type-cell">{categoryLabel(categoryOf(node))}</span>
                          <span>{node.nodeType === "folder" ? "—" : formatBytes(node.size)}</span>
                          <span>{formatDate(node.modifiedAt)}</span>
                        </div>
                      );
                    })}
                  </div>
                </div>

                {showInspector && (
                  <aside className="fm-inspector">
                    {focused ? (
                      <>
                        <div className="fm-inspector-title"><i className={categoryOf(focused)}>{nodeIcon(focused, 24)}</i><div><strong>{focused.name}</strong><span>{focused.logicalPath}</span></div></div>
                        <dl>
                          <div><dt>状态</dt><dd><em className={`fm-status ${focused.status}`}>{focused.status === "active" ? "正常" : "回收站"}</em></dd></div>
                          <div><dt>类型</dt><dd>{focused.nodeType === "folder" ? "文件夹" : focused.mimeType ?? categoryLabel(categoryOf(focused))}</dd></div>
                          <div><dt>大小</dt><dd>{focused.nodeType === "folder" ? "—" : formatBytes(focused.size)}</dd></div>
                          <div><dt>版本</dt><dd>v{focused.version}</dd></div>
                          <div><dt>创建时间</dt><dd>{formatDate(focused.createdAt)}</dd></div>
                          <div><dt>修改时间</dt><dd>{formatDate(focused.modifiedAt)}</dd></div>
                          {focused.sha256 && <div className="wide"><dt>SHA-256</dt><dd className="fm-hash">{focused.sha256}</dd></div>}
                        </dl>
                        {focused.sha256 && duplicateCountByHash.has(focused.sha256) && <div className="fm-insight"><Copy size={15} /><span>检测到 {duplicateCountByHash.get(focused.sha256)} 个内容完全相同的文件，可通过“重复文件”分类统一检查。</span></div>}
                        <div className="fm-inspector-actions">
                          {focused.status === "active" && focused.nodeType === "folder" && <button className="primary" onClick={() => openFolder(focused)}><FolderOpen size={15} />打开文件夹</button>}
                          {focused.status === "active" && <button className="primary" onClick={() => downloadNode(focused)} disabled={Boolean(busy)}><Download size={15} />下载</button>}
                          {focused.status === "active" && <button onClick={() => renameNode(focused)} disabled={Boolean(busy)}><Pencil size={15} />重命名</button>}
                          {focused.status === "active" && <button onClick={() => moveSelection([focused])} disabled={Boolean(busy)}><Move size={15} />移动</button>}
                          {focused.notionPageUrl && <button onClick={() => openNotionPage(focused)}><ExternalLink size={15} />Notion 页面</button>}
                          {focused.status === "active" ? <button className="danger" onClick={() => setSelectionTrashed([focused], true)} disabled={Boolean(busy)}><Trash2 size={15} />移入回收站</button> : <button onClick={() => setSelectionTrashed([focused], false)} disabled={Boolean(busy)}><RotateCcw size={15} />恢复</button>}
                        </div>
                      </>
                    ) : (
                      <div className="fm-inspector-empty"><Layers3 size={34} /><strong>选择一个项目</strong><span>这里会显示文件详情、哈希、版本和可用操作。</span></div>
                    )}
                  </aside>
                )}
              </section>
            </>
          )}
        </div>
      </main>

      {selectedNodes.length > 0 && (
        <div className="fm-selection-bar">
          <div><strong>已选择 {selectedNodes.length} 项</strong><span>{selectedSize ? `合计 ${formatBytes(selectedSize)}` : "包含文件夹"}</span></div>
          <div>
            {selectedNodes.length === 1 && selectedNodes[0].status === "active" && <button onClick={() => downloadNode(selectedNodes[0])} disabled={Boolean(busy)}><Download size={15} />下载</button>}
            {selectedNodes.some((node) => node.status === "active") && <button onClick={() => moveSelection(selectedNodes)} disabled={Boolean(busy)}><Move size={15} />批量移动</button>}
            {selectedNodes.some((node) => node.status === "active") && <button className="danger" onClick={() => setSelectionTrashed(selectedNodes, true)} disabled={Boolean(busy)}><Trash2 size={15} />移入回收站</button>}
            {selectedNodes.some((node) => node.status === "trashed") && <button onClick={() => setSelectionTrashed(selectedNodes, false)} disabled={Boolean(busy)}><RotateCcw size={15} />恢复</button>}
            <button className="icon" onClick={() => setSelectedIds(new Set())}><X size={16} /></button>
          </div>
        </div>
      )}

      {busy && <div className="fm-busy"><LoaderCircle className="spin" size={18} /><span>正在处理文件操作…</span></div>}
    </div>
  );
}
