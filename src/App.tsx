import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
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
  DriveInitResult,
  DriveNode,
  DriveTransfer,
  DriveTransferProgress,
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

function formatBytes(bytes: number): string {
  if (!bytes) return "0 B";
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

function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || path;
}

function progressPercent(progress?: DriveTransferProgress | null): number {
  if (!progress?.totalBytes) return 0;
  return Math.min(100, Math.round((progress.transferredBytes / progress.totalBytes) * 100));
}

function nodeIcon(node: DriveNode) {
  if (node.nodeType === "folder") return <Folder size={18} />;
  return <FileText size={18} />;
}

function statusLabel(status: DriveTransfer["status"]): string {
  return {
    queued: "等待中",
    running: "传输中",
    completed: "已完成",
    failed: "失败",
    cancelled: "已取消",
  }[status];
}

export default function App() {
  const [view, setView] = useState<DriveView>("drive");
  const [config, setConfig] = useState<AppConfig>(DEFAULT_CONFIG);
  const [token, setToken] = useState("");
  const [hasToken, setHasToken] = useState(false);
  const [nodes, setNodes] = useState<DriveNode[]>([]);
  const [transfers, setTransfers] = useState<DriveTransfer[]>([]);
  const [currentFolderId, setCurrentFolderId] = useState<string | null>(null);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [showTrash, setShowTrash] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [activeProgress, setActiveProgress] = useState<DriveTransferProgress | null>(null);

  const [legacyFilePath, setLegacyFilePath] = useState("");
  const [legacyScan, setLegacyScan] = useState<ScanResult | null>(null);
  const [legacyResult, setLegacyResult] = useState<SyncResult | null>(null);

  const driveReady = Boolean(config.driveDataSourceId.trim());
  const credentialsReady = hasToken || Boolean(token.trim());
  const selectedNode = nodes.find((node) => node.id === selectedNodeId) || null;
  const currentFolder = nodes.find((node) => node.id === currentFolderId) || null;

  useEffect(() => {
    Promise.all([
      invoke<AppConfig>("get_saved_config"),
      invoke<boolean>("has_saved_token"),
      invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }).catch(() => []),
      invoke<DriveTransfer[]>("get_drive_transfers").catch(() => []),
    ])
      .then(([saved, savedToken, savedNodes, savedTransfers]) => {
        setConfig({ ...DEFAULT_CONFIG, ...saved });
        setHasToken(savedToken);
        setNodes(savedNodes);
        setTransfers(savedTransfers);
      })
      .catch((error) => setNotice({ type: "error", text: String(error) }));

    const unlisten = listen<DriveTransferProgress>("drive-transfer-progress", (event) => {
      setActiveProgress(event.payload);
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const breadcrumbs = useMemo(() => {
    const result: DriveNode[] = [];
    let cursor = currentFolder;
    const visited = new Set<string>();
    while (cursor && !visited.has(cursor.id)) {
      visited.add(cursor.id);
      result.unshift(cursor);
      cursor = cursor.parentId
        ? nodes.find((node) => node.id === cursor?.parentId) || undefined
        : undefined;
    }
    return result;
  }, [currentFolder, nodes]);

  const visibleNodes = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (showTrash) {
      return nodes
        .filter((node) => node.status === "trashed")
        .filter(
          (node) =>
            !normalizedQuery ||
            node.name.toLocaleLowerCase().includes(normalizedQuery) ||
            node.logicalPath.toLocaleLowerCase().includes(normalizedQuery),
        );
    }
    if (normalizedQuery) {
      return nodes.filter(
        (node) =>
          node.status === "active" &&
          (node.name.toLocaleLowerCase().includes(normalizedQuery) ||
            node.logicalPath.toLocaleLowerCase().includes(normalizedQuery)),
      );
    }
    return nodes.filter(
      (node) => node.status === "active" && (node.parentId || null) === currentFolderId,
    );
  }, [nodes, query, showTrash, currentFolderId]);

  const activeFolders = useMemo(
    () => nodes.filter((node) => node.status === "active" && node.nodeType === "folder"),
    [nodes],
  );

  const stats = useMemo(() => {
    const active = nodes.filter((node) => node.status === "active");
    return {
      files: active.filter((node) => node.nodeType === "file").length,
      folders: active.filter((node) => node.nodeType === "folder").length,
      bytes: active.reduce((sum, node) => sum + node.size, 0),
      trashed: nodes.filter((node) => node.status === "trashed").length,
    };
  }, [nodes]);

  async function refreshLocalData(includeNotice = false) {
    const [nextNodes, nextTransfers, nextConfig] = await Promise.all([
      invoke<DriveNode[]>("get_drive_nodes", { includeTrashed: true }),
      invoke<DriveTransfer[]>("get_drive_transfers"),
      invoke<AppConfig>("get_saved_config"),
    ]);
    setNodes(nextNodes);
    setTransfers(nextTransfers);
    setConfig({ ...DEFAULT_CONFIG, ...nextConfig });
    if (includeNotice) {
      setNotice({ type: "success", text: "本地云盘索引已刷新。" });
    }
  }

  async function persistSettings() {
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
      await persistSettings();
      const result = await invoke<DriveInitResult>("initialize_drive", {
        rootPageId: config.rootPageId.trim(),
      });
      setConfig((current) => ({
        ...current,
        driveDatabaseId: result.databaseId,
        driveDataSourceId: result.dataSourceId,
      }));
      await refreshLocalData();
      setNotice({
        type: "success",
        text: result.created
          ? `已创建 Notion Drive，共恢复 ${result.nodeCount} 个节点。`
          : `已连接现有 Notion Drive，共恢复 ${result.nodeCount} 个节点。`,
      });
      setView("drive");
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function refreshRemoteIndex() {
    setBusy("refresh");
    setNotice(null);
    try {
      const remoteNodes = await invoke<DriveNode[]>("refresh_drive_index");
      setNodes(remoteNodes);
      setSelectedNodeId(null);
      setNotice({ type: "success", text: `远端索引重建完成，共 ${remoteNodes.length} 个节点。` });
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
      const folder = await invoke<DriveNode>("create_drive_folder", {
        name: name.trim(),
        parentId: currentFolderId,
      });
      setNodes((current) => [...current, folder]);
      setNotice({ type: "success", text: `已创建文件夹：${folder.logicalPath}` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function chooseAndUploadFiles() {
    const selected = await open({
      directory: false,
      multiple: true,
      title: "选择需要上传到 Notion Drive 的文件",
    });
    const paths = Array.isArray(selected) ? selected : typeof selected === "string" ? [selected] : [];
    if (!paths.length) return;

    setBusy("upload");
    setNotice({ type: "info", text: `已加入 ${paths.length} 个文件，正在顺序上传。` });
    let succeeded = 0;
    const failures: string[] = [];
    for (const path of paths) {
      try {
        await invoke<DriveNode>("upload_drive_file", {
          request: { filePath: path, parentId: currentFolderId },
        });
        succeeded += 1;
      } catch (error) {
        failures.push(`${fileName(path)}：${String(error)}`);
      }
      await refreshLocalData();
    }
    setBusy(null);
    setActiveProgress(null);
    if (failures.length) {
      setNotice({
        type: "error",
        text: `上传完成：成功 ${succeeded}，失败 ${failures.length}。${failures[0]}`,
      });
    } else {
      setNotice({ type: "success", text: `${succeeded} 个文件已上传到 Notion Drive。` });
    }
  }

  async function downloadSelected() {
    if (!selectedNode || selectedNode.nodeType !== "file") return;
    const destination = await save({
      title: "保存云盘文件",
      defaultPath: config.downloadDirectory
        ? `${config.downloadDirectory.replace(/[\\/]$/, "")}/${selectedNode.name}`
        : selectedNode.name,
    });
    if (!destination) return;

    setBusy("download");
    try {
      await invoke<DriveTransfer>("download_drive_file", {
        request: { nodeId: selectedNode.id, destinationPath: destination },
      });
      const separator = Math.max(destination.lastIndexOf("/"), destination.lastIndexOf("\\"));
      if (separator > 0) {
        const nextConfig = { ...config, downloadDirectory: destination.slice(0, separator) };
        setConfig(nextConfig);
        await invoke("save_config", { config: nextConfig });
      }
      await refreshLocalData();
      setNotice({ type: "success", text: `下载完成：${destination}` });
    } catch (error) {
      await refreshLocalData();
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
      setActiveProgress(null);
    }
  }

  async function renameSelected() {
    if (!selectedNode) return;
    const name = window.prompt("请输入新名称", selectedNode.name);
    if (!name?.trim() || name.trim() === selectedNode.name) return;
    setBusy("rename");
    try {
      await invoke<DriveNode>("rename_drive_node", {
        nodeId: selectedNode.id,
        newName: name.trim(),
      });
      await refreshLocalData();
      setNotice({ type: "success", text: "重命名完成。" });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function moveSelected() {
    if (!selectedNode) return;
    const folderList = activeFolders
      .filter((folder) => folder.id !== selectedNode.id)
      .map((folder) => folder.logicalPath)
      .sort()
      .join("\n");
    const targetPath = window.prompt(
      `请输入目标文件夹路径，输入 / 表示根目录：\n\n${folderList}`,
      currentFolder?.logicalPath || "/",
    );
    if (targetPath === null) return;
    const normalized = targetPath.trim() || "/";
    const target = normalized === "/" ? null : activeFolders.find((folder) => folder.logicalPath === normalized);
    if (normalized !== "/" && !target) {
      setNotice({ type: "error", text: `找不到目标文件夹：${normalized}` });
      return;
    }
    setBusy("move");
    try {
      await invoke<DriveNode>("move_drive_node", {
        nodeId: selectedNode.id,
        newParentId: target?.id || null,
      });
      await refreshLocalData();
      setSelectedNodeId(null);
      setNotice({ type: "success", text: "移动完成。" });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function setSelectedTrashed(trashed: boolean) {
    if (!selectedNode) return;
    const confirmed = trashed
      ? window.confirm(`将“${selectedNode.name}”移入回收站？文件夹中的所有内容也会一起隐藏。`)
      : true;
    if (!confirmed) return;
    setBusy("trash");
    try {
      const count = await invoke<number>("set_drive_node_trashed", {
        nodeId: selectedNode.id,
        trashed,
      });
      await refreshLocalData();
      setSelectedNodeId(null);
      setNotice({
        type: "success",
        text: trashed ? `已将 ${count} 个节点移入回收站。` : `已恢复 ${count} 个节点。`,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function clearTransfers() {
    try {
      const count = await invoke<number>("clear_finished_drive_transfers");
      await refreshLocalData();
      setNotice({ type: "success", text: `已清理 ${count} 条已结束的传输记录。` });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    }
  }

  async function disconnectDrive() {
    if (!window.confirm("仅断开本机索引，不会删除 Notion 中的文件。确定继续？")) return;
    await invoke("disconnect_drive");
    await refreshLocalData();
    setCurrentFolderId(null);
    setSelectedNodeId(null);
    setNotice({ type: "info", text: "已断开本机云盘索引，可在设置中重新连接。" });
  }

  async function chooseLegacyFile() {
    const selected = await open({ directory: false, multiple: false, title: "选择单个文件" });
    if (typeof selected === "string") setLegacyFilePath(selected);
  }

  async function uploadLegacyFile() {
    if (!legacyFilePath) return;
    setBusy("legacy-upload");
    try {
      const record = await invoke<UploadRecord>("upload_single_file", {
        request: {
          filePath: legacyFilePath,
          rootPageId: config.rootPageId,
          displayMode: "file",
        },
      });
      setNotice({
        type: record.status === "success" ? "success" : "error",
        text: record.message || record.status,
      });
    } catch (error) {
      setNotice({ type: "error", text: String(error) });
    } finally {
      setBusy(null);
    }
  }

  async function chooseLegacyFolder() {
    const selected = await open({ directory: true, multiple: false, title: "选择同步文件夹" });
    if (typeof selected !== "string") return;
    const next = { ...config, folderPath: selected };
    setConfig(next);
    await invoke("save_config", { config: next });
    setBusy("legacy-scan");
    try {
      const result = await invoke<ScanResult>("scan_folder", {
        folderPath: selected,
        skipHidden: next.skipHidden,
      });
      setLegacyScan(result);
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

  return (
    <div className="drive-shell">
      <aside className="drive-sidebar">
        <div className="brand">
          <div className="brand-icon"><Cloud size={22} /></div>
          <div><strong>Notion File</strong><span>个人云盘 · v0.5.0</span></div>
        </div>

        <nav>
          <button className={view === "drive" ? "active" : ""} onClick={() => setView("drive")}>
            <HardDrive size={17} />我的云盘
          </button>
          <button className={view === "transfers" ? "active" : ""} onClick={() => setView("transfers")}>
            <ListChecks size={17} />传输中心
          </button>
          <button className={view === "legacy" ? "active" : ""} onClick={() => setView("legacy")}>
            <FolderSync size={17} />传统同步
          </button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            <Settings size={17} />连接设置
          </button>
        </nav>

        <div className={`connection-card ${driveReady && hasToken ? "ready" : ""}`}>
          {driveReady && hasToken ? <CheckCircle2 size={18} /> : <AlertCircle size={18} />}
          <div>
            <strong>{driveReady ? "云盘已连接" : "云盘未初始化"}</strong>
            <span>{hasToken ? "Token 已安全保存" : "需要保存 Notion Token"}</span>
          </div>
        </div>
      </aside>

      <main className="drive-main">
        <header className="drive-topbar">
          <div>
            <strong>{view === "drive" ? (showTrash ? "回收站" : "我的云盘") : view === "transfers" ? "传输中心" : view === "legacy" ? "传统上传与同步" : "连接设置"}</strong>
            <span>{driveReady ? `Data Source ${config.driveDataSourceId.slice(0, 8)}…` : "使用 Notion Database 保存远端文件索引"}</span>
          </div>
          {busy && <div className="busy-label"><LoaderCircle size={15} className="spin" />正在处理</div>}
        </header>

        <div className="drive-content">
          {notice && (
            <div className={`notice ${notice.type}`}>
              {notice.type === "error" ? <AlertCircle size={18} /> : notice.type === "success" ? <CheckCircle2 size={18} /> : <Cloud size={18} />}
              <span>{notice.text}</span>
              <button onClick={() => setNotice(null)}><X size={15} /></button>
            </div>
          )}

          {activeProgress && busy && (
            <div className="transfer-banner">
              <div>
                {activeProgress.direction === "upload" ? <Upload size={18} /> : <Download size={18} />}
                <span><strong>{activeProgress.fileName}</strong><small>{activeProgress.stage}</small></span>
              </div>
              <div className="progress-meta">{formatBytes(activeProgress.transferredBytes)} / {formatBytes(activeProgress.totalBytes)}</div>
              <div className="progress-track"><div style={{ width: `${progressPercent(activeProgress)}%` }} /></div>
            </div>
          )}

          {view === "drive" && (
            <section>
              {!driveReady ? (
                <div className="onboarding-card">
                  <div className="onboarding-icon"><Cloud size={38} /></div>
                  <h1>把 Notion 变成个人文件云盘</h1>
                  <p>程序会在指定父页面下创建专用数据库，用于保存虚拟目录、文件元数据和附件位置。本地 SQLite 作为快速缓存，可以随时从 Notion 重建。</p>
                  <button className="primary" onClick={() => setView("settings")}><Settings size={16} />开始配置</button>
                </div>
              ) : (
                <>
                  <div className="stats-grid">
                    <div><FileText size={18} /><strong>{stats.files}</strong><span>文件</span></div>
                    <div><Folder size={18} /><strong>{stats.folders}</strong><span>文件夹</span></div>
                    <div><Database size={18} /><strong>{formatBytes(stats.bytes)}</strong><span>已索引容量</span></div>
                    <div><Archive size={18} /><strong>{stats.trashed}</strong><span>回收站节点</span></div>
                  </div>

                  <div className="drive-toolbar">
                    <div className="toolbar-actions">
                      {!showTrash && <button className="primary" onClick={chooseAndUploadFiles} disabled={Boolean(busy)}><Upload size={16} />上传文件</button>}
                      {!showTrash && <button onClick={createFolder} disabled={Boolean(busy)}><Plus size={16} />新建文件夹</button>}
                      <button onClick={refreshRemoteIndex} disabled={Boolean(busy)}><RefreshCw size={16} />重建索引</button>
                      <button className={showTrash ? "active" : ""} onClick={() => { setShowTrash((value) => !value); setSelectedNodeId(null); }}><Trash2 size={16} />{showTrash ? "返回云盘" : "回收站"}</button>
                    </div>
                    <label className="search-box"><Search size={15} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索名称或路径" /></label>
                  </div>

                  {!showTrash && (
                    <div className="breadcrumbs">
                      <button onClick={() => setCurrentFolderId(null)}><Home size={15} />根目录</button>
                      {breadcrumbs.map((folder) => (
                        <span key={folder.id}><ChevronRight size={14} /><button onClick={() => setCurrentFolderId(folder.id)}>{folder.name}</button></span>
                      ))}
                    </div>
                  )}

                  <div className="drive-workspace">
                    <div className="file-panel">
                      <div className="file-table header-row"><span>名称</span><span>大小</span><span>修改时间</span><span>状态</span></div>
                      <div className="file-table-body">
                        {visibleNodes.length === 0 ? (
                          <div className="empty-state"><FolderOpen size={36} /><strong>{query ? "没有匹配的文件" : showTrash ? "回收站为空" : "这个文件夹为空"}</strong><span>{!showTrash && !query ? "上传文件或新建文件夹开始使用" : ""}</span></div>
                        ) : visibleNodes.map((node) => (
                          <button
                            key={node.id}
                            className={`file-table row ${selectedNodeId === node.id ? "selected" : ""}`}
                            onClick={() => setSelectedNodeId(node.id)}
                            onDoubleClick={() => node.nodeType === "folder" && node.status === "active" && setCurrentFolderId(node.id)}
                          >
                            <span className="file-name"><i>{nodeIcon(node)}</i><span><strong>{node.name}</strong><small>{node.logicalPath}</small></span></span>
                            <span>{node.nodeType === "folder" ? "—" : formatBytes(node.size)}</span>
                            <span>{formatDate(node.modifiedAt)}</span>
                            <span><em className={`node-status ${node.status}`}>{node.status === "active" ? "正常" : "已删除"}</em></span>
                          </button>
                        ))}
                      </div>
                    </div>

                    <aside className="inspector">
                      {selectedNode ? (
                        <>
                          <div className="inspector-title"><i>{nodeIcon(selectedNode)}</i><div><strong>{selectedNode.name}</strong><span>{selectedNode.logicalPath}</span></div></div>
                          <dl>
                            <div><dt>类型</dt><dd>{selectedNode.nodeType === "folder" ? "文件夹" : selectedNode.mimeType || "文件"}</dd></div>
                            <div><dt>大小</dt><dd>{formatBytes(selectedNode.size)}</dd></div>
                            <div><dt>版本</dt><dd>v{selectedNode.version}</dd></div>
                            <div><dt>修改时间</dt><dd>{formatDate(selectedNode.modifiedAt)}</dd></div>
                            {selectedNode.sha256 && <div><dt>SHA-256</dt><dd className="hash">{selectedNode.sha256}</dd></div>}
                          </dl>
                          <div className="inspector-actions">
                            {selectedNode.status === "active" && selectedNode.nodeType === "file" && <button className="primary" onClick={downloadSelected} disabled={Boolean(busy)}><Download size={15} />下载</button>}
                            {selectedNode.status === "active" && <button onClick={renameSelected} disabled={Boolean(busy)}><Pencil size={15} />重命名</button>}
                            {selectedNode.status === "active" && <button onClick={moveSelected} disabled={Boolean(busy)}><Move size={15} />移动</button>}
                            {selectedNode.notionPageUrl && <button onClick={() => window.open(selectedNode.notionPageUrl, "_blank")}><ExternalLink size={15} />Notion 页面</button>}
                            {selectedNode.status === "active" ? (
                              <button className="danger" onClick={() => setSelectedTrashed(true)} disabled={Boolean(busy)}><Trash2 size={15} />移入回收站</button>
                            ) : (
                              <button onClick={() => setSelectedTrashed(false)} disabled={Boolean(busy)}><RotateCcw size={15} />恢复</button>
                            )}
                          </div>
                        </>
                      ) : (
                        <div className="empty-inspector"><HardDrive size={30} /><strong>选择一个节点</strong><span>查看文件信息并执行下载、移动或重命名。</span></div>
                      )}
                    </aside>
                  </div>
                </>
              )}
            </section>
          )}

          {view === "transfers" && (
            <section>
              <div className="section-heading"><div><h1>传输中心</h1><p>上传与下载记录保存在 SQLite 中，应用异常退出后的未完成任务会标记为失败。</p></div><button onClick={clearTransfers}><Trash2 size={15} />清理已结束记录</button></div>
              <div className="transfer-list">
                {transfers.length === 0 ? <div className="empty-state"><History size={36} /><strong>暂无传输记录</strong></div> : transfers.map((transfer) => {
                  const percent = transfer.totalBytes ? Math.round((transfer.transferredBytes / transfer.totalBytes) * 100) : 0;
                  return <div className="transfer-item" key={transfer.id}>
                    <i>{transfer.direction === "upload" ? <Upload size={17} /> : <Download size={17} />}</i>
                    <div className="transfer-copy"><strong>{transfer.fileName}</strong><span>{transfer.message || transfer.localPath || ""}</span><div className="mini-progress"><div style={{ width: `${Math.min(100, percent)}%` }} /></div></div>
                    <div className="transfer-size">{formatBytes(transfer.transferredBytes)} / {formatBytes(transfer.totalBytes)}</div>
                    <em className={`transfer-status ${transfer.status}`}>{statusLabel(transfer.status)}</em>
                  </div>;
                })}
              </div>
            </section>
          )}

          {view === "settings" && (
            <section className="settings-page">
              <div className="section-heading"><div><h1>连接设置</h1><p>内部 Integration 必须被添加到父页面的 Connections 中，才有权限创建云盘数据库。</p></div></div>
              <div className="settings-card">
                <label><span><KeyRound size={16} />Notion Token</span><input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={hasToken ? "Token 已保存，留空表示不修改" : "secret_... 或 ntn_..."} /></label>
                <label><span><Link size={16} />父页面链接或 ID</span><input value={config.rootPageId} onChange={(event) => setConfig({ ...config, rootPageId: event.target.value })} placeholder="https://www.notion.so/..." /></label>
                <details>
                  <summary>连接已有云盘数据库</summary>
                  <label><span><Database size={16} />Database ID</span><input value={config.driveDatabaseId} onChange={(event) => setConfig({ ...config, driveDatabaseId: event.target.value })} placeholder="新建云盘时留空" /></label>
                  <label><span><Database size={16} />Data Source ID</span><input value={config.driveDataSourceId} onChange={(event) => setConfig({ ...config, driveDataSourceId: event.target.value })} placeholder="跨设备连接已有云盘时填写" /></label>
                </details>
                <div className="settings-actions">
                  <button onClick={persistSettings} disabled={!credentialsReady || Boolean(busy)}>保存设置</button>
                  <button className="primary" onClick={initializeDrive} disabled={!credentialsReady || !config.rootPageId.trim() || Boolean(busy)}>{busy === "initialize" ? <LoaderCircle size={15} className="spin" /> : <Cloud size={15} />}初始化或连接云盘</button>
                  {driveReady && <button className="danger" onClick={disconnectDrive}>断开本机索引</button>}
                </div>
              </div>
              <div className="info-card"><AlertCircle size={18} /><div><strong>数据安全说明</strong><p>断开本机索引不会删除 Notion 中的数据。文件下载前会重新获取短期有效的签名地址，完成后使用 SHA-256 校验。</p></div></div>
            </section>
          )}

          {view === "legacy" && (
            <section>
              <div className="section-heading"><div><h1>传统上传与文件夹同步</h1><p>保留 v0.4.0 的原有能力。新文件管理建议优先使用“我的云盘”。</p></div></div>
              <div className="legacy-grid">
                <div className="legacy-card">
                  <h2><Upload size={18} />单文件上传</h2>
                  <p>创建独立 Notion 页面并附加文件，支持大文件分片与超大视频切分。</p>
                  <div className="path-display">{legacyFilePath || "尚未选择文件"}</div>
                  <div className="card-actions"><button onClick={chooseLegacyFile}>选择文件</button><button className="primary" onClick={uploadLegacyFile} disabled={!legacyFilePath || Boolean(busy)}>上传</button></div>
                </div>
                <div className="legacy-card">
                  <h2><FolderSync size={18} />文件夹转文档</h2>
                  <p>扫描本地文件夹并重建为一篇 Notion 文档，适合一次性备份和资料归档。</p>
                  <div className="path-display">{config.folderPath || "尚未选择文件夹"}</div>
                  {legacyScan && <small>已扫描 {legacyScan.files.length} 个文件，共 {formatBytes(legacyScan.totalBytes)}</small>}
                  {legacyResult && <small>最近结果：创建 {legacyResult.created}，更新 {legacyResult.updated}，失败 {legacyResult.failed}</small>}
                  <div className="card-actions"><button onClick={chooseLegacyFolder}>选择并扫描</button><button className="primary" onClick={syncLegacyFolder} disabled={!config.folderPath || Boolean(busy)}>开始同步</button></div>
                </div>
              </div>
            </section>
          )}
        </div>
      </main>
    </div>
  );
}
