// SPDX-License-Identifier: GPL-3.0-or-later
// B 站 playurl client + DASH manifest parser.
//
// Endpoint: `https://api.bilibili.com/x/player/wbi/playurl` (wbi-signed).
//
// Returns either:
//   - `Dash` manifest with separate `video[]` and `audio[]` streams
//     (the modern format used for everything since ~2020).
//   - `Flv` single-stream `durl[]` segments (older videos or SD quality).
//
// The CLI only consumes the DASH form for now (it has separate
// per-quality audio + video tracks we can pick from). FLV is parsed
// and surfaced as `Stream::Flv` for downstream code, but the worker
// currently only writes the file via aria2 (one URL → one file).

use crate::error::CliError;
use crate::ipc::shared::{get_sec, init_client, wbi_sign};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// =====================  Public types  =====================

/// Top-level playurl response — we only deserialize the bits we need
/// (everything else is in `raw` for callers that want more).
///
/// All fields except `quality` and `format` are `serde(default)` so
/// we never fail on a missing key. B 站 has been known to add/remove
/// fields on a whim.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlayUrlManifest {
    #[serde(default)]
    pub quality: i64,
    #[serde(default)]
    pub format: String, // "flv" | "dash" | "mp4"
    #[serde(default)]
    pub timelength: i64, // total duration in ms
    #[serde(default)]
    pub accept_description: Vec<QualityOption>,
    #[serde(default)]
    pub accept_quality: Vec<i64>,
    #[serde(default)]
    pub dash: Option<DashManifest>,
    #[serde(default)]
    pub durl: Option<Vec<DurlEntry>>,
    /// Catch-all for unknown fields from upstream.
    #[serde(default)]
    pub raw: serde_json::Value,
}

/// One entry in B 站's `accept_description` array. The upstream
/// returns either a flat string (older versions) or a structured
/// object (current). We try the object form first; on failure we
/// fall back to `String` and wrap.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QualityOption {
    Structured {
        quality: i64,
        format: String,
        description: String,
        display_desc: String,
        superscript: String,
        codecs: Option<String>,
    },
    Legacy(String),
}

impl QualityOption {
    pub fn quality(&self) -> i64 {
        match self {
            QualityOption::Structured { quality, .. } => *quality,
            // Legacy form is just a description string; the caller
            // should not rely on the quality code in this case.
            QualityOption::Legacy(_) => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DashManifest {
    pub duration: i64,
    pub min_buffer_time: f64,
    pub video: Vec<DashStream>,
    pub audio: Vec<DashStream>,
    pub dolby: Option<DashStream>,
    pub flac: Option<DashStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashStream {
    pub id: i64,
    pub base_url: String,
    pub backup_url: Option<Vec<String>>,
    pub bandwidth: i64,
    pub mime_type: String,
    pub codecs: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub frame_rate: Option<String>,
    pub sar: Option<String>,
    pub start_with_sap: Option<i64>,
    pub segment_base: Option<DashSegmentBase>,
    pub codecid: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashSegmentBase {
    pub initialization: String,
    pub index_range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurlEntry {
    pub url: String,
    pub size: i64,
    pub length: i64,
    pub backup_url: Option<Vec<String>>,
}

/// One piece of work for the downloader: a single URL (one segment or
/// one FLV file) plus its expected size and an output filename.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayableSegment {
    pub url: String,
    pub backup_urls: Vec<String>,
    pub size: i64,
    pub out_name: String,
    pub kind: SegmentKind,
    pub bandwidth: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub codec: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    Video,
    Audio,
    Flv,
}

// =====================  Public API  =====================

/// Build a `playurl` query and execute it. Returns the parsed manifest.
///
/// `aid` + `cid` come from a prior `view` API call. `qn` is the
/// quality code (e.g. 80 = 1080P, 64 = 720P, 32 = 480P, 16 = 360P).
/// `fnval` is the streaming format flag: 1 = flv (legacy), 16 = dash.
pub async fn fetch(
    aid: i64,
    cid: i64,
    qn: i64,
    fnval: i64,
) -> Result<PlayUrlManifest, CliError> {
    // Build the base params, then sign with WBI.
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("avid".into(), aid.to_string());
    params.insert("cid".into(), cid.to_string());
    params.insert("qn".into(), qn.to_string());
    params.insert("fnval".into(), fnval.to_string());
    params.insert("fnver".into(), "0".into());
    params.insert("fourk".into(), "1".into());
    params.insert("voice_balance".into(), "1".into());
    params.insert("gaia_source".into(), "pre-load".into());
    params.insert("from".into(), "0".into());
    params.insert("is_main_page".into(), "true".into());
    // Force a fresh manifest each time (cache can return old broken
    // URLs after a quality change).
    params.insert("ts".into(), get_sec().to_string());

    let (signed_query, w_rid) = wbi_sign(&params).await?;
    let url = format!("https://api.bilibili.com/x/player/wbi/playurl?{signed_query}&w_rid={w_rid}");
    tracing::debug!("playurl url = {url}");

    let client = init_client().await?;
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            String::from("playurl api failed"),
        ));
    }
    let body: serde_json::Value = resp.json().await?;
    if body.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("playurl rejected")
            .to_string();
        return Err(CliError::api(code, msg));
    }
    let data = body
        .get("data")
        .ok_or_else(|| CliError::msg("playurl: no data"))?;
    let manifest: PlayUrlManifest = serde_json::from_value(data.clone())
        .map_err(|e| CliError::msg(format!("playurl: bad shape: {e}")))?;
    Ok(manifest)
}

/// Pick the best `qn` from `accept_quality` that is `<= max_qn`.
pub fn pick_quality(accept_quality: &[i64], max_qn: i64) -> i64 {
    accept_quality
        .iter()
        .copied()
        .filter(|&q| q <= max_qn)
        .max()
        .unwrap_or_else(|| *accept_quality.iter().min().unwrap_or(&16))
}

/// Expand a `PlayUrlManifest` into a flat list of `PlayableSegment`s.
/// The DASH path returns one video + one audio segment (we don't
/// split the m4s into byte ranges — the upstream serves a single
/// `base_url` per quality). The FLV path returns a single segment.
pub fn expand(manifest: &PlayUrlManifest) -> Vec<PlayableSegment> {
    let mut out = Vec::new();
    if let Some(dash) = &manifest.dash {
        // Pick the first video stream (playurl returns them already
        // sorted by resolution). The caller can re-fetch with a
        // different qn if they want a different quality.
        for v in &dash.video {
            out.push(PlayableSegment {
                url: v.base_url.clone(),
                backup_urls: v.backup_url.clone().unwrap_or_default(),
                size: 0, // playurl doesn't return per-stream size for DASH
                out_name: format!(
                    "video-{}-{}.m4s",
                    v.id,
                    v.width
                        .zip(v.height)
                        .map(|(w, h)| format!("{w}x{h}"))
                        .unwrap_or_else(|| v.idc().to_string())
                ),
                kind: SegmentKind::Video,
                bandwidth: v.bandwidth,
                width: v.width,
                height: v.height,
                codec: v.codecs.clone(),
            });
        }
        for a in &dash.audio {
            out.push(PlayableSegment {
                url: a.base_url.clone(),
                backup_urls: a.backup_url.clone().unwrap_or_default(),
                size: 0,
                out_name: format!("audio-{}.m4s", a.id),
                kind: SegmentKind::Audio,
                bandwidth: a.bandwidth,
                width: None,
                height: None,
                codec: a.codecs.clone(),
            });
        }
        if let Some(d) = &manifest.dash.as_ref().and_then(|d| d.dolby.clone()) {
            out.push(PlayableSegment {
                url: d.base_url.clone(),
                backup_urls: d.backup_url.clone().unwrap_or_default(),
                size: 0,
                out_name: format!("audio-dolby-{}.m4s", d.id),
                kind: SegmentKind::Audio,
                bandwidth: d.bandwidth,
                width: None,
                height: None,
                codec: d.codecs.clone(),
            });
        }
    } else if let Some(durls) = &manifest.durl {
        for (i, d) in durls.iter().enumerate() {
            out.push(PlayableSegment {
                url: d.url.clone(),
                backup_urls: d.backup_url.clone().unwrap_or_default(),
                size: d.size,
                out_name: format!("flv-{i}.bin"),
                kind: SegmentKind::Flv,
                bandwidth: 0,
                width: None,
                height: None,
                codec: String::new(),
            });
        }
    }
    out
}

impl DashStream {
    fn idc(&self) -> &i64 {
        &self.id
    }
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pick_quality_picks_highest_at_or_below() {
        let q = vec![80, 64, 32, 16];
        assert_eq!(pick_quality(&q, 80), 80);
        assert_eq!(pick_quality(&q, 32), 32);
        assert_eq!(pick_quality(&q, 100), 80);
    }

    #[test]
    fn pick_quality_falls_back_to_min() {
        let q = vec![80, 64];
        assert_eq!(pick_quality(&q, 0), 64);
    }

    #[test]
    fn expand_dash_yields_video_plus_audio() {
        let manifest = PlayUrlManifest {
            quality: 80,
            format: "dash".into(),
            timelength: 1000,
            accept_description: vec![],
            accept_quality: vec![80, 64, 32, 16],
            dash: Some(DashManifest {
                duration: 1000,
                min_buffer_time: 1.0,
                video: vec![DashStream {
                    id: 1,
                    base_url: "https://example/v.m4s".into(),
                    backup_url: Some(vec!["https://bak/v.m4s".into()]),
                    bandwidth: 1_000_000,
                    mime_type: "video/mp4".into(),
                    codecs: "avc1.640028".into(),
                    width: Some(1920),
                    height: Some(1080),
                    frame_rate: Some("30".into()),
                    sar: None,
                    start_with_sap: None,
                    segment_base: None,
                    codecid: 7,
                }],
                audio: vec![DashStream {
                    id: 30280,
                    base_url: "https://example/a.m4s".into(),
                    backup_url: None,
                    bandwidth: 320_000,
                    mime_type: "audio/mp4".into(),
                    codecs: "mp4a.40.2".into(),
                    width: None,
                    height: None,
                    frame_rate: None,
                    sar: None,
                    start_with_sap: None,
                    segment_base: None,
                    codecid: 0,
                }],
                dolby: None,
                flac: None,
            }),
            durl: None,
            raw: json!({}),
        };
        let segs = expand(&manifest);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].kind, SegmentKind::Video);
        assert_eq!(segs[0].width, Some(1920));
        assert_eq!(segs[1].kind, SegmentKind::Audio);
        assert_eq!(segs[1].codec, "mp4a.40.2");
    }

    #[test]
    fn expand_durl_yields_flv() {
        let manifest = PlayUrlManifest {
            quality: 32,
            format: "flv".into(),
            timelength: 5000,
            accept_description: vec![],
            accept_quality: vec![32],
            dash: None,
            durl: Some(vec![DurlEntry {
                url: "https://example/v.flv".into(),
                size: 12345,
                length: 5000,
                backup_url: None,
            }]),
            raw: json!({}),
        };
        let segs = expand(&manifest);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].kind, SegmentKind::Flv);
        assert_eq!(segs[0].size, 12345);
    }

    #[test]
    fn expand_empty_manifest_is_empty() {
        let manifest = PlayUrlManifest {
            quality: 16,
            format: "flv".into(),
            timelength: 0,
            accept_description: vec![],
            accept_quality: vec![16],
            dash: None,
            durl: None,
            raw: json!({}),
        };
        assert!(expand(&manifest).is_empty());
    }
}
