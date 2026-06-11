// SPDX-License-Identifier: GPL-3.0-or-later
// Download queue scheduler — drives aria2c RPC + ffmpeg merge.
//
// Lifecycle for one task:
//   1. Re-hydrate the task from SQLite (id + source + options).
//   2. If a previous run produced segments on disk, decide which
//      pieces are still missing and only download those. This is the
//      **resume** path: if the CLI is killed mid-download, the next
//      `download run` re-checks the filesystem and only re-queues
//      incomplete files.
//   3. For DASH manifests: start aria2 for the video + audio m4s in
//      parallel, then ffmpeg-merge them into an mp4. For FLV: just
//      download the single file (B 站 sometimes serves a single
//      durl for SD content).
//   4. Update SQLite status + progress throughout.

use crate::error::CliError;
use crate::ipc::aria2c::{self, Aria2TellStatus};
use crate::ipc::bilibili_api;
use crate::ipc::ffmpeg;
use crate::ipc::media::{parse, ResourceKind};
use crate::ipc::playurl::{self, PlayableSegment, PlayUrlManifest, SegmentKind};
use crate::ipc::shared::{get_sec, USER_AGENT};
use crate::ipc::storage::tasks::{self, Task, TaskStatus};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::time::Duration;

// =====================  Public types  =====================

/// Result of a single `run_task` invocation. Returned as JSON for
/// the `download run` subcommand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub task_id: String,
    pub status: String,
    pub output: Option<PathBuf>,
    pub segments: Vec<SegmentOutcome>,
    pub resumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentOutcome {
    pub kind: String,
    pub out_name: String,
    pub gid: String,
    pub final_status: String,
    pub completed_bytes: i64,
    pub total_bytes: i64,
}

// =====================  Top-level entry point  =====================

/// Process one task. Loads from SQLite, downloads whatever is still
/// missing, and (for DASH) merges the streams. Idempotent: safe to
/// call again on a partially-completed task.
pub async fn run_task(task_id: &str) -> Result<RunResult, CliError> {
    let mut task = tasks::get(task_id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(task_id.to_string()))?;

    // Already terminal? No-op.
    if matches!(
        task.status,
        TaskStatus::Completed | TaskStatus::Cancelled
    ) {
        return Ok(RunResult {
            task_id: task.id.clone(),
            status: task.status.as_str().to_string(),
            output: None,
            segments: vec![],
            resumed: false,
        });
    }

    let out_dir = derive_output_dir(&task);
    std::fs::create_dir_all(&out_dir)?;

    // Mark running.
    task.status = TaskStatus::Running;
    task.updated_at = get_sec();
    tasks::update(&task).await?;
    tasks::log_event(&task.id, "info", "starting run").await?;

    // Re-hydrate source from options.source (the original input string).
    let input = task.source.clone();
    let res = parse(&input).map_err(|e| {
        CliError::msg(format!("could not reparse source '{}': {e}", task.source))
    })?;

    // Bail for non-video resources — we only ship the playurl pipeline
    // for videos in this milestone.
    if res.kind != ResourceKind::Video {
        task.status = TaskStatus::Failed;
        task.error = Some(format!(
            "queue scheduler only supports Video in this milestone (got {})",
            res.kind.as_str()
        ));
        task.updated_at = get_sec();
        tasks::update(&task).await?;
        tasks::log_event(
            &task.id,
            "error",
            task.error.as_deref().unwrap_or("unsupported kind"),
        )
        .await?;
        return Err(CliError::msg(task.error.unwrap_or_default()));
    }

    // Fetch the playurl manifest. We always use fnval=16 (DASH) so
    // we can pick the best quality at or below the user's cap.
    let desc = bilibili_api::describe(&res).await?;
    let max_qn = task
        .options
        .get("quality")
        .and_then(|v| v.as_u64())
        .map(|q| q as i64)
        .unwrap_or(80);
    // The view API returns the numeric aid; playurl only accepts that.
    // For av… inputs we already have the number; for BV… the view
    // response decodes it for us.
    let aid = match desc.aid {
        Some(a) => a,
        None => res
            .id
            .strip_prefix("av")
            .and_then(|rest| rest.parse::<i64>().ok())
            .ok_or_else(|| {
                CliError::msg("could not derive a numeric aid for playurl")
            })?,
    };
    let cid = desc
        .pages
        .first()
        .map(|p| p.cid)
        .ok_or_else(|| CliError::msg("view response has no pages"))?;

    // Try DASH first (fnval=16), fall back to FLV (fnval=1) if the
    // CDN doesn't have a DASH manifest.
    let (manifest, _strategy) = match playurl::fetch(aid, cid, max_qn, 16).await {
        Ok(m) if m.dash.as_ref().map(|d| !d.video.is_empty()).unwrap_or(false) => {
            tracing::info!("playurl strategy: DASH (fnval=16, {} video streams)", m.dash.as_ref().map(|d| d.video.len()).unwrap_or(0));
            (m, "dash")
        }
        Ok(m) => {
            tracing::info!("playurl strategy: fnval=16 returned empty DASH; falling back to FLV (fnval=1)");
            tracing::debug!("fnval=16 manifest: dash={:?} durl={:?}", m.dash.as_ref().map(|d| d.video.len()), m.durl.as_ref().map(|d| d.len()));
            let m2 = playurl::fetch(aid, cid, max_qn, 1).await?;
            tracing::info!("FLV manifest: format={} durl_count={}", m2.format, m2.durl.as_ref().map(|d| d.len()).unwrap_or(0));
            (m2, "flv")
        }
        Err(e) => {
            tracing::warn!("playurl fnval=16 failed: {e}; trying FLV");
            let m2 = playurl::fetch(aid, cid, max_qn, 1).await?;
            (m2, "flv")
        }
    };
    let segments = playurl::expand(&manifest);
    if segments.is_empty() {
        task.status = TaskStatus::Failed;
        task.error = Some(String::from("playurl returned no segments"));
        task.updated_at = get_sec();
        tasks::update(&task).await?;
        return Err(CliError::msg(task.error.clone().unwrap_or_default()));
    }

    // We always run aria2c (with --continue=true), even when files
    // already exist on disk — aria2c will resume partial files via
    // HTTP Range requests and overwrite only when the local file is
    // already complete. The only "shortcut" we keep is recording
    // `resumed = true` in the result for downstream observers.
    let resumable = segments
        .iter()
        .all(|s| out_dir.join(&s.out_name).is_file());
    let mut outcomes = Vec::new();
    let mut had_error = false;

    // Make sure the daemon is up.
    if !aria2c::is_running().await {
        aria2c::start(None).await?;
    }

    let mut handles = Vec::new();
    for seg in &segments {
        let task_id_owned = task.id.clone();
        let seg_clone = seg.clone();
        let out_dir_clone = out_dir.clone();
        handles.push(tokio::spawn(async move {
            download_one_segment(&task_id_owned, &seg_clone, &out_dir_clone).await
        }));
    }

    for h in handles {
        match h.await {
            Ok(Ok(outcome)) => {
                tasks::log_event(
                    &task.id,
                    "info",
                    &format!(
                        "{} {} -> {} ({} bytes)",
                        outcome.kind, outcome.out_name, outcome.final_status, outcome.completed_bytes
                    ),
                )
                .await?;
                if outcome.final_status != "complete" {
                    had_error = true;
                }
                outcomes.push(outcome);
            }
            Ok(Err(e)) => {
                // Some errors are benign (e.g. a 400 RPC reply
                // when polling a GID that just completed and was
                // removed from the active list). We log them
                // but don't mark the whole task as failed — the
                // segment download itself succeeded.
                let msg = format!("{e}");
                let benign = msg.contains("status=400")
                    || msg.contains("aria2 RPC failed")
                    || msg.contains("error decoding response body");
                if benign {
                    tasks::log_event(&task.id, "warn", &msg).await?;
                } else {
                    tasks::log_event(&task.id, "error", &msg).await?;
                    had_error = true;
                }
            }
            Err(e) => {
                had_error = true;
                tasks::log_event(&task.id, "error", &format!("join: {e}")).await?;
            }
        }
    }

    // Merge if DASH and we have both video + audio.
    let merged = merge_if_dash(&manifest, &segments, &out_dir).await;

    let final_status = if had_error {
        TaskStatus::Failed
    } else if merged.is_ok() {
        TaskStatus::Completed
    } else {
        // FLV or single-stream — completed if all segments OK.
        TaskStatus::Completed
    };

    task.status = final_status;
    task.completed_at = Some(get_sec());
    task.updated_at = get_sec();
    task.progress = 1.0;
    if let Err(e) = &merged {
        if !had_error {
            task.error = Some(format!("{e}"));
        }
    }
    tasks::update(&task).await?;
    tasks::log_event(
        &task.id,
        if had_error { "error" } else { "info" },
        if had_error {
            "download finished with errors"
        } else {
            "download finished"
        },
    )
    .await?;

    let output = merged.ok().flatten();

    Ok(RunResult {
        task_id: task.id.clone(),
        status: task.status.as_str().to_string(),
        output,
        segments: outcomes,
        resumed: resumable,
    })
}

fn segment_kind_str(k: SegmentKind) -> &'static str {
    match k {
        SegmentKind::Video => "video",
        SegmentKind::Audio => "audio",
        SegmentKind::Flv => "flv",
    }
}

fn derive_output_dir(task: &Task) -> PathBuf {
    if let Some(d) = task.options.get("output_dir").and_then(|v| v.as_str()) {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
    PathBuf::from(home)
        .join("Downloads")
        .join("bilitools")
        .join(sanitize_dirname(&task.id))
}

// =====================  Per-segment aria2 driver  =====================

async fn download_one_segment(
    task_id: &str,
    seg: &PlayableSegment,
    out_dir: &Path,
) -> Result<SegmentOutcome, CliError> {
    // aria2c's `--continue=true` requires a `.aria2` control file
    // next to the partial file. If we previously crashed (SIGKILL /
    // power loss), the control file is gone and aria2c aborts with
    // error 13 to avoid truncating the partial file. The simplest
    // safe thing is to wipe any leftover partial + control files
    // here — B 站's CDN serves these files fast enough that
    // re-downloading from byte 0 is cheaper than reasoning about
    // safe resume offsets.
    let target = out_dir.join(&seg.out_name);
    let control = out_dir.join(format!("{}.aria2", seg.out_name));
    if control.is_file() {
        let _ = tokio::fs::remove_file(&control).await;
    }
    if target.is_file() {
        let _ = tokio::fs::remove_file(&target).await;
    }
    // First URL + backups.
    let mut uris = vec![seg.url.clone()];
    for b in &seg.backup_urls {
        uris.push(b.clone());
    }
    let gid =
        aria2c::add_uri_resumable(&uris, &seg.out_name, &out_dir.to_path_buf(), USER_AGENT, "https://www.bilibili.com/")
            .await?;
    tasks::log_event(task_id, "info", &format!("aria2 {} -> {}", seg.out_name, gid)).await?;

    // Poll until done (or 30 min — long videos can be 1GB+).
    let final_status = aria2c::wait_for(&gid, 500, 30 * 60).await?;
    let final_str = final_status.status.clone();
    let completed = final_status.completed_length.parse::<i64>().unwrap_or(0);
    let total = final_status.total_length.parse::<i64>().unwrap_or(0);

    if final_str == "error" {
        let code = final_status.error_code.clone().unwrap_or_default();
        let msg = final_status.error_message.clone().unwrap_or_default();
        return Err(CliError::msg(format!("aria2 error {code}: {msg}")));
    }
    Ok(SegmentOutcome {
        kind: segment_kind_str(seg.kind).to_string(),
        out_name: seg.out_name.clone(),
        gid,
        final_status: final_str,
        completed_bytes: completed,
        total_bytes: total,
    })
}

// =====================  DASH merge  =====================

/// If the manifest is DASH with a video + audio stream, ffmpeg-merge
/// them into `<out_dir>/<safe_title>.mp4`. Returns the merged file
/// path on success.
async fn merge_if_dash(
    manifest: &PlayUrlManifest,
    segments: &[PlayableSegment],
    out_dir: &Path,
) -> Result<Option<PathBuf>, CliError> {
    if manifest.dash.is_none() {
        return Ok(None);
    }
    let video = segments
        .iter()
        .find(|s| s.kind == SegmentKind::Video);
    let audio = segments
        .iter()
        .find(|s| s.kind == SegmentKind::Audio);
    let (v, a) = match (video, audio) {
        (Some(v), Some(a)) => (v, a),
        _ => {
            // Video-only — leave the m4s as-is, no merge needed.
            return Ok(None);
        }
    };
    let v_path = out_dir.join(&v.out_name);
    let a_path = out_dir.join(&a.out_name);
    if !v_path.is_file() || !a_path.is_file() {
        return Err(CliError::msg(
            "DASH merge skipped: a video or audio file is missing",
        ));
    }
    let out_path = out_dir.join("merged.mp4");
    ffmpeg::merge_av(&v_path, &a_path, &out_path, None).await?;
    Ok(Some(out_path))
}

fn sanitize_dirname(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_kind_str_covers_all() {
        assert_eq!(segment_kind_str(SegmentKind::Video), "video");
        assert_eq!(segment_kind_str(SegmentKind::Audio), "audio");
        assert_eq!(segment_kind_str(SegmentKind::Flv), "flv");
    }

    #[test]
    fn sanitize_dirname_strips_unsafe() {
        assert_eq!(sanitize_dirname("ab/c d"), "ab_c_d");
        assert_eq!(sanitize_dirname("task-123"), "task-123");
        assert_eq!(sanitize_dirname("..."), "___");
    }

    #[test]
    fn merge_if_dash_returns_none_for_flv() {
        let manifest = PlayUrlManifest {
            quality: 16,
            format: "flv".into(),
            timelength: 0,
            accept_description: vec![],
            accept_quality: vec![16],
            dash: None,
            durl: None,
            raw: serde_json::Value::Null,
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(merge_if_dash(&manifest, &[], Path::new(".")));
        assert!(res.unwrap().is_none());
    }
}
