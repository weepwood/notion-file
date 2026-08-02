use crate::models::{ScanResult, ScannedFile, TaskState};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path};
use walkdir::WalkDir;

const EXCLUDED_DIRS: &[&str] = &[".git", ".notion-backup", ".notion-sync", "node_modules", "target", "dist", "build", ".idea", ".vscode"];
const MAX_FILES: usize = 100_000;

fn is_hidden_or_excluded(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).unwrap_or(path).components().any(|component| {
        if let Component::Normal(value) = component {
            let name = value.to_string_lossy();
            EXCLUDED_DIRS.contains(&name.as_ref()) || name.starts_with('.')
        } else { false }
    })
}

fn sha256(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("无法打开文件：{}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn scan(root: &str, skip_hidden: bool, state: &TaskState) -> Result<ScanResult> {
    let root_path = Path::new(root);
    if !root_path.exists() { anyhow::bail!("文件夹不存在：{root}"); }
    if !root_path.is_dir() { anyhow::bail!("所选路径不是文件夹：{root}"); }

    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    let mut current_paths = HashSet::new();

    for entry in WalkDir::new(root_path).follow_links(false).into_iter().filter_entry(|entry| {
        !(skip_hidden && entry.path() != root_path && is_hidden_or_excluded(entry.path(), root_path))
    }) {
        let entry = entry.context("遍历文件夹失败")?;
        if !entry.file_type().is_file() { continue; }
        if files.len() >= MAX_FILES { anyhow::bail!("文件数量超过限制（{MAX_FILES} 个）"); }

        let absolute = entry.path();
        let relative = absolute.strip_prefix(root_path).context("无法计算相对路径")?.to_string_lossy().replace('\\', "/");
        let metadata = entry.metadata()?;
        let hash = sha256(absolute)?;
        let status = match state.entries.get(&relative) {
            None => "new",
            Some(previous) if previous.deleted || previous.hash != hash => "modified",
            Some(_) => "unchanged",
        };
        let mime_type = mime_guess::from_path(absolute).first_or_octet_stream().essence_str().to_string();
        let modified_at = metadata.modified().ok().and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok()).map(|duration| duration.as_secs() as i64).unwrap_or_default();
        total_bytes = total_bytes.saturating_add(metadata.len());
        current_paths.insert(relative.clone());
        files.push(ScannedFile {
            relative_path: relative,
            absolute_path: absolute.to_string_lossy().to_string(),
            size: metadata.len(),
            modified_at,
            mime_type,
            status: status.to_string(),
            hash,
        });
    }

    files.sort_by(|a, b| a.relative_path.to_lowercase().cmp(&b.relative_path.to_lowercase()));
    let deleted_count = state.entries.iter().filter(|(path, entry)| !entry.deleted && !current_paths.contains(*path)).count();
    let changed_count = files.iter().filter(|file| file.status != "unchanged").count();
    let unchanged_count = files.len().saturating_sub(changed_count);
    Ok(ScanResult { root: root_path.to_string_lossy().to_string(), files, total_bytes, changed_count, unchanged_count, deleted_count })
}
