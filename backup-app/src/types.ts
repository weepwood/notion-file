export type Tab = "backup" | "restore" | "settings";

export interface BackupConfig {
  source_folder: string;
  root_page_id: string;
  backup_name: string;
  include_hidden: boolean;
  follow_symlinks: boolean;
  exclude_patterns: string[];
  max_file_size_mib: number;
  skip_unchanged_snapshot: boolean;
}

export type FileStatus = "new" | "changed" | "unchanged" | "skipped" | "error";

export interface ScanFile {
  relative_path: string;
  absolute_path: string;
  size: number;
  modified_ms: number;
  sha256?: string | null;
  status: FileStatus;
  message?: string | null;
}

export interface ScanResult {
  files: ScanFile[];
  deleted_paths: string[];
  total_files: number;
  total_bytes: number;
  new_count: number;
  changed_count: number;
  unchanged_count: number;
  skipped_count: number;
  error_count: number;
}

export interface ProgressPayload {
  phase: string;
  current: number;
  total: number;
  relative_path?: string | null;
  message: string;
  bytes_done?: number;
  bytes_total?: number;
}

export interface BackupResult {
  snapshot_id?: string | null;
  snapshot_page_id?: string | null;
  uploaded_files: number;
  reused_files: number;
  deleted_files: number;
  total_files: number;
  total_bytes: number;
  message: string;
}

export interface SnapshotRecord {
  id: string;
  page_id: string;
  manifest_block_id: string;
  created_at: string;
  file_count: number;
  total_bytes: number;
  uploaded_files: number;
  deleted_files: number;
}

export interface RestoreResult {
  restored_files: number;
  skipped_files: number;
  failed_files: number;
  restored_bytes: number;
  errors: string[];
}
