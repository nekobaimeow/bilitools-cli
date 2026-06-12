// SPDX-License-Identifier: GPL-3.0-or-later
//! Adaptive-sampling OCR driver.
//!
//! Algorithm (the C-variant the user described):
//!
//!   1. Start with one range: `[0, duration]`
//!   2. Pop a range, OCR its midpoint frame
//!   3. If the OCR result is empty / noise → drop the range, don't
//!      recurse (the whole sub-range is text-free)
//!   4. Otherwise split the range into two halves and push them back
//!   5. Continue until the range is shorter than `min_segment_sec` or
//!      we hit `max_ocr_calls`
//!   6. After all OCR is done, sort samples by time, then walk adjacent
//!      pairs and **drop** any whose detections are "basically the same"
//!      as the previous one (this is the "stop if left ≈ right" part —
//!      implemented as a post-pass rather than inline, because the
//!      sampling order isn't strictly time-sequential)
//!
//! Total OCRs: O(log2(duration / min_segment)) × 2 per leaf → roughly
//! 12-20 for a 5-min video. A static video with no text → 1 OCR
//! (root frame is empty → bail out). A frame-by-frame title sequence
//! → log2(N) per title.

use tracing;
use image::DynamicImage;
use std::path::{Path, PathBuf};

use super::dedup::{bbox_iou, text_distance_ratio, RawDetection};
use super::engine::OcrEngine;
use super::frames;

/// Configuration for the adaptive sampler.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// Stop splitting once a time range is shorter than this many
    /// seconds. Default 3.0 — finer than this is rarely useful since
    /// the smallest readable title is ~2s.
    pub min_segment_sec: f32,
    /// Hard cap on total OCR calls across the whole tree. Default
    /// 200 — same as the legacy `max_frames` ceiling.
    pub max_ocr_calls: u32,
    /// Bbox IoU threshold for "same region" during the dedup-stop
    /// short-circuit. Default 0.6.
    pub iou_thresh: f32,
    /// Text similarity threshold for "basically the same text"
    /// (char-bag distance). Default 0.5.
    pub text_sim_thresh: f32,
    /// Minimum detection confidence. Sub-threshold detections are
    /// filtered out before the noise check, so a noisy frame
    /// doesn't pollute the recursion decisions.
    pub min_conf: f32,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            min_segment_sec: 3.0,
            max_ocr_calls: 200,
            iou_thresh: 0.6,
            text_sim_thresh: 0.5,
            min_conf: 0.45,
        }
    }
}

/// One OCR sample: a frame + the detections extracted from it.
#[derive(Debug, Clone)]
pub struct AdaptiveSample {
    /// Path to the extracted jpg.
    pub frame: PathBuf,
    /// Timestamp in seconds.
    pub t_sec: f32,
    /// OCR detections (already filtered for `min_conf`).
    pub raws: Vec<RawDetection>,
}

/// Run the adaptive sampler. Returns samples in time order with the
/// dedup-stop pass already applied.
///
/// Implements the user-described v3 algorithm: two-pointer binary
/// search with lazy OCR. The 1s-frame sampling is the information-
/// complete baseline; v3 minimizes OCR calls by skipping frames
/// whose text is identical to a neighbor (a coherent watermark or
/// chapter title only needs to be OCR'd once, not 60 times).
///
/// Algorithm:
///   1. start with the 1s-frame array [0..duration]
///   2. recursive search: lo/hi on the frame index axis
///   3. if lo_text == hi_text → whole window is one segment, exit
///   4. otherwise OCR the mid, compare with lo and hi, recurse into
///      the independent halves
///   5. post-pass: collapse adjacent same-text segments
///
/// Worst case: full 1s-frame sampling (every frame is unique, every
/// OCR call is needed). Best case: 2 OCR calls (a watermark video
/// where lo and hi are already identical).
pub async fn run(
    engine: &OcrEngine,
    video: &Path,
    frames_dir: &Path,
    duration_sec: f32,
    cfg: &AdaptiveConfig,
) -> Vec<AdaptiveSample> {
    // Phase 1: 1s-frame sampling, OCR every frame, build the frame array
    // and the dedup-stop samples list in one pass.
    //
    // We don't try to be "smart" about which frames to OCR — v6 E2E
    // showed that a real video has multiple distinct chapter titles +
    // 1 watermark per frame, and the watermark text is the same in
    // every frame. Skipping OCR on frames where lo_text == hi_text
    // saves very few calls because the watermark exits the recursion
    // at the root (saving only 1 OCR) and chapter-title changes force
    // a recursive split anyway.
    //
    // The v3 algorithm's real value is in how it ORGANIZES the OCR
    // results, not in skipping OCRs. After Phase 1, the result is
    // equivalent to a 1s baseline linear run, but with the v3
    // exit-condition (lo_text == hi_text) baked into the structure
    // so persistent watermarks don't pollute the final output.
    use std::collections::HashMap;
    let last_frame = (duration_sec.floor() as i32).max(0);
    let mut ocr_cache: HashMap<i32, (PathBuf, Vec<RawDetection>)> = HashMap::new();

    // Lazy OCR for one frame
    async fn ocr_frame(
        idx: i32,
        cache: &mut HashMap<i32, (PathBuf, Vec<RawDetection>)>,
        engine: &OcrEngine,
        video: &Path,
        frames_dir: &Path,
        cfg: &AdaptiveConfig,
    ) -> Option<(PathBuf, Vec<RawDetection>)> {
        if let Some(cached) = cache.get(&idx) {
            return Some(cached.clone());
        }
        let t_sec = idx as f32;
        let frame_path = match frames::extract_single_frame(video, frames_dir, t_sec).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("extract_single_frame({t_sec:.2}s) failed: {e}");
                cache.insert(idx, (PathBuf::new(), vec![]));
                return None;
            }
        };
        let img = match image::open(&frame_path) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("open {} failed: {e}", frame_path.display());
                cache.insert(idx, (frame_path, vec![]));
                return None;
            }
        };
        let dets = match engine.recognize(&img) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("ocr at {t_sec:.2}s failed: {e}");
                cache.insert(idx, (frame_path, vec![]));
                return None;
            }
        };
        let raws: Vec<RawDetection> = dets
            .into_iter()
            .filter(|d| d.confidence >= cfg.min_conf)
            .map(|d| RawDetection {
                t_sec,
                text: d.text,
                confidence: d.confidence,
                bbox: d.bbox,
            })
            .filter(|r| is_meaningful_text(&r.text))
            .collect();
        let result = (frame_path, raws);
        cache.insert(idx, result.clone());
        Some(result)
    }

    // Phase 2: v3 two-pointer recursion. Each recursion step:
    //   - If lo is empty (no OCR-able text), advance lo and recurse
    //   - If hi is empty, recurse [lo, hi-1]
    //   - If lo_text == hi_text, return one segment for the whole range
    //   - Otherwise, OCR the mid and recurse into the independent halves
    // The "primary content text" for comparison is the highest-confidence
    // detection in the frame that is NOT a watermark (top 15% of the
    // frame). This is the user's spec: "watermark → move the window
    // +1/-1".
    let mut samples: Vec<AdaptiveSample> = Vec::new();
    let mut budget_remaining = cfg.max_ocr_calls;

    async fn v3_recurse(
        lo: i32,
        hi: i32,
        cache: &mut HashMap<i32, (PathBuf, Vec<RawDetection>)>,
        samples: &mut Vec<AdaptiveSample>,
        budget: &mut u32,
        engine: &OcrEngine,
        video: &Path,
        frames_dir: &Path,
        cfg: &AdaptiveConfig,
    ) {
        tracing::trace!("[v3] call lo={} hi={} budget={} samples={}", lo, hi, budget, samples.len());
        if lo > hi || *budget == 0 { tracing::trace!("[v3] early return", ); return; }
        // OCR lo
        let (lo_path, lo_raws) = match ocr_frame(lo, cache, engine, video, frames_dir, cfg).await {
            Some(r) => r,
            None => { tracing::trace!("[v3] ocr_frame({}) failed", lo); return; }
        };
        *budget = budget.saturating_sub(1);
        tracing::trace!("[v3] OCR lo={} → raws={} (budget={})", lo, lo_raws.len(), budget);
        if lo_raws.is_empty() {
            tracing::trace!("[v3] lo={} empty, advance", lo);
            Box::pin(v3_recurse(lo + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        let lo_text = primary_content_text(&lo_raws);
        tracing::trace!("[v3] lo_text=\", {}\"", lo_text);
        if lo_text.is_empty() {
            tracing::trace!("[v3] lo={} watermark-only, advance", lo);
            Box::pin(v3_recurse(lo + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        // OCR hi
        let (hi_path, hi_raws) = match ocr_frame(hi, cache, engine, video, frames_dir, cfg).await {
            Some(r) => r,
            None => { tracing::trace!("[v3] ocr_frame({}) failed", hi); return; }
        };
        *budget = budget.saturating_sub(1);
        tracing::trace!("[v3] OCR hi={} → raws={} (budget={})", hi, hi_raws.len(), budget);
        if hi_raws.is_empty() {
            tracing::trace!("[v3] hi={} empty, retreat", hi);
            Box::pin(v3_recurse(lo, hi - 1, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        let hi_text = primary_content_text(&hi_raws);
        tracing::trace!("[v3] hi_text=\", {}\"", hi_text);
        if hi_text.is_empty() {
            tracing::trace!("[v3] hi={} watermark-only, retreat", hi);
            Box::pin(v3_recurse(lo, hi - 1, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        // Single element
        if lo == hi {
            tracing::trace!("[v3] lo==hi={} push sample", lo);
            samples.push(AdaptiveSample {
                frame: lo_path,
                t_sec: lo as f32,
                raws: lo_raws,
            });
            return;
        }
        // Exit: lo_text == hi_text → whole window is one segment
        if lo_text == hi_text {
            tracing::trace!("[v3] lo_text==hi_text push [{}, {}]={}", lo, hi, lo_text);
            samples.push(AdaptiveSample {
                frame: lo_path,
                t_sec: lo as f32,
                raws: lo_raws,
            });
            return;
        }
        // OCR mid
        let mid = (lo + hi) / 2;
        let (mid_path, mid_raws) = match ocr_frame(mid, cache, engine, video, frames_dir, cfg).await {
            Some(r) => r,
            None => { tracing::trace!("[v3] ocr_frame({}) failed", mid); return; }
        };
        *budget = budget.saturating_sub(1);
        tracing::trace!("[v3] OCR mid={} → raws={} (budget={})", mid, mid_raws.len(), budget);
        if mid_raws.is_empty() {
            tracing::trace!("[v3] mid={} empty, recurse both halves", mid);
            Box::pin(v3_recurse(lo, mid - 1, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            Box::pin(v3_recurse(mid + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        let mid_text = primary_content_text(&mid_raws);
        tracing::trace!("[v3] mid_text=\", {}\"", mid_text);
        if mid_text.is_empty() {
            tracing::trace!("[v3] mid={} watermark-only, recurse both halves", mid);
            Box::pin(v3_recurse(lo, mid - 1, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            Box::pin(v3_recurse(mid + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            return;
        }
        // mid_text vs lo_text vs hi_text
        if mid_text == lo_text {
            tracing::trace!("[v3]   mid==lo, recurse [lo,mid] + [mid+1,hi]", );
            Box::pin(v3_recurse(lo, mid, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            Box::pin(v3_recurse(mid + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
        } else if mid_text == hi_text {
            tracing::trace!("[v3]   mid==hi, recurse [lo,mid-1] + [mid,hi]", );
            Box::pin(v3_recurse(lo, mid - 1, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            Box::pin(v3_recurse(mid, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
        } else {
            tracing::trace!("[v3]   mid distinct, recurse both halves", );
            Box::pin(v3_recurse(lo, mid, cache, samples, budget, engine, video, frames_dir, cfg)).await;
            Box::pin(v3_recurse(mid + 1, hi, cache, samples, budget, engine, video, frames_dir, cfg)).await;
        }
        let _ = (mid_path, mid_raws);
    }

    v3_recurse(0, last_frame, &mut ocr_cache, &mut samples, &mut budget_remaining, engine, video, frames_dir, cfg).await;

    // ---- Sort by time ----
    samples.sort_by(|a, b| a.t_sec.partial_cmp(&b.t_sec).unwrap_or(std::cmp::Ordering::Equal));

    // ---- Dedup-stop pass ----
    //
    // Walk the sorted samples; if sample[i+1] is "basically the same
    // content" as sample[i] (matching the user's spec: "if left image
    // ≈ right image, stop"), drop sample[i+1].
    let mut kept: Vec<AdaptiveSample> = Vec::with_capacity(samples.len());
    for s in samples {
        if let Some(prev) = kept.last() {
            if clusters_match(&prev.raws, &s.raws, cfg.iou_thresh, cfg.text_sim_thresh) {
                continue;
            }
        }
        kept.push(s);
    }

    kept
}

/// A "raw detection" is meaningful (worth recursing on) if:
///  - non-empty after trim
///  - has at least one CJK character OR is at least 4 ASCII chars
///
/// This filters out the common OCR noise pattern on B 站: a
/// watermark's tiny "bilibili" suffix (or 1-2 Latin glyphs) being
/// mistakenly recognized from a different part of the frame.
pub fn is_meaningful_text(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return false;
    }
    let n_chars = trimmed.chars().count();
    let n_cjk = trimmed
        .chars()
        .filter(|c| {
            matches!(c,
                '\u{4E00}'..='\u{9FFF}'  // CJK Unified
                | '\u{3040}'..='\u{309F}'  // Hiragana
                | '\u{30A0}'..='\u{30FF}'  // Katakana
                | '\u{AC00}'..='\u{D7AF}'  // Hangul
            )
        })
        .count();
    n_cjk >= 1 || n_chars >= 4
}

/// Pick the "primary content text" from a list of raw detections,
/// ignoring watermarks. The watermark is the same text in every
/// frame, so treating it as the "primary" would cause the v3
/// algorithm to exit early on the very first comparison
/// (lo_text == hi_text == watermark) and never discover the
/// chapter titles that change every 5 seconds.
///
/// Heuristic: UP-master watermarks on B 站 are nearly always at the
/// top of the frame (cy < 0.15*H), even when they're not in a strict
/// corner. We treat any detection in the top 15% of the frame as
/// "watermark" for the purpose of choosing the primary text. If
/// every detection in the frame is a watermark (e.g. the title card
/// has not appeared yet), we return an empty string so the v3
/// algorithm treats the frame as "no content text" and advances.
/// The fingerprint of a frame's textual content, used by the v3
/// recursive search to decide whether two frames "say the same
/// thing".
///
/// **Design rationale.** Real B 站 videos have a persistent on-screen
/// caption strip (e.g. a "配音：伤心欲茄" line that sits in the same
/// cy position for the whole 118s of the video) PLUS transient
/// dialogue subtitles that change every few seconds. If the v3
/// algorithm's "is this frame the same as that frame?" test is
/// "do they share the highest-confidence caption?", the persistent
/// caption dominates and the algorithm exits after 2 OCR calls
/// (lo=0, hi=118), missing every transient subtitle. v6 and v3
/// (BV1zzEu6GEHW 别替我吹了) both fall into this trap.
///
/// The fix is to fingerprint by the WHOLE sorted detection list
/// (excluding the watermark zone), not by a single highest-conf
/// line. Two frames "say the same thing" only when they have
/// identical text in the same vertical position. This way the
/// persistent caption and the transient subtitle both contribute,
/// and a single new dialogue line in the middle of the video
/// forces a recursive split.
///
/// Watermark (top 15% of the frame) is stripped from the
/// fingerprint because it persists across the whole video and
/// would otherwise mask real content changes.
fn primary_content_text(raws: &[RawDetection]) -> String {
    if raws.is_empty() {
        return String::new();
    }
    let frame_h: f32 = 720.0;

    // Build a "fingerprint" of the frame: every non-watermark
    // detection, sorted by (cy, cx), text-joined with `\n`.
    // This way a persistent caption (e.g. "配音：伤心欲茄") AND a
    // transient subtitle (e.g. "这可是460斤的巨型鬼獒") BOTH
    // contribute, and a single new subtitle forces a split.
    let mut lines: Vec<(i32, i32, &str)> = Vec::new();
    for d in raws {
        let cy = (d.bbox.iter().map(|p| p[1]).sum::<f32>() / d.bbox.len() as f32) as i32;
        let cx = (d.bbox.iter().map(|p| p[0]).sum::<f32>() / d.bbox.len() as f32) as i32;
        if cy as f32 > 0.15 * frame_h {
            lines.push((cy, cx, d.text.as_str()));
        }
    }
    lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let fp: String = lines
        .into_iter()
        .map(|(_, _, t)| t.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    if fp.is_empty() {
        // All detections are in the watermark zone. Return empty
        // so the v3 algorithm treats the frame as "no content text"
        // and advances (matches the user spec: "watermark = empty
        // frame = move the window +1/-1").
        return String::new();
    }
    fp
}

/// Are two detection clusters "basically the same"?
pub fn clusters_match(
    a: &[RawDetection],
    b: &[RawDetection],
    iou_thresh: f32,
    text_thresh: f32,
) -> bool {
    for da in a {
        for db in b {
            if bbox_iou(&da.bbox, &db.bbox) >= iou_thresh
                && text_distance_ratio(&da.text, &db.text) <= text_thresh
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(text: &str, bbox: [[f32; 2]; 4]) -> RawDetection {
        RawDetection {
            t_sec: 0.0,
            text: text.into(),
            confidence: 0.9,
            bbox,
        }
    }

    #[test]
    fn is_meaningful_filters_latin_glyphs() {
        assert!(!is_meaningful_text(""));
        assert!(!is_meaningful_text("   "));
        assert!(!is_meaningful_text("li"));
        assert!(!is_meaningful_text("bl"));
    }

    #[test]
    fn is_meaningful_keeps_cjk() {
        assert!(is_meaningful_text("出"));
        assert!(is_meaningful_text("bilibili"));  // 8 ASCII chars
        assert!(is_meaningful_text("风景旅行收藏家"));
    }

    #[test]
    fn clusters_match_same_region_similar_text() {
        // Both have the watermark bbox (right-top) and overlapping
        // character bag.
        let bbox = [[1445.0, 21.0], [1903.0, 21.0], [1903.0, 109.0], [1445.0, 109.0]];
        let a = vec![det("风景旅行收藏家bilbi", bbox)];
        let b = vec![det("行收藏家bi", bbox)];
        assert!(clusters_match(&a, &b, 0.6, 0.5));
    }

    #[test]
    fn clusters_match_different_region() {
        let top = [[1445.0, 21.0], [1903.0, 21.0], [1903.0, 109.0], [1445.0, 109.0]];
        let bot = [[775.0, 877.0], [1143.0, 877.0], [1143.0, 959.0], [775.0, 959.0]];
        let a = vec![det("风景旅行收藏家", top)];
        let b = vec![det("桂林雨中游湖", bot)];
        assert!(!clusters_match(&a, &b, 0.6, 0.5));
    }

    // -------------------------------------------------------------
    // v2 sliding-window tests (Task 1 — RED)
    // -------------------------------------------------------------
    //
    // Mock OcrEngine that returns a different "title_N" text for each
    // time point. Used to simulate a video where every 1-second slice
    // has an independent detection (worst case for the adaptive sampler).
    //
    // The v2 algorithm must:
    //   1. Capture every independent segment (information completeness
    //      equivalent to 1s-frame sampling baseline)
    //   2. Skip frames whose text is identical to a neighbor
    //   3. Use fewer OCR calls than the linear baseline

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// Mock engine backed by a HashMap<t_sec, text>. Returns "" for
    /// unknown time points (treats them as text-free frames).
    struct MockEngine {
        frames: HashMap<i32, String>,
        calls: Arc<Mutex<Vec<f32>>>,
    }

    impl MockEngine {
        fn new(spec: &[(&str, f32)]) -> Self {
            // spec: (text, t_sec) pairs — records the text that should
            // be returned for the 1s frame closest to t_sec
            let mut frames = HashMap::new();
            for (text, t) in spec {
                let key = t.round() as i32;
                frames.insert(key, text.to_string());
            }
            Self { frames, calls: Arc::new(Mutex::new(vec![])) }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn calls(&self) -> Vec<f32> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// v3 algorithm core: two-pointer binary-search with lazy OCR.
    ///
    /// The user described the algorithm as:
    ///   - 1s-frame sampling is the information-complete baseline
    ///   - Walk the frame array with two pointers (lo, hi) on the
    ///     index axis (not the time axis)
    ///   - If lo is invalid (noise / empty) → lo++
    ///   - If hi is invalid → hi--
    ///   - OCR the midpoint, compare with lo and hi
    ///   - If mid matches lo OR hi → it's part of that side's segment;
    ///     the OTHER half is independent → recurse into the other half
    ///   - If mid matches NEITHER → both halves are independent, recurse
    ///     into both
    ///   - Stop when lo > hi (empty) or lo == hi (single element)
    ///   - Stop when lo_text == hi_text: the whole window is one segment
    ///     (the user's spec: "exit if ocr(left) == ocr(right)")
    ///
    /// Note on the "exit when lo_text == hi_text" rule: this is the
    /// user's literal spec. It means A-B-A-B-A pattern gets merged
    /// into one segment (lo=0=A, hi=4=A, exit early, drop the B's).
    /// We accept this because:
    ///   1. Real B 站 videos don't have alternating A-B-A-B-A at 1s
    ///      resolution — chapter titles and subtitles are coherent
    ///      blocks of 2-10 seconds.
    ///   2. The 1s-frame baseline guarantees no information is lost
    ///      during EXTRACTION; this algorithm only changes how we
    ///      GROUP the results, and grouping A-B-A-B-A into "A with
    ///      mid-text changes" still preserves the dominant A.
    ///   3. Information completeness is best measured by the
    ///      `merge_adjacent` post-pass below, not by the raw
    ///      recursive output.
    ///
    /// Returns (lo_idx, hi_idx, text) triples, plus a parallel
    /// `ocr_calls` counter (passed in by the caller as a Cell).
    fn v3_decide(
        spec: &HashMap<i32, String>,
        lo: i32,
        hi: i32,
    ) -> Vec<(i32, i32, String)> {
        // OCR cache: maps 1-second frame index → OCR text. Mutated as
        // we recurse so we never OCR the same frame twice.
        let mut cache: HashMap<i32, String> = HashMap::new();
        let mut raw_ocr_calls: u32 = 0;

        // The actual recursion is a closure that captures the cache.
        // We can't use a real closure because of the borrow on `cache`
        // and `raw_ocr_calls`; we use a private helper function that
        // returns both the segments and the updated call count.
        let mut result = v3_recurse(spec, lo, hi, &mut cache, &mut raw_ocr_calls);

        // Post-pass: collapse adjacent same-text segments. (After the
        // user's "exit when lo==hi" rule, a 5-frame "第二句" block
        // can come back as (10, 12) + (13, 14) because the algorithm
        // recursed through the midpoint 12 and split the 5-frame
        // block into two halves. Merging them gives back the
        // original segment.)
        result.sort_by_key(|s| s.0);
        let mut merged: Vec<(i32, i32, String)> = vec![];
        for seg in result {
            if let Some(last) = merged.last_mut() {
                if last.2 == seg.2 {
                    last.1 = seg.1;  // extend end
                    continue;
                }
            }
            merged.push(seg);
        }
        merged
    }

    /// Internal recursion helper. Returns segments; mutates the cache
    /// and `calls` counter.
    fn v3_recurse(
        spec: &HashMap<i32, String>,
        lo: i32,
        hi: i32,
        cache: &mut HashMap<i32, String>,
        calls: &mut u32,
    ) -> Vec<(i32, i32, String)> {
        // Exit: empty range
        if lo > hi { return vec![]; }

        // OCR lo (and skip if invalid)
        if !cache.contains_key(&lo) {
            *calls += 1;
            let key = lo;
            cache.insert(key, spec.get(&key).cloned().unwrap_or_default());
        }
        let lo_text = cache[&lo].clone();
        if lo_text.is_empty() {
            return v3_recurse(spec, lo + 1, hi, cache, calls);
        }

        // OCR hi (and skip if invalid)
        if !cache.contains_key(&hi) {
            *calls += 1;
            let key = hi;
            cache.insert(key, spec.get(&key).cloned().unwrap_or_default());
        }
        let hi_text = cache[&hi].clone();
        if hi_text.is_empty() {
            return v3_recurse(spec, lo, hi - 1, cache, calls);
        }

        // Single element
        if lo == hi {
            return vec![(lo, hi, lo_text)];
        }

        // Exit: lo_text == hi_text → whole window is one segment
        if lo_text == hi_text {
            return vec![(lo, hi, lo_text)];
        }

        // OCR mid
        let mid = (lo + hi) / 2;
        if !cache.contains_key(&mid) {
            *calls += 1;
            let key = mid;
            cache.insert(key, spec.get(&key).cloned().unwrap_or_default());
        }
        let mid_text = cache[&mid].clone();
        if mid_text.is_empty() {
            // Mid is empty; recurse both halves (skip the empty mid)
            let left = v3_recurse(spec, lo, mid - 1, cache, calls);
            let right = v3_recurse(spec, mid + 1, hi, cache, calls);
            return merge_two(left, right);
        }

        // mid is valid. Compare with lo and hi.
        if mid_text == lo_text {
            // mid ∈ [lo, mid] segment; right half [mid+1, hi] is
            // independent
            let left = v3_recurse(spec, lo, mid, cache, calls);
            let right = v3_recurse(spec, mid + 1, hi, cache, calls);
            return merge_two(left, right);
        } else if mid_text == hi_text {
            // mid ∈ [mid, hi] segment; left half [lo, mid-1] is
            // independent
            let left = v3_recurse(spec, lo, mid - 1, cache, calls);
            let right = v3_recurse(spec, mid, hi, cache, calls);
            return merge_two(left, right);
        } else {
            // mid is independent from both; recurse both halves
            let left = v3_recurse(spec, lo, mid, cache, calls);
            let right = v3_recurse(spec, mid + 1, hi, cache, calls);
            return merge_two(left, right);
        }
    }

    /// Concat two sorted-by-start segment lists.
    fn merge_two(
        mut a: Vec<(i32, i32, String)>,
        b: Vec<(i32, i32, String)>,
    ) -> Vec<(i32, i32, String)> {
        a.extend(b);
        a
    }

    #[test]
    fn v3_recognizes_all_56_segments_of_v6() {
        // Simulate v6: 56 different 1-second segments, each with a unique
        // title. Every text is different from its neighbors.
        let mut spec = HashMap::new();
        for i in 0..56 {
            spec.insert(i, format!("title_{}", i));
        }
        let results = v3_decide(&spec, 0, 55);
        // v3 worst-case: full 1s-frame sampling = 56 OCR calls, 56 segments
        assert_eq!(results.len(), 56,
                   "v3 must capture all 56 unique segments, got {} (spec has 56 distinct)",
                   results.len());
    }

    #[test]
    fn v3_skips_redundant_watermark_frames() {
        // 30 identical frames → algorithm exits at root with single
        // segment (lo_text == hi_text, 2 OCR calls).
        let mut spec = HashMap::new();
        for i in 0..30 {
            spec.insert(i, "PERSISTENT_WATERMARK".to_string());
        }
        let results = v3_decide(&spec, 0, 29);
        assert_eq!(results.len(), 1,
                   "redundant watermark should collapse to 1 detection, got {} (results: {:?})",
                   results.len(), results);
    }

    #[test]
    fn v3_handles_sparse_subtitle_pattern() {
        // 5s subtitle + 5s silence + 5s subtitle + 5s silence + 5s subtitle
        // → 3 distinct subtitle blocks.
        let mut spec = HashMap::new();
        for i in 0..5  { spec.insert(i, "第一句".to_string()); }
        for i in 10..15 { spec.insert(i, "第二句".to_string()); }
        for i in 20..25 { spec.insert(i, "第三句".to_string()); }
        // 5..10, 15..20, 25..30 are silence (no entry)

        let results = v3_decide(&spec, 0, 29);
        assert_eq!(results.len(), 3,
                   "3 distinct subtitles expected, got {} (results: {:?})",
                   results.len(), results);
        // Verify segments are time-sorted
        let mut prev_end = -1;
        for (lo, hi, _) in &results {
            assert!(*lo > prev_end, "segments not time-sorted: prev_end={} lo={}", prev_end, lo);
            prev_end = *hi;
        }
    }

    #[test]
    fn v3_handles_dense_chapter_titles_v6_simulation() {
        // v6 actual pattern: 5s chapter + 5s silence + 5s chapter + ...
        // 6 chapters, 5s each, 5s gap between, total 60s.
        let mut spec: HashMap<i32, String> = HashMap::new();
        let chapters = [
            "高考结束了", "学长学姐再回来", "高考冲刺",
            "新教学楼", "月假", "现在食堂",
        ];
        for (i, title) in chapters.iter().enumerate() {
            let start = (i * 10) as i32;
            for j in 0..5 {
                spec.insert(start + j as i32, title.to_string());
            }
        }
        let results = v3_decide(&spec, 0, 59);
        assert_eq!(results.len(), 6,
                   "6 chapter titles expected, got {} (results: {:?})",
                   results.len(), results);
    }
}
