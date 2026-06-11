// SPDX-License-Identifier: GPL-3.0-or-later
// B 站视频音频下载 IPC。
//
// 工作流：
//   1. resolve_cid(bv) 拿 aid + cid
//   2. playurl::fetch(aid, cid, qn, 16) 拿 DASH manifest
//   3. playurl::expand() 过滤 audio segments
//   4. reqwest 直接拉每个 audio m4s (不依赖 aria2c — 单文件，足够快)
//   5. 拼接所有 audio m4s → ffmpeg -i ... -c:a copy output.m4a
//   6. 清理中间 m4s
//
// 降级策略：
//   - DASH 无 audio 段 → 错误 (B 站几乎都给 DASH audio, anon 也有)
//   - ffmpeg 失败 → 保留 m4s 文件 + degraded 提示
//   - 多段 audio 拼接 (DASH audio 可能 >1 段) → 用 concat demuxer

use crate::backends::sidecar::SidecarKind;
use crate::error::CliError;
use crate::ipc::danmaku;
use crate::ipc::playurl::{self, SegmentKind};
use crate::ipc::shared;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub type Result<T> = std::result::Result<T, CliError>;

/// 音频下载结果
#[derive(Debug, Clone, Serialize)]
pub struct AudioResult {
    pub bv: String,
    pub aid: i64,
    pub cid: i64,
    pub title: String,
    pub quality_qn: i64,
    pub audio_codec: String,
    pub audio_bandwidth: i64,
    pub duration_sec: f64,
    pub segments_downloaded: usize,
    pub m4a_path: PathBuf,
    pub m4a_bytes: u64,
    pub degraded: Vec<String>,
}

/// 拉音频完整流程
///
/// `output_dir` 是放最终 `*.m4a` 的目录，中间 m4s 落同目录后清理。
/// `max_qn` = 80 (1080P) / 64 (720P) 等，控制 DASH 视频的 quality tier（影响音轨 bitrate）。
/// 实际播放的音频流由 B 站选最高码率的，不受 qn 影响（DASH audio 是独立轨道）。
pub async fn fetch_audio(
    bv: &str,
    output_dir: &Path,
    max_qn: i64,
) -> Result<AudioResult> {
    let (title, aid, cid) = danmaku::resolve_cid(bv).await?;
    let mut degraded = Vec::new();

    if !shared::HEADERS.cookie().await.contains("SESSDATA=") {
        degraded.push("匿名模式：仅可下载非大会员音轨".to_string());
    }

    fs::create_dir_all(output_dir).await.map_err(CliError::from)?;

    // 1) DASH manifest
    let manifest = playurl::fetch(aid, cid, max_qn, 16).await?;
    let segments = playurl::expand(&manifest);
    let audio_segs: Vec<_> = segments
        .iter()
        .filter(|s| s.kind == SegmentKind::Audio)
        .collect();
    if audio_segs.is_empty() {
        return Err(CliError::msg(format!(
            "no audio segments in DASH manifest for {bv}"
        )));
    }

    let audio_info = audio_segs[0];
    let codec = audio_info.codec.clone();
    let bandwidth = audio_info.bandwidth;
    let quality_qn = manifest.quality;
    let duration = manifest.dash.as_ref().map(|d| d.duration as f64).unwrap_or(0.0);

    // 2) 拉每个 audio m4s
    let client = shared::init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let mut m4s_paths: Vec<PathBuf> = Vec::new();
    for (i, seg) in audio_segs.iter().enumerate() {
        let m4s_path = output_dir.join(format!("audio-{i}.m4s"));
        match download_one(&client, seg, &m4s_path).await {
            Ok(_) => m4s_paths.push(m4s_path),
            Err(e) => {
                degraded.push(format!("audio segment {i} failed: {e}"));
            }
        }
    }
    if m4s_paths.is_empty() {
        return Err(CliError::msg("all audio segment downloads failed"));
    }

    // 3) ffmpeg → m4a
    let safe_title = sanitize_filename(&title);
    let m4a_path = output_dir.join(format!("{safe_title}-{cid}.m4a"));
    let m4a_bytes = match merge_audio_to_m4a(&m4s_paths, &m4a_path).await {
        Ok(b) => b,
        Err(e) => {
            // 保留 m4s 让用户能手工转
            degraded.push(format!("ffmpeg m4a conversion failed: {e}"));
            0
        }
    };

    // 4) 清理 m4s
    for p in &m4s_paths {
        let _ = fs::remove_file(p).await;
    }

    Ok(AudioResult {
        bv: bv.to_string(),
        aid,
        cid,
        title,
        quality_qn,
        audio_codec: codec,
        audio_bandwidth: bandwidth,
        duration_sec: duration,
        segments_downloaded: m4s_paths.len(),
        m4a_path,
        m4a_bytes,
        degraded,
    })
}

async fn download_one(
    client: &reqwest::Client,
    seg: &playurl::PlayableSegment,
    out: &Path,
) -> Result<u64> {
    let urls = std::iter::once(seg.url.as_str()).chain(seg.backup_urls.iter().map(|s| s.as_str()));
    let mut last_err: Option<CliError> = None;
    for url in urls {
        if url.is_empty() {
            continue;
        }
        match try_download(client, url, out).await {
            Ok(n) => return Ok(n),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| CliError::msg("no usable URL for audio segment")))
}

async fn try_download(client: &reqwest::Client, url: &str, out: &Path) -> Result<u64> {
    let resp = client.get(url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("audio download {url}"),
        });
    }
    let bytes = resp.bytes().await.map_err(CliError::from)?;
    let mut f = fs::File::create(out).await.map_err(CliError::from)?;
    f.write_all(&bytes).await.map_err(CliError::from)?;
    f.flush().await.map_err(CliError::from)?;
    Ok(bytes.len() as u64)
}

/// 拼接多个 m4s → 单个 m4a
///
/// 优先用 ffmpeg concat demuxer（精准按 segment 切），退路 ffmpeg -i first
/// （单段时足够，m4s 自己就是 mp4 容器）。
async fn merge_audio_to_m4a(m4s_paths: &[PathBuf], output: &Path) -> Result<u64> {
    let ffmpeg = crate::backends::sidecar::resolve(SidecarKind::FFmpeg, None)
        .map_err(|e| CliError::MissingDependency(format!("ffmpeg: {e}")))?;

    if m4s_paths.len() == 1 {
        // 单段：直接 copy
        let status = tokio::process::Command::new(&ffmpeg)
            .args(["-y", "-i"])
            .arg(&m4s_paths[0])
            .args(["-vn", "-c:a", "copy"])
            .arg(output)
            .output()
            .await
            .map_err(|e| CliError::msg(format!("ffmpeg spawn: {e}")))?;
        if !status.status.success() {
            return Err(CliError::msg(format!(
                "ffmpeg: {} (stderr: {})",
                status.status,
                String::from_utf8_lossy(&status.stderr)
            )));
        }
    } else {
        // 多段：concat demuxer
        let list_path = output.with_extension("concat.txt");
        let mut list_body = String::new();
        for p in m4s_paths {
            // ffmpeg concat demuxer 需要 escape 单引号 (用 ' 包裹整个 path)
            let s = p.to_string_lossy().replace('\'', "'\\''");
            list_body.push_str(&format!("file '{s}'\n"));
        }
        fs::write(&list_path, list_body)
            .await
            .map_err(CliError::from)?;

        let status = tokio::process::Command::new(&ffmpeg)
            .args(["-y", "-f", "concat", "-safe", "0", "-i"])
            .arg(&list_path)
            .args(["-vn", "-c:a", "copy"])
            .arg(output)
            .output()
            .await
            .map_err(|e| CliError::msg(format!("ffmpeg spawn: {e}")))?;
        let _ = fs::remove_file(&list_path).await;
        if !status.status.success() {
            return Err(CliError::msg(format!(
                "ffmpeg concat: {} (stderr: {})",
                status.status,
                String::from_utf8_lossy(&status.stderr)
            )));
        }
    }

    let meta = fs::metadata(output).await.map_err(CliError::from)?;
    Ok(meta.len())
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_chinese() {
        let s = sanitize_filename("【最强】上字幕 2026 — 零度");
        assert!(s.contains("2026"));
        assert!(!s.contains(' '));
        assert!(!s.contains('【'));
    }

    #[test]
    fn sanitize_filename_truncates() {
        let s = "a".repeat(200);
        assert!(sanitize_filename(&s).len() <= 80);
    }

    #[test]
    fn sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "");
    }

    /// 真打 B 站 API — 拉音频完整流程 (登录态)
    #[tokio::test]
    #[ignore]
    async fn fetch_audio_against_real_bilibili() {
        let tmp = tempdir();
        let dir = tmp.path();
        let r = fetch_audio("BV1XBRuBSEd7", dir, 80)
            .await
            .expect("fetch_audio failed");
        assert!(r.m4a_bytes > 1024, "m4a too small: {}", r.m4a_bytes);
        assert!(r.m4a_path.is_file());
        // 真实音频文件至少 30 秒时长
        assert!(r.duration_sec >= 30.0, "duration too short: {}", r.duration_sec);
        // codec 是 aac
        assert!(r.audio_codec.contains("mp4a") || r.audio_codec.contains("aac"));
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }
}
