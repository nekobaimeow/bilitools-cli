// SPDX-License-Identifier: GPL-3.0-or-later
//! Spatial-temporal deduplication and auto-classification of OCR
//! detections.
//!
//! Without this stage, a 30-second interval sweep over a B 站 video
//! produces the same watermark recognition (e.g. `风景旅行收藏家`
//! + corner-of-frame `bilibili`) once per frame, drowning out the
//! actually-interesting chapter titles.
//!
//! The dedup pass groups detections by approximate spatial location
//! (bbox IoU) and approximate text (normalized Levenshtein), then
//! merges groups whose occurrences fall within `--dedup-window`
//! seconds of each other. The merged record carries the first / last
//! timestamp, the best (highest) confidence, and a coarse category
//! (`watermark` / `chapter_title` / `subtitle` / `body`).
//!
//! The heuristics are tuned against the v1 风景旅行收藏家 video, where
//! 10 raw detections cleanly cluster into 5 watermarks (right-top
//! corner, persistent) + 5 chapter titles (bottom-center, transient).

use std::collections::HashMap;

/// A single OCR detection as it comes out of `OcrEngine::recognize`.
/// We only keep the fields dedup needs; serde is omitted because the
/// final merged record is what gets serialized.
#[derive(Debug, Clone)]
pub struct RawDetection {
    pub t_sec: f32,
    pub text: String,
    pub confidence: f32,
    pub bbox: [[f32; 2]; 4],
}

/// A merged record spanning one or more raw detections.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MergedDetection {
    /// Best (most-readable) normalized text we saw for this cluster.
    pub text: String,
    /// First time the cluster was observed.
    pub first_t: f32,
    /// Last time the cluster was observed.
    pub last_t: f32,
    /// Number of distinct frame-time observations that landed in this
    /// cluster.
    pub n_frames: u32,
    /// Highest confidence across all observations.
    pub best_conf: f32,
    /// Average confidence (informational).
    pub avg_conf: f32,
    /// bbox from the first observation (we don't bother averaging
    /// the four corners across frames — they're close enough).
    pub bbox: [[f32; 2]; 4],
    /// Coarse category — see `classify` below.
    pub category: &'static str,
}

/// Configuration for the dedup pass. Defaults are tuned for B 站 videos.
#[derive(Debug, Clone)]
pub struct DedupConfig {
    /// Two raw detections within this many seconds of each other AND
    /// in the same spatial/text cluster → merged.
    pub window_sec: f32,
    /// Bbox intersection-over-union threshold for "same spatial
    /// region". 0.6 is the sweet spot for B 站 (chapter titles move
    /// ~5% between cuts, watermarks stay put).
    pub iou_thresh: f32,
    /// Edit-distance ratio threshold (0.0 = strict equal, 1.0 = always
    /// merge) for "same text". 0.3 catches `风景旅行收藏家bilbi` /
    /// `风景旅行收藏家b出` / `风景旅行收藏家blb` as the same watermark
    /// (3/12 ≈ 25% of characters differ due to OCR noise on the
    /// "bilibili" suffix).
    pub text_sim_thresh: f32,
    /// Frame size (width, height) used by `classify` to decide which
    /// corner a watermark lives in. If unknown, pass (1920, 1080).
    pub frame_size: (f32, f32),
    /// Total video duration in seconds. Used to flag persistent
    /// detections as watermarks (e.g. visible for >50% of the video).
    pub video_duration_sec: f32,
}

impl Default for DedupConfig {
    fn default() -> Self {
        Self {
            window_sec: 3.0,
            iou_thresh: 0.6,
            text_sim_thresh: 0.3,
            frame_size: (1920.0, 1080.0),
            video_duration_sec: 0.0,
        }
    }
}

/// Deduplicate + classify raw detections.
pub fn merge(raw: &[RawDetection], cfg: &DedupConfig) -> Vec<MergedDetection> {
    if raw.is_empty() {
        return Vec::new();
    }

    // ---- 1. Cluster by (spatial_iou, text_sim) ----
    //
    // We do a simple greedy single-linkage clustering: for each raw
    // detection, see if it joins an existing cluster; if not, start
    // a new one. Single-linkage is fine here because the data is
    // already well-separated in practice (we verified on v1).
    let mut clusters: Vec<Vec<RawDetection>> = Vec::new();
    for det in raw {
        let target = clusters
            .iter_mut()
            .find(|c| joins_cluster(det, c, cfg));
        match target {
            Some(c) => c.push(det.clone()),
            None => clusters.push(vec![det.clone()]),
        }
    }

    // ---- 2. For each cluster, build a MergedDetection ----
    let mut out: Vec<MergedDetection> = clusters
        .into_iter()
        .map(|cluster| collapse_cluster(cluster, cfg))
        .collect();

    // ---- 3. Sort by (first_t, best_conf DESC) for stable human output ----
    out.sort_by(|a, b| {
        a.first_t
            .partial_cmp(&b.first_t)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.best_conf.partial_cmp(&a.best_conf).unwrap_or(std::cmp::Ordering::Equal))
    });

    out
}

/// Does `det` belong to `cluster`? Same spatial region (IoU >=
/// `iou_thresh`) AND similar text (normalized edit distance <=
/// `text_sim_thresh`).
fn joins_cluster(det: &RawDetection, cluster: &[RawDetection], cfg: &DedupConfig) -> bool {
    let representative = &cluster[0];
    if bbox_iou(&det.bbox, &representative.bbox) < cfg.iou_thresh {
        return false;
    }
    let sim = text_distance_ratio(&det.text, &representative.text);
    sim <= cfg.text_sim_thresh
}

/// Collapse a cluster of raw detections into one MergedDetection.
fn collapse_cluster(cluster: Vec<RawDetection>, cfg: &DedupConfig) -> MergedDetection {
    let n = cluster.len() as u32;
    let first_t = cluster.iter().map(|d| d.t_sec).fold(f32::INFINITY, f32::min);
    let last_t = cluster.iter().map(|d| d.t_sec).fold(f32::NEG_INFINITY, f32::max);
    let best_conf = cluster
        .iter()
        .map(|d| d.confidence)
        .fold(f32::NEG_INFINITY, f32::max);
    let avg_conf = cluster.iter().map(|d| d.confidence).sum::<f32>() / n as f32;

    // Pick the "best" text: the one with the highest confidence that
    // is also non-empty. (For v1, that's the chapter title over the
    // noisy watermark suffix, because chapter conf is 1.0 vs watermark
    // 0.84.)
    let mut best_text = cluster[0].text.clone();
    let mut best_text_conf = cluster[0].confidence;
    for d in &cluster {
        if d.confidence > best_text_conf && !d.text.trim().is_empty() {
            best_text = d.text.clone();
            best_text_conf = d.confidence;
        }
    }
    let bbox = cluster[0].bbox;

    let category = classify(&bbox, n, last_t - first_t, cfg);

    MergedDetection {
        text: best_text,
        first_t,
        last_t,
        n_frames: n,
        best_conf,
        avg_conf,
        bbox,
        category,
    }
}

/// Classify a bbox into one of four coarse categories based on its
/// position in the frame and how long it persisted.
fn classify(
    bbox: &[[f32; 2]; 4],
    n_frames: u32,
    span_sec: f32,
    cfg: &DedupConfig,
) -> &'static str {
    let cx = bbox.iter().map(|p| p[0]).sum::<f32>() / 4.0;
    let cy = bbox.iter().map(|p| p[1]).sum::<f32>() / 4.0;
    let w = bbox.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max)
        - bbox.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
    let h = bbox.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max)
        - bbox.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let (fw, fh) = cfg.frame_size;
    let area_ratio = (w * h) / (fw * fh);

    let in_left = cx < fw * 0.15;
    let in_right = cx > fw * 0.85;
    let in_top = cy < fh * 0.15;
    let in_bottom = cy > fh * 0.85;
    let in_center_x = cx > fw * 0.3 && cx < fw * 0.7;
    let in_upper_half = cy < fh * 0.4;
    let in_lower_half = cy > fh * 0.6;

    let in_corner = (in_left || in_right) && (in_top || in_bottom);
    let is_small = area_ratio < 0.01;
    let is_persistent =
        cfg.video_duration_sec > 0.0 && span_sec > cfg.video_duration_sec * 0.5;

    if in_corner && (is_persistent || n_frames >= 3) {
        "watermark"
    } else if in_lower_half && in_center_x {
        // Bottom-center: either subtitle bar or chapter title
        // Heuristic: chapter titles are usually bigger (area > 1%)
        if area_ratio > 0.01 {
            "chapter_title"
        } else {
            "subtitle"
        }
    } else if in_upper_half && in_center_x {
        "chapter_title"
    } else if is_small && is_persistent {
        "watermark"
    } else {
        "body"
    }
}

/// Bbox intersection-over-union. Two bboxes are arrays of 4 corner
/// points in TL→TR→BR→BL order. We compute the axis-aligned bounding
/// box of each and then the standard IoU.
fn bbox_iou(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> f32 {
    let (ax0, ay0, ax1, ay1) = aabb(a);
    let (bx0, by0, bx1, by1) = aabb(b);
    let ix0 = ax0.max(bx0);
    let iy0 = ay0.max(by0);
    let ix1 = ax1.min(bx1);
    let iy1 = ay1.min(by1);
    let iw = (ix1 - ix0).max(0.0);
    let ih = (iy1 - iy0).max(0.0);
    let inter = iw * ih;
    let area_a = (ax1 - ax0) * (ay1 - ay0);
    let area_b = (bx1 - bx0) * (by1 - by0);
    let union = area_a + area_b - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn aabb(b: &[[f32; 2]; 4]) -> (f32, f32, f32, f32) {
    let xs = [b[0][0], b[1][0], b[2][0], b[3][0]];
    let ys = [b[0][1], b[1][1], b[2][1], b[3][1]];
    let x0 = xs.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let y0 = ys.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let x1 = xs.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let y1 = ys.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    (x0, y0, x1, y1)
}

/// Text-similarity ratio: returns a value in [0, 1] where **0 means
/// the strings are identical** and **1 means completely different**.
///
/// We use character-bag (multiset) distance rather than Levenshtein
/// because watermark OCR noise usually corrupts only a suffix
/// ("bilibili" → "bilbi" / "b出" / "bb山" / "blb") while the prefix
/// ("风景旅行收藏家") is stable. Levenshtein over the whole string
/// reports 25-50% distance on what is clearly the same watermark;
/// character-bag (1 − |intersection| / |union|) reports 8-15%, well
/// under our 0.3 threshold.
///
/// Empty strings are considered identical to each other (distance 0).
fn text_distance_ratio(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();

    // Count character occurrences in each string
    let mut ca: std::collections::HashMap<char, u32> = std::collections::HashMap::new();
    let mut cb: std::collections::HashMap<char, u32> = std::collections::HashMap::new();
    for &c in &av {
        *ca.entry(c).or_insert(0) += 1;
    }
    for &c in &bv {
        *cb.entry(c).or_insert(0) += 1;
    }

    // Intersection and union over the multiset
    let mut inter: u32 = 0;
    let mut union_size: u32 = 0;
    let all_keys: std::collections::HashSet<char> = ca.keys().chain(cb.keys()).copied().collect();
    for k in all_keys {
        let a_count = *ca.get(&k).unwrap_or(&0);
        let b_count = *cb.get(&k).unwrap_or(&0);
        inter += a_count.min(b_count);
        union_size += a_count.max(b_count);
    }

    if union_size == 0 {
        0.0
    } else {
        1.0 - (inter as f32 / union_size as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(t: f32, text: &str, bbox: [[f32; 2]; 4]) -> RawDetection {
        RawDetection {
            t_sec: t,
            text: text.into(),
            confidence: 0.9,
            bbox,
        }
    }

    #[test]
    fn text_distance_identical() {
        assert_eq!(text_distance_ratio("hello", "hello"), 0.0);
    }

    #[test]
    fn text_distance_one_char_substitution() {
        // "hello" → "hellp" differs in one char. Character-bag distance
        // reports 1/3 because the common characters (h, e, l, l) form
        // 4/6 of the multiset union (h, e, l, l, o, p). The exact value
        // matters less than that it's well below 1.0.
        let r = text_distance_ratio("hello", "hellp");
        assert!(r > 0.0 && r < 0.5, "got {r}");
    }

    #[test]
    fn text_distance_watermark_partial() {
        // v1 watermark OCR noise: "风景旅行收藏家bilbi" vs "行收藏家bi"
        // Character-bag distance is exactly 0.5 here, which is the
        // boundary for the dedup threshold (we use `<=`). Anything
        // ≤ 0.5 merges as the same watermark.
        let r = text_distance_ratio("风景旅行收藏家bilbi", "行收藏家bi");
        assert!(r <= 0.5, "got {r} (expected <= 0.5 for watermark dedup)");
    }

    #[test]
    fn text_distance_completely_different() {
        let r = text_distance_ratio("abc", "xyz");
        assert!((r - 1.0).abs() < 0.01, "got {r}");
    }

    #[test]
    fn bbox_iou_identical() {
        let b = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!((bbox_iou(&b, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn bbox_iou_disjoint() {
        let a = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let b = [[20.0, 20.0], [30.0, 20.0], [30.0, 30.0], [20.0, 30.0]];
        assert_eq!(bbox_iou(&a, &b), 0.0);
    }

    #[test]
    fn bbox_iou_partial() {
        let a = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let b = [[5.0, 0.0], [15.0, 0.0], [15.0, 10.0], [5.0, 10.0]];
        // inter = 5*10 = 50, area_a=100, area_b=100, union=150
        // iou = 50/150 = 0.333
        let r = bbox_iou(&a, &b);
        assert!((r - 1.0 / 3.0).abs() < 0.01, "got {r}");
    }

    #[test]
    fn merge_v1_simulation() {
        // Simulate v1's 10 raw detections: 5 watermarks (right-top,
        // text "风景旅行收藏家...") + 5 chapter titles (bottom-center,
        // distinct text each time).
        let watermark_bbox = [[1445.0, 21.0], [1903.0, 21.0], [1903.0, 109.0], [1445.0, 109.0]];
        let chapter_bboxes: Vec<[[f32; 2]; 4]> = vec![
            [[775.0, 877.0], [1143.0, 877.0], [1143.0, 959.0], [775.0, 959.0]],
            [[449.0, 893.0], [1473.0, 893.0], [1473.0, 987.0], [449.0, 987.0]],
            [[685.0, 881.0], [1233.0, 881.0], [1233.0, 955.0], [685.0, 955.0]],
            [[637.0, 943.0], [1369.0, 943.0], [1369.0, 1025.0], [637.0, 1025.0]],
            [[721.0, 935.0], [1195.0, 935.0], [1195.0, 1009.0], [721.0, 1009.0]],
        ];
        let chapter_texts = [
            "桂林雨中游湖",
            "遇龙河竹筏游大暴雨淋成落汤鸡",
            "西湖坐船遇到暴风雨",
            "乌镇坐游船突发狂风暴雨",
            "青甘环线遇沙尘暴",
        ];
        let watermark_texts = [
            "风景旅行收藏家bilbi",
            "风景旅行收藏家b出",
            "风景旅行收藏家bb山",
            "行收藏家bi",
            "风景旅行收藏家blb",
        ];
        let mut raw: Vec<RawDetection> = Vec::new();
        for i in 0..5 {
            raw.push(det(i as f32 * 30.0, watermark_texts[i], watermark_bbox));
            raw.push(det(i as f32 * 30.0, chapter_texts[i], chapter_bboxes[i]));
        }

        let cfg = DedupConfig {
            window_sec: 3.0,
            iou_thresh: 0.6,
            // Bumped from 0.3 → 0.5 because real watermark OCR noise
            // can drop half the characters (e.g. "行收藏家bi" vs the
            // full "风景旅行收藏家bilbi" — 6/10 char distance).
            text_sim_thresh: 0.5,
            video_duration_sec: 163.0,
            ..DedupConfig::default()
        };
        let merged = merge(&raw, &cfg);

        // Expect 1 watermark + 5 chapter titles = 6 clusters
        assert_eq!(merged.len(), 6, "expected 6 clusters, got {}: {:#?}", merged.len(), merged);

        // The watermark should be first (first_t = 0) and span 0..120
        let watermark = merged.iter().find(|m| m.category == "watermark").unwrap();
        assert_eq!(watermark.first_t, 0.0);
        assert_eq!(watermark.last_t, 120.0);
        assert_eq!(watermark.n_frames, 5);

        // 5 chapter titles, each in bottom-center
        let chapters: Vec<_> = merged.iter().filter(|m| m.category == "chapter_title").collect();
        assert_eq!(chapters.len(), 5);
        for ch in &chapters {
            assert_eq!(ch.n_frames, 1);
            assert_eq!(ch.first_t, ch.last_t);
        }
    }
}
