// SPDX-License-Identifier: GPL-3.0-or-later
// `analyze` subcommand — unified video content extraction.
//
// Extracts all text content from a B 站 video in one shot:
//   1. Metadata (title, description, duration, pages)
//   2. Audio → ASR transcript (via sensevoice)
//   3. Danmaku XML → plain text
//   4. Subtitles → plain text
//   5. Reviews/comments → plain text
//   6. (optional) OCR from video frames
//
// Output: analysis.json + analysis.txt in a per-BV directory.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::danmaku::{self, DanmakuFormat, DanmakuSource};
use crate::ipc::review::{self, ReviewSort};
use crate::ipc::subtitle;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;

pub type Result<T> = std::result::Result<T, CliError>;

// ── Output structures ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzeResult {
    pub bv: String,
    pub aid: i64,
    pub cid: i64,
    pub title: String,
    pub description: Option<String>,
    pub duration_sec: Option<f64>,
    pub pages: Vec<PageInfo>,
    pub analyzed_at: String,
    pub output_dir: PathBuf,
    /// Audio + transcript
    pub audio: Option<AudioSection>,
    /// Danmaku text content
    pub danmaku: Option<DanmakuSection>,
    /// Subtitles by language
    pub subtitles: Vec<SubtitleSection>,
    /// Review / comment text
    pub reviews: Option<ReviewSection>,
    /// OCR text (requires `--with-ocr`)
    pub ocr: Option<serde_json::Value>,
    /// Non-fatal errors per component
    pub degraded: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub cid: i64,
    pub title: String,
    pub duration: Option<f64>,
    pub part: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioSection {
    pub m4a_path: PathBuf,
    pub m4a_bytes: u64,
    pub audio_codec: String,
    pub audio_bandwidth: i64,
    /// Full transcript text
    pub transcript: String,
    /// Transcript split into non-empty segments
    pub segments: Vec<String>,
    pub char_count: usize,
    pub segment_count: usize,
    pub language: String,
    pub device: String,
    pub rtf: Option<f32>,
    pub audio_duration_sec: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DanmakuSection {
    pub live_count: usize,
    pub text_lines: Vec<String>,
    pub xml_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleSection {
    pub lan: String,
    pub lan_doc: String,
    pub text_lines: Vec<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewSection {
    pub total: i64,
    pub loaded: usize,
    pub text_lines: Vec<String>,
}

// ── Main entry point ───────────────────────────────────────────────

pub async fn run(cmd: &Command, out: &Output) -> Result<()> {
    let Command::Analyze {
        input,
        output_dir,
        no_audio,
        no_danmaku,
        no_subtitle,
        no_review,
        with_ocr,
        transcribe_language,
        transcribe_device,
        transcribe_keep_tags,
        ocr_interval,
        ocr_max_frames,
        ocr_min_conf,
        ocr_dedup_window,
        video_path,
    } = cmd
    else {
        return Err(CliError::Other("internal: not an Analyze command".into()));
    };

    // ── 1. Resolve video ───────────────────────────────────────────
    let (title, aid, cid) = danmaku::resolve_cid(input).await?;
    let bv = danmaku::extract_bvid(input)
        .unwrap_or_else(|| input.to_string());
    let safe_slug = slugify(&title);

    let base_dir = output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("./analyze_output"));
    let work_dir = base_dir.join(format!("{bv}-{safe_slug}"));
    fs::create_dir_all(&work_dir).await.map_err(CliError::from)?;

    out.status(&format!(
        "[analyze] {} — {title}",
        bv
    ));
    out.status(&format!("[analyze] output: {}", work_dir.display()));

    // Fetch description via bilibili_api
    let (description, duration_sec, pages) = fetch_meta(&bv).await;

    let mut degraded: Vec<String> = Vec::new();

    // ── 2. Run components in parallel ──────────────────────────────
    let audio_fut = if *no_audio {
        None
    } else {
        Some(run_audio(
            input,
            &work_dir,
            out,
            transcribe_language,
            transcribe_device,
            *transcribe_keep_tags,
        ))
    };
    let danmaku_fut = if *no_danmaku {
        None
    } else {
        Some(run_danmaku(input, &work_dir))
    };
    let subtitle_fut = if *no_subtitle {
        None
    } else {
        Some(run_subtitle(input, &work_dir))
    };
    let review_fut = if *no_review {
        None
    } else {
        Some(run_review(input))
    };

    let (audio_res, danmaku_res, sub_res, rev_res) = tokio::join!(
        opt_fut(audio_fut),
        opt_fut(danmaku_fut),
        opt_fut(subtitle_fut),
        opt_fut(review_fut),
    );

    // Collect results
    let audio = match audio_res {
        Ok(Some(a)) => Some(a),
        Ok(None) => None,
        Err(e) => { degraded.push(format!("audio: {e}")); None }
    };
    let danmaku = match danmaku_res {
        Ok(Some(d)) => Some(d),
        Ok(None) => None,
        Err(e) => { degraded.push(format!("danmaku: {e}")); None }
    };
    let subtitles = match sub_res {
        Ok(s) => s,
        Err(e) => { degraded.push(format!("subtitle: {e}")); Some(Vec::new()) }
    };
    let reviews = match rev_res {
        Ok(Some(r)) => Some(r),
        Ok(None) => None,
        Err(e) => { degraded.push(format!("review: {e}")); None }
    };

    // ── 3. OCR (optional, gated) ───────────────────────────────────
    let ocr: Option<serde_json::Value> = if *with_ocr {
        match run_ocr(
            video_path.as_deref(),
            &work_dir,
            *ocr_interval,
            *ocr_max_frames,
            *ocr_min_conf,
            *ocr_dedup_window,
        ).await {
            Ok(v) => Some(v),
            Err(e) => {
                degraded.push(format!("ocr: {e}"));
                None
            }
        }
    } else {
        None
    };

    // ── 4. Assemble output ─────────────────────────────────────────
    let analyzed_at = chrono::Utc::now().to_rfc3339();
    let result = AnalyzeResult {
        bv: bv.clone(),
        aid,
        cid,
        title: title.clone(),
        description,
        duration_sec,
        pages,
        analyzed_at,
        output_dir: work_dir.clone(),
        audio,
        danmaku,
        subtitles: subtitles.unwrap_or_default(),
        reviews,
        ocr,
        degraded,
    };

    // Write analysis.txt (single unified output)
    let txt = format_analysis_txt(&result);
    let out_path = work_dir.join("analysis.txt");
    fs::write(&out_path, txt.as_bytes())
        .await
        .map_err(CliError::from)?;

    // ── 5. Print summary ───────────────────────────────────────────
    if out.is_json() {
        out.ok(serde_json::to_value(&result)
            .unwrap_or(serde_json::json!({"error": "serialize failed"})))?;
    } else {
        out.status(&format!(
            "[done] analysis complete for {bv}\n\
             [done]   output: {}\n\
             [done]   audio: {}  danmaku: {}  subtitle: {}  review: {}  ocr: {}  degraded: {}",
            out_path.display(),
            result.audio.as_ref().map(|a| format!("{} chars", a.char_count)).unwrap_or_else(|| "skip".into()),
            result.danmaku.as_ref().map(|d| d.text_lines.len().to_string()).unwrap_or_else(|| "skip".into()),
            result.subtitles.len(),
            result.reviews.as_ref().map(|r| r.text_lines.len().to_string()).unwrap_or_else(|| "skip".into()),
            result.ocr.as_ref().map(|_| "yes").unwrap_or("skip"),
            result.degraded.len(),
        ));
    }

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────

/// Run an optional future, returning Ok(None) when passed None.
async fn opt_fut<T>(fut: Option<impl std::future::Future<Output = Result<T>>>) -> Result<Option<T>> {
    match fut {
        Some(f) => f.await.map(Some),
        None => Ok(None),
    }
}

/// Fetch metadata (description + pages) via the view API.
async fn fetch_meta(bv: &str) -> (Option<String>, Option<f64>, Vec<PageInfo>) {
    use crate::ipc::shared::init_client;
    let client = match init_client().await {
        Ok(c) => c,
        Err(_) => return (None, None, Vec::new()),
    };
    let url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bv}");
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return (None, None, Vec::new()),
    };
    let body: serde_json::Value = match resp.json().await {
        Ok(b) => b,
        Err(_) => return (None, None, Vec::new()),
    };
    let data = match body.get("data") {
        Some(d) => d,
        None => return (None, None, Vec::new()),
    };
    let desc = data.get("desc").and_then(|v| v.as_str()).map(|s| s.to_string());
    let duration = data.get("duration").and_then(|v| v.as_f64());
    let pages: Vec<PageInfo> = data
        .get("pages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| PageInfo {
                    cid: p.get("cid").and_then(|v| v.as_i64()).unwrap_or(0),
                    title: p.get("part").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    duration: p.get("duration").and_then(|v| v.as_i64()).map(|d| d as f64),
                    part: p.get("part").and_then(|v| v.as_str()).map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    (desc, duration, pages)
}

// ── Component runners ──────────────────────────────────────────────

async fn run_audio(
    input: &str,
    work_dir: &Path,
    out: &Output,
    language: &str,
    device: &str,
    keep_tags: bool,
) -> Result<AudioSection> {
    use crate::ipc::audio;
    use crate::ipc::transcribe::{self, TranscribeOpts};

    out.status("[analyze/audio] downloading m4a...");
    let audio_result = audio::fetch_audio(input, work_dir, 80).await?;

    if audio_result.m4a_bytes == 0 {
        return Err(CliError::msg("audio download produced 0 bytes"));
    }

    out.status(&format!(
        "[analyze/audio] m4a: {} ({:.1}s)",
        audio_result.m4a_path.display(),
        audio_result.duration_sec
    ));

    // ASR
    out.status(&format!("[analyze/audio] transcribing (lang={language} device={device})..."));
    let opts = TranscribeOpts {
        m4a_path: audio_result.m4a_path.clone(),
        output_txt: Some(work_dir.join("transcript.txt")),
        language: language.to_string(),
        device: device.to_string(),
        keep_tags,
        vad_max_sec: 15,
        sensevoice_cli: None,
        python_bin: None,
        timeout: Duration::from_secs(45 * 60),
    };
    let tr = transcribe::transcribe(&opts).await?;

    out.status(&format!(
        "[analyze/audio] transcript: {} chars, {} segments (rtf: {})",
        tr.char_count,
        tr.segment_count,
        tr.rtf.map(|v| format!("{v:.3}")).unwrap_or_else(|| "?".into())
    ));

    Ok(AudioSection {
        m4a_path: audio_result.m4a_path,
        m4a_bytes: audio_result.m4a_bytes,
        audio_codec: audio_result.audio_codec,
        audio_bandwidth: audio_result.audio_bandwidth,
        transcript: tr.text,
        segments: tr.segments,
        char_count: tr.char_count,
        segment_count: tr.segment_count,
        language: tr.language,
        device: tr.device,
        rtf: tr.rtf,
        audio_duration_sec: tr.audio_duration_sec,
    })
}

async fn run_danmaku(input: &str, work_dir: &Path) -> Result<DanmakuSection> {
    let result = danmaku::fetch_and_convert(
        input,
        work_dir,
        DanmakuSource::Live,
        DanmakuFormat::Xml,
    ).await?;

    // Extract text from danmaku XML
    let text_lines = if let Some(ref xml_path) = result.xml_path {
        let xml_bytes = fs::read(xml_path).await.unwrap_or_default();
        let xml_str = String::from_utf8_lossy(&xml_bytes);
        // Extract text from <d p="...">text</d> nodes
        extract_danmaku_text(&xml_str)
    } else {
        Vec::new()
    };

    Ok(DanmakuSection {
        live_count: result.live_count,
        text_lines,
        xml_path: result.xml_path,
    })
}

/// Extract the text content from all <d> nodes in B 站 danmaku XML.
fn extract_danmaku_text(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<d ") {
            if let Some(end) = xml[i..].find("</d>") {
                // Find the > that closes the opening <d ...> tag
                let tag_body = &xml[i..i + end];
                if let Some(text_start) = tag_body.find('>') {
                    // Text is between the > of <d ...> and the start of </d>
                    let text = &xml[i + text_start + 1..i + end];
                    let cleaned = text.trim();
                    if !cleaned.is_empty() {
                        out.push(cleaned.to_string());
                    }
                }
                i += end + 4;
                continue;
            }
        }
        i += 1;
    }
    out
}

async fn run_subtitle(input: &str, work_dir: &Path) -> Result<Vec<SubtitleSection>> {
    let result = subtitle::fetch_all(input, work_dir).await?;
    let mut out = Vec::new();

    for f in &result.fetched {
        let text_lines = extract_subtitle_text(&f.path).await;
        out.push(SubtitleSection {
            lan: f.entry.lan.clone(),
            lan_doc: f.entry.lan_doc.clone(),
            text_lines,
            path: Some(f.path.clone()),
        });
    }

    // If no subtitles, try listing and logging
    if out.is_empty() {
        let list = subtitle::list(input).await?;
        for e in &list.entries {
            out.push(SubtitleSection {
                lan: e.lan.clone(),
                lan_doc: e.lan_doc.clone(),
                text_lines: vec![format!("(URL: {})", e.subtitle_url)],
                path: None,
            });
        }
    }

    Ok(out)
}

/// Parse B 站 subtitle JSON to extract plain text lines.
async fn extract_subtitle_text(path: &Path) -> Vec<String> {
    let bytes = match fs::read(path).await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let v: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let body = match v.get("body").and_then(|b| b.as_array()) {
        Some(b) => b,
        None => return Vec::new(),
    };
    body.iter()
        .filter_map(|item| item.get("content").and_then(|c| c.as_str()))
        .map(|s| s.to_string())
        .collect()
}

async fn run_review(input: &str) -> Result<ReviewSection> {
    let result = review::fetch_main(input, ReviewSort::Hot, 1, 20).await?;
    let text_lines: Vec<String> = result
        .replies
        .iter()
        .map(|r| {
            let top = if r.is_top { "[置顶] " } else { "" };
            format!("{}{}: {}", top, r.uname, r.message)
        })
        .collect();

    Ok(ReviewSection {
        total: result.total,
        loaded: result.replies.len(),
        text_lines,
    })
}

async fn run_ocr(
    video_path: Option<&Path>,
    work_dir: &Path,
    interval: f32,
    max_frames: u32,
    _min_conf: f32,
    dedup_window: f32,
) -> Result<serde_json::Value> {
    let video = match video_path {
        Some(p) => p.to_path_buf(),
        None => return Err(CliError::msg(
            "OCR requires --video-path <local_video.mp4>. \
             Download the video first with `bilitools download submit <bv>`."
        )),
    };
    if !video.is_file() {
        return Err(CliError::PathNotFound(video));
    }

    let _ = (interval, max_frames, dedup_window, work_dir);

    run_ocr_impl(&video, work_dir, interval, max_frames, _min_conf, dedup_window).await
}

async fn run_ocr_impl(
    video: &Path,
    work_dir: &Path,
    interval: f32,
    max_frames: u32,
    min_conf: f32,
    dedup_window: f32,
) -> Result<serde_json::Value> {
    use crate::ipc::ocr::dedup::{merge, DedupConfig, MergedDetection, RawDetection};
    use crate::ipc::ocr::engine::OcrEngine;
    use crate::ipc::ocr::frames::{self, parse_frame_ts};
    use crate::ipc::ocr::model_paths;

    let models = model_paths::find_model()
        .map_err(|e| CliError::msg(format!("OCR model: {e}")))?;
    let engine = OcrEngine::load(&models)
        .map_err(|e| CliError::msg(format!("OCR engine: {e}")))?;

    let ocr_dir = work_dir.join("ocr_frames");
    fs::create_dir_all(&ocr_dir).await.map_err(CliError::from)?;

    let mf = if max_frames == 0 { u32::MAX } else { max_frames };
    let extract = frames::extract_frames(video, &ocr_dir, interval, mf)
        .await
        .map_err(|e| CliError::msg(format!("frame extraction: {e}")))?;

    let mut raw_dets: Vec<RawDetection> = Vec::new();
    for frame_path in &extract.frames {
        let t_sec = parse_frame_ts(frame_path).unwrap_or(0.0);
        let img = image::open(frame_path)
            .map_err(|e| CliError::msg(format!("open frame {}: {e}", frame_path.display())))?;
        match engine.recognize(&img) {
            Ok(dets) => {
                for d in dets {
                    if d.confidence >= min_conf {
                        raw_dets.push(RawDetection {
                            t_sec,
                            text: d.text,
                            confidence: d.confidence,
                            bbox: d.bbox,
                        });
                    }
                }
            }
            Err(e) => {
                // Non-fatal: one frame fails, continue
                tracing::warn!("OCR frame {}: {e}", frame_path.display());
            }
        }
    }

    // Dedup
    let dedup_cfg = DedupConfig {
        window_sec: dedup_window,
        iou_thresh: 0.6,
        text_sim_thresh: 0.5,
        frame_size: (1920.0, 1080.0),
        video_duration_sec: 0.0,
    };
    let merged: Vec<MergedDetection> = merge(&raw_dets, &dedup_cfg);

    let json = serde_json::to_value(&merged).unwrap_or(serde_json::Value::Null);

    // Clean up frames unless keep
    let _ = fs::remove_dir_all(&ocr_dir).await;

    Ok(json)
}

// ── Text formatting ────────────────────────────────────────────────

fn format_analysis_txt(r: &AnalyzeResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("=== Video Analysis: {} ===\n", r.bv));
    out.push_str(&format!("Title:       {}\n", r.title));
    if let Some(ref d) = r.description {
        out.push_str(&format!("Description: {d}\n"));
    }
    if let Some(d) = r.duration_sec {
        out.push_str(&format!("Duration:    {:.1}s\n", d));
    }
    out.push_str(&format!("Pages:       {}\n", r.pages.len()));
    out.push_str(&format!("Analyzed:    {}\n\n", r.analyzed_at));

    // Audio / transcript
    if let Some(ref a) = r.audio {
        out.push_str("=== Transcript (ASR) ===\n");
        out.push_str(&a.transcript);
        out.push_str("\n\n");
    }

    // Danmaku
    if let Some(ref d) = r.danmaku {
        out.push_str(&format!("=== Danmaku ({} live) ===\n", d.live_count));
        for t in &d.text_lines {
            out.push_str(&format!("  {t}\n"));
        }
        out.push('\n');
    }

    // Subtitles
    for s in &r.subtitles {
        out.push_str(&format!("=== Subtitle: {} ({}) ===\n", s.lan, s.lan_doc));
        for t in &s.text_lines {
            out.push_str(&format!("  {t}\n"));
        }
        out.push('\n');
    }

    // Reviews
    if let Some(ref rv) = r.reviews {
        out.push_str(&format!("=== Reviews ({} of {} loaded) ===\n", rv.loaded, rv.total));
        for t in &rv.text_lines {
            out.push_str(&format!("  {t}\n"));
        }
        out.push('\n');
    }

    // OCR
    if let Some(ref o) = r.ocr {
        out.push_str("=== OCR ===\n");
        if let Some(arr) = o.as_array() {
            for item in arr {
                let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let t_sec = item.get("first_t").and_then(|v| v.as_f64()).unwrap_or(0.0);
                out.push_str(&format!("  [{:.1}s] {text}\n", t_sec));
            }
        }
        out.push('\n');
    }

    // Degraded
    if !r.degraded.is_empty() {
        out.push_str("=== Degraded ===\n");
        for d in &r.degraded {
            out.push_str(&format!("  - {d}\n"));
        }
        out.push('\n');
    }

    out
}

/// Slugify a title for use in directory names.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(60));
    for c in s.chars() {
        if c.is_whitespace() {
            out.push('_');
        } else if c.is_control() || c == '/' || c == '\\' || c == ':' || c == '*' || c == '?' || c == '"' || c == '<' || c == '>' || c == '|' {
            // skip problematic chars
        } else {
            out.push(c);
        }
        if out.len() >= 60 {
            break;
        }
    }
    if out.is_empty() { "untitled".into() } else { out }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_chinese() {
        let s = slugify("【原神】4K 实机演示");
        // Chinese chars + brackets are safe on Linux filesystems
        assert!(s.contains("原神"));
        assert!(s.contains("4K"));
        assert!(!s.contains(' '));
    }

    #[test]
    fn slugify_truncates() {
        let s = slugify(&"a".repeat(100));
        assert!(s.len() <= 60);
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("   "), "___");
    }

    #[test]
    fn extract_danmaku_text_basic() {
        let xml = r#"<?xml version="1.0"?><i><d p="1,1,25,1,1,1,1,1">你好</d><d p="2,1,25,1,1,1,1,2">世界</d></i>"#;
        let lines = extract_danmaku_text(xml);
        assert_eq!(lines, vec!["你好", "世界"]);
    }

    #[test]
    fn extract_danmaku_text_empty() {
        assert!(extract_danmaku_text("<i></i>").is_empty());
        assert!(extract_danmaku_text("").is_empty());
    }

    #[test]
    fn extract_danmaku_text_skips_empty() {
        let xml = r#"<?xml version="1.0"?><i><d p="1,1,25,1,1,1,1,1"> </d><d p="2,1,25,1,1,1,1,2">text</d></i>"#;
        let lines = extract_danmaku_text(xml);
        assert_eq!(lines, vec!["text"]);
    }
}
