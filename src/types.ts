export type SyncStatus =
  | "new"
  | "modified"
  | "unchanged"
  | "deleted"
  | "failed"
  | "synced";

export interface AppConfig {
  folderPath: string;
  rootPageId: string;
  archiveDeleted: boolean;
  skipHidden: boolean;
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

export interface SyncProgress {
  current: number;
  total: number;
  relativePath: string;
  stage: string;
}
