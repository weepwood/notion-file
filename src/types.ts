export type SyncStatus =
  | "new"
  | "modified"
  | "unchanged"
  | "deleted"
  | "failed"
  | "synced";

export type UploadDisplayMode = "file" | "video";
export type DriveView = "drive" | "transfers" | "legacy" | "settings";
export type DriveNodeStatus = "active" | "trashed";
export type TransferStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

export interface AppConfig {
  folderPath: string;
  rootPageId: string;
  archiveDeleted: boolean;
  skipHidden: boolean;
  driveDatabaseId: string;
  driveDataSourceId: string;
  downloadDirectory: string;
}

export interface DriveNode {
  id: string;
  parentId?: string;
  nodeType: "file" | "folder";
  name: string;
  logicalPath: string;
  mimeType?: string;
  size: number;
  sha256?: string;
  notionPageId: string;
  notionPageUrl?: string;
  notionBlockId?: string;
  fileUploadId?: string;
  status: DriveNodeStatus;
  version: number;
  originalPath?: string;
  createdAt: string;
  modifiedAt: string;
}

export interface DriveTransfer {
  id: string;
  nodeId?: string;
  direction: "upload" | "download";
  fileName: string;
  localPath?: string;
  status: TransferStatus;
  totalBytes: number;
  transferredBytes: number;
  message?: string;
  createdAt: string;
  updatedAt: string;
}

export interface DriveTransferProgress {
  transferId: string;
  nodeId?: string;
  direction: "upload" | "download";
  fileName: string;
  stage: string;
  transferredBytes: number;
  totalBytes: number;
}

export interface DriveInitResult {
  databaseId: string;
  dataSourceId: string;
  created: boolean;
  nodeCount: number;
}

export interface ScannedFile {
  relativePath: string;
  absolutePath: string;
  size: number;
  modifiedAt: number;
  mimeType: string;
  status: SyncStatus;
  hash: string;
}

export interface ScanResult {
  root: string;
  files: ScannedFile[];
  totalBytes: number;
  changedCount: number;
  unchangedCount: number;
  deletedCount: number;
}

export interface SyncRequest {
  folderPath: string;
  rootPageId: string;
  archiveDeleted: boolean;
  skipHidden: boolean;
}

export interface SyncItemResult {
  relativePath: string;
  status: SyncStatus;
  pageId?: string;
  message?: string;
}

export interface SyncResult {
  startedAt: string;
  finishedAt: string;
  documentTitle: string;
  pageId: string;
  pageUrl?: string;
  created: number;
  updated: number;
  unchanged: number;
  archived: number;
  failed: number;
  items: SyncItemResult[];
}

export interface FfmpegStatus {
  available: boolean;
  ffmpegPath?: string;
  ffprobePath?: string;
  version?: string;
  message: string;
}

export interface UploadRecord {
  id: string;
  filePath: string;
  fileName: string;
  size: number;
  mimeType: string;
  sha256: string;
  uploadedAt: string;
  status: "success" | "failed";
  pageId?: string;
  pageUrl?: string;
  message?: string;
  displayMode?: UploadDisplayMode;
  segmentCount?: number;
  usedFfmpeg?: boolean;
}

export interface SyncProgress {
  current: number;
  total: number;
  relativePath: string;
  stage: string;
}

export interface UploadProgress {
  current: number;
  total: number;
  stage: string;
  detail: string;
}
