use crate::models::{FfmpegStatus, UploadProgress};
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tauri::{AppHandle, Emitter, Manager};
use tokio::process::Command;

pub const MAX_VIDEO_SEGMENT_BYTES: u64 = 4_800_000_000;
const TARGET_VIDEO_SEGMENT_BYTES: u64 = 4_600_000_000;
const MAX_SPLIT_ATTEMPTS: usize = 5;

struct TempDirectory {
    path: PathBuf,
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct SplitVideo {
    pub parts: Vec<PathBuf>,
    _temp_directory: TempDirectory,
}

pub fn detect_ffmpeg() -> FfmpegStatus {
    for (ffmpeg, ffprobe) in command_candidates() {
        let Some(version) = command_version(&ffmpeg) else {
            continue;
        };
        if command_version(&ffprobe).is_none() {
            continue;
        }

        return FfmpegStatus {
            available: true,
            ffmpeg_path: Some(ffmpeg.to_string_lossy().to_string()),
            ffprobe_path: Some(ffprobe.to_string_lossy().to_string()),
            version: Some(version.clone()),
            message: format!("已检测到 {version}"),
        };
    }

    FfmpegStatus {
        available: false,
        ffmpeg_path: None,
        ffprobe_path: None,
        version: None,
        message: "未检测到 ffmpeg/ffprobe。请安装后加入 PATH，或设置 FFMPEG_PATH。".to_string(),
    }
}

pub async fn split_video(
    app: &AppHandle,
    input: &Path,
    input_size: u64,
) -> Result<SplitVideo> {
    let status = detect_ffmpeg();
    if !status.available {
        anyhow::bail!(status.message);
    }

    let ffmpeg = PathBuf::from(status.ffmpeg_path.context("缺少 ffmpeg 路径")?);
    let ffprobe = PathBuf::from(status.ffprobe_path.context("缺少 ffprobe 路径")?);
    emit_progress(app, 0, 1, "正在分析超大视频", &input.to_string_lossy());

    let duration = probe_duration(&ffprobe, input).await?;
    if !duration.is_finite() || duration <= 0.0 {
        anyhow::bail!("ffprobe 未返回有效的视频时长");
    }

    let temp_dir = app
        .path()
        .app_cache_dir()
        .context("无法获取应用缓存目录")?
        .join("video-segments")
        .join(format!("{}-{}", Utc::now().timestamp_millis(), std::process::id()));
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .context("无法创建视频切分临时目录")?;
    let temp_directory = TempDirectory {
        path: temp_dir.clone(),
    };

    let output_pattern = temp_dir.join(format!("{}.part-%03d.mkv", safe_stem(input)));
    let mut segment_seconds = estimate_segment_seconds(duration, input_size);

    for attempt in 1..=MAX_SPLIT_ATTEMPTS {
        clear_directory(&temp_dir).await?;
        emit_progress(
            app,
            attempt,
            MAX_SPLIT_ATTEMPTS,
            "正在使用 ffmpeg 切分视频",
            &format!("第 {attempt} 次校准，目标每段约 4.8 GB"),
        );

        let output = Command::new(&ffmpeg)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-i")
            .arg(input)
            .arg("-map")
            .arg("0:v?")
            .arg("-map")
            .arg("0:a?")
            .arg("-map")
            .arg("0:s?")
            .arg("-c")
            .arg("copy")
            .arg("-f")
            .arg("segment")
            .arg("-segment_time")
            .arg(format!("{segment_seconds:.3}"))
            .arg("-reset_timestamps")
            .arg("1")
            .arg("-avoid_negative_ts")
            .arg("make_zero")
            .arg("-max_muxing_queue_size")
            .arg("4096")
            .arg(&output_pattern)
            .output()
            .await
            .context("无法启动 ffmpeg 视频切分进程")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            anyhow::bail!(
                "ffmpeg 视频切分失败{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!("：{stderr}")
                }
            );
        }

        let parts = list_parts(&temp_dir).await?;
        if parts.len() < 2 {
            anyhow::bail!("ffmpeg 未生成预期的视频分段");
        }

        let mut largest = 0_u64;
        for part in &parts {
            largest = largest.max(tokio::fs::metadata(part).await?.len());
        }

        if largest <= MAX_VIDEO_SEGMENT_BYTES {
            emit_progress(
                app,
                parts.len(),
                parts.len(),
                "视频切分完成",
                &format!("已生成 {} 个可播放分段", parts.len()),
            );
            return Ok(SplitVideo {
                parts,
                _temp_directory: temp_directory,
            });
        }

        let adjustment = (MAX_VIDEO_SEGMENT_BYTES as f64 / largest as f64) * 0.92;
        segment_seconds = (segment_seconds * adjustment).max(1.0);
    }

    anyhow::bail!("视频切分后仍有分段超过 4.8 GB；源视频关键帧间隔可能过长")
}

async fn probe_duration(ffprobe: &Path, input: &Path) -> Result<f64> {
    let output = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(input)
        .output()
        .await
        .context("无法启动 ffprobe")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(
            "ffprobe 分析视频失败{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!("：{stderr}")
            }
        );
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .context("无法解析 ffprobe 返回的视频时长")
}

fn estimate_segment_seconds(duration: f64, input_size: u64) -> f64 {
    if input_size == 0 {
        return 1.0;
    }
    (duration * TARGET_VIDEO_SEGMENT_BYTES as f64 / input_size as f64)
        .max(1.0)
        .min(duration)
}

fn safe_stem(path: &Path) -> String {
    let source = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("video");
    let value: String = source
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if value.is_empty() {
        "video".to_string()
    } else {
        value
    }
}

async fn list_parts(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = tokio::fs::read_dir(directory)
        .await
        .context("无法读取视频切分结果")?;
    let mut parts = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("mkv") {
            parts.push(path);
        }
    }
    parts.sort();
    Ok(parts)
}

async fn clear_directory(directory: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            tokio::fs::remove_file(path).await?;
        } else if path.is_dir() {
            tokio::fs::remove_dir_all(path).await?;
        }
    }
    Ok(())
}

fn command_candidates() -> Vec<(PathBuf, PathBuf)> {
    let mut candidates = Vec::new();

    if let Ok(value) = std::env::var("FFMPEG_PATH") {
        let path = PathBuf::from(value);
        let ffmpeg = if path.is_dir() {
            path.join(executable_name("ffmpeg"))
        } else {
            path
        };
        candidates.push((ffmpeg.clone(), sibling_ffprobe(&ffmpeg)));
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            let ffmpeg = directory.join(executable_name("ffmpeg"));
            candidates.push((ffmpeg.clone(), sibling_ffprobe(&ffmpeg)));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let ffmpeg = PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffmpeg.exe");
            candidates.push((ffmpeg.clone(), sibling_ffprobe(&ffmpeg)));
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(root) = std::env::var(variable) {
                let ffmpeg = PathBuf::from(root)
                    .join("ffmpeg")
                    .join("bin")
                    .join("ffmpeg.exe");
                candidates.push((ffmpeg.clone(), sibling_ffprobe(&ffmpeg)));
            }
        }
        let ffmpeg = PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe");
        candidates.push((ffmpeg.clone(), sibling_ffprobe(&ffmpeg)));
    }

    candidates.push((
        PathBuf::from(executable_name("ffmpeg")),
        PathBuf::from(executable_name("ffprobe")),
    ));

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|(ffmpeg, ffprobe)| {
            seen.insert(format!(
                "{}|{}",
                ffmpeg.to_string_lossy(),
                ffprobe.to_string_lossy()
            ))
        })
        .collect()
}

fn sibling_ffprobe(ffmpeg: &Path) -> PathBuf {
    ffmpeg
        .parent()
        .map(|parent| parent.join(executable_name("ffprobe")))
        .unwrap_or_else(|| PathBuf::from(executable_name("ffprobe")))
}

fn executable_name(base: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn command_version(command: &Path) -> Option<String> {
    let output = StdCommand::new(command).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn emit_progress(app: &AppHandle, current: usize, total: usize, stage: &str, detail: &str) {
    let _ = app.emit(
        "upload-progress",
        UploadProgress {
            current,
            total,
            stage: stage.to_string(),
            detail: detail.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{estimate_segment_seconds, safe_stem, MAX_VIDEO_SEGMENT_BYTES};
    use std::path::Path;

    #[test]
    fn estimates_smaller_than_five_gib_segments() {
        let seconds = estimate_segment_seconds(10_000.0, 10 * 1024 * 1024 * 1024);
        assert!(seconds > 4_000.0);
        assert!(MAX_VIDEO_SEGMENT_BYTES < 5 * 1024 * 1024 * 1024);
    }

    #[test]
    fn sanitizes_segment_file_names() {
        assert_eq!(safe_stem(Path::new("a:b?.mp4")), "a_b_");
    }
}
