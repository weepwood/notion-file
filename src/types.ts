export type FileStatus = "new" | "modified" | "unchanged" | "backed_up" | "marked_deleted" | "failed" | "restored" | "skipped";

export interface BackupJob {
  id: string;
  name: string;
  folderPath: string;
  rootPageId: string;
  skipHidden: boolean;
  includeTextPreview: boolean;
  autoBackupMinutes: number;
  enabled: boolean;
  lastBackupAt?: string | null;
}

export interface AppConfig {
  jobs: BackupJob[];
  activeJobId?: string | null;
}

export interface ScannedFile {
  relativePath: string;
  absolutePath: string;
  size: number;
  modifiedAt: number;
  mimeType: string;
  status: FileStatus;
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

export interface BackupItemResult {
  relativePath: string;
  status: FileStatus;
  pageId?: string;
  message?: string;
}

export interface BackupResult {
  startedAt: string;
  finishedAt: string;
  snapshotPageId?: string | null;
  uploaded: number;
  unchanged: number;
  markedDeleted: number;
  failed: number;
  totalBytes: number;
  items: BackupItemResult[];
}

export interface BackupSnapshot {
  id: string;
  startedAt: string;
  finishedAt: string;
  summaryPageId?: string | null;
  totalFiles: number;
  totalBytes: number;
  uploaded: number;
  unchanged: number;
  markedDeleted: number;
  failed: number;
}

export interface RestoreResult {
  restored: number;
  skipped: number;
  failed: number;
  items: BackupItemResult[];
}

export interface BackupProgress {
  current: number;
  total: number;
  relativePath: string;
  stage: string;
}
