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
    #[serde(default)]
    pub duration: i64,
    #[serde(default)]
    pub min_buffer_time: f64,
    #[serde(default)]
    pub video: Vec<DashStream>,
    #[serde(default)]
    pub audio: Vec<DashStream>,
    #[serde(default)]
    pub dolby: Option<DashStream>,
    #[serde(default)]
    pub flac: Option<DashStream>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DashStream {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub backup_url: Option<Vec<String>>,
    #[serde(default)]
    pub bandwidth: i64,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub codecs: String,
    #[serde(default)]
    pub width: Option<i64>,
    #[serde(default)]
    pub height: Option<i64>,
    #[serde(default)]
    pub frame_rate: Option<String>,
    #[serde(default)]
    pub sar: Option<String>,
    #[serde(default)]
    pub start_with_sap: Option<i64>,
    #[serde(default)]
    pub segment_base: Option<serde_json::Value>,
    #[serde(default)]
    pub codecid: i64,
}

impl<'de> Deserialize<'de> for DashStream {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize the entire stream as a `serde_json::Value` (an
        // object) and pick whichever of the candidate names is
        // present. This is the only way to handle B 站 returning the
        // same logical field under both `baseUrl` and `base_url`
        // — sometimes both at once — without serde complaining about
        // duplicate fields.
        let v = serde_json::Value::deserialize(deserializer)?;
        let obj = match &v {
            serde_json::Value::Object(m) => m,
            _ => {
                return Err(serde::de::Error::custom(
                    "DashStream: expected a JSON object",
                ));
            }
        };

        fn pick_string(v: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> String {
            for k in keys {
                if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
                    return s.to_string();
                }
            }
            String::new()
        }
        fn pick_opt_string(v: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
            for k in keys {
                if let Some(s) = v.get(*k).and_then(|x| x.as_str()) {
                    return Some(s.to_string());
                }
            }
            None
        }
        fn pick_i64(v: &serde_json::Map<String, serde_json::Value>, keys: &[&str], default: i64) -> i64 {
            for k in keys {
                if let Some(n) = v.get(*k).and_then(|x| x.as_i64()) {
                    return n;
                }
            }
            default
        }
        fn pick_opt_i64(v: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<i64> {
            for k in keys {
                if let Some(n) = v.get(*k).and_then(|x| x.as_i64()) {
                    return Some(n);
                }
            }
            None
        }
        fn pick_string_array(v: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<Vec<String>> {
            for k in keys {
                if let Some(arr) = v.get(*k).and_then(|x| x.as_array()) {
                    let out: Vec<String> = arr
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                    return Some(out);
                }
            }
            None
        }
        fn pick_segbase(v: &serde_json::Map<String, serde_json::Value>) -> Option<serde_json::Value> {
            for k in &["SegmentBase", "segment_base"] {
                if let Some(s) = v.get(*k) {
                    return Some(s.clone());
                }
            }
            None
        }

        Ok(DashStream {
            id: pick_i64(obj, &["id"], 0),
            base_url: pick_string(obj, &["base_url", "baseUrl"]),
            backup_url: pick_string_array(obj, &["backup_url", "backupUrl"]),
            bandwidth: pick_i64(obj, &["bandwidth"], 0),
            mime_type: pick_string(obj, &["mime_type", "mimeType"]),
            codecs: pick_string(obj, &["codecs"]),
            width: pick_opt_i64(obj, &["width"]),
            height: pick_opt_i64(obj, &["height"]),
            frame_rate: pick_opt_string(obj, &["frame_rate", "frameRate"]),
            sar: pick_opt_string(obj, &["sar"]),
            start_with_sap: pick_opt_i64(obj, &["start_with_sap", "startWithSap"]),
            segment_base: pick_segbase(obj),
            codecid: pick_i64(obj, &["codecid"], 0),
        })
    }
}

// =====================  Dual-name field deserializers  =====================
//
// B 站's playurl API returns the same logical field under both
// camelCase and snake_case names — and sometimes both at once.
// `#[serde(alias = "X")]` fails on duplicates because serde sees the
// same target struct field being assigned twice. The fix is to
// install a `MapVisitor` that consumes all map keys (without
// complaining about unrecognized ones) and then we look up whichever
// of the candidate names we want.

use serde::de::{Deserializer, MapAccess, Visitor};
use std::fmt;

struct PickFirstOf<'a>(&'a [&'a str]);

impl<'de, 'a> Visitor<'de> for PickFirstOf<'a> {
    type Value = serde_json::Value;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a JSON object with at least one of {:?}", self.0)
    }
    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        // Walk every key, store the first matching value in `Value::Object`
        // so the upstream caller can pick whichever key they want.
        let mut map = serde_json::Map::new();
        while let Some((key, value)) = access.next_entry::<String, serde_json::Value>()? {
            map.entry(key).or_insert(value);
        }
        Ok(serde_json::Value::Object(map))
    }
}

fn first_value<'de, D>(deserializer: D, keys: &[&str]) -> Result<Option<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = deserializer.deserialize_map(PickFirstOf(keys))?;
    if let serde_json::Value::Object(map) = v {
        for k in keys {
            if let Some(found) = map.get(*k) {
                return Ok(Some(found.clone()));
            }
        }
    }
    Ok(None)
}

fn deserialize_first_url<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(first_value(deserializer, &["base_url", "baseUrl"])?
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default())
}

fn deserialize_first_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(first_value(deserializer, &["mime_type", "mimeType"])?
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default())
}

fn deserialize_first_string_opt<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(first_value(deserializer, &["frame_rate", "frameRate"])?
        .and_then(|v| v.as_str().map(String::from)))
}

fn deserialize_first_string_array_opt<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(first_value(deserializer, &["backup_url", "backupUrl"])?
        .and_then(|v| match v {
            serde_json::Value::Array(arr) => Some(
                arr.into_iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
            ),
            serde_json::Value::String(s) => Some(vec![s]),
            _ => None,
        }))
}

/// A `DashSegmentBase` that tolerates B 站 returning it under
/// `SegmentBase` (camelCase) or `segment_base` (snake_case) — or both.
/// We deserialize the whole map, then pick the first non-empty pair.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DashSegmentBase {
    #[serde(default)]
    pub initialization: String,
    #[serde(default)]
    pub index_range: String,
}

impl DashSegmentBase {
    /// Parse from a raw `Value` (which is the entire stream object),
    /// looking for `SegmentBase` or `segment_base`.
    pub fn from_value(v: &serde_json::Value) -> Option<Self> {
        let seg = v
            .get("SegmentBase")
            .or_else(|| v.get("segment_base"))?;
        let init = seg
            .get("Initialization")
            .or_else(|| seg.get("initialization"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let idx = seg
            .get("indexRange")
            .or_else(|| seg.get("index_range"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if init.is_empty() && idx.is_empty() {
            None
        } else {
            Some(Self {
                initialization: init,
                index_range: idx,
            })
        }
    }
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
    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            String::from("playurl api failed"),
        ));
    }
    let body_bytes = resp.bytes().await.map_err(CliError::from)?;
    let body: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| CliError::msg(format!("playurl json: {e}")))?;
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
///
/// DASH manifests include **every** quality tier (112/80/64/32/16) as
/// separate `dash.video[]` entries. We only want the one matching
/// `manifest.quality` (which B 站's API already chose for us based
/// on the requested `qn` + the user's account). For audio we pick
/// the first entry — there is usually one (or one Dolby + one AAC).
pub fn expand(manifest: &PlayUrlManifest) -> Vec<PlayableSegment> {
    let mut out = Vec::new();
    if let Some(dash) = &manifest.dash {
        // DASH video: pick the single stream whose `id` matches the
        // manifest's `quality` field. B 站 uses the same `id` (i.e.
        // qn code) for `dash.video[i].id` and the chosen `quality`.
        let chosen_video = dash
            .video
            .iter()
            .find(|v| v.id == manifest.quality)
            .or_else(|| dash.video.first());
        if let Some(v) = chosen_video {
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
                        .unwrap_or_else(|| v.id.to_string())
                ),
                kind: SegmentKind::Video,
                bandwidth: v.bandwidth,
                width: v.width,
                height: v.height,
                codec: v.codecs.clone(),
            });
        }
        // DASH audio: pick the first audio stream. The list is usually
        // ordered by quality (highest bitrate first); the manifest's
        // `quality` field doesn't apply to audio.
        for a in dash.audio.iter().take(1) {
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

    /// B 站 fnval=16 (DASH) responses ship the same logical field
    /// under both camelCase and snake_case names — sometimes both at
    /// once. The `deserialize_with` helpers in this module must accept
    /// either name without failing.
    #[test]
    fn dash_stream_accepts_snake_case_only() {
        let raw = r#"{
            "id": 32,
            "base_url": "https://example/v.m4s",
            "bandwidth": 100000,
            "mime_type": "video/mp4",
            "codecs": "avc1.640033",
            "width": 1920,
            "height": 1080
        }"#;
        let s: DashStream = serde_json::from_str(raw).unwrap();
        assert_eq!(s.base_url, "https://example/v.m4s");
        assert_eq!(s.width, Some(1920));
        assert_eq!(s.height, Some(1080));
    }

    #[test]
    fn dash_stream_accepts_camel_case_only() {
        let raw = r#"{
            "id": 32,
            "baseUrl": "https://example/v.m4s",
            "bandwidth": 100000,
            "mimeType": "video/mp4",
            "codecs": "avc1.640033",
            "width": 1920,
            "height": 1080,
            "frameRate": "30.000"
        }"#;
        let s: DashStream = serde_json::from_str(raw).unwrap();
        assert_eq!(s.base_url, "https://example/v.m4s");
        assert_eq!(s.frame_rate.as_deref(), Some("30.000"));
    }

    /// This is the case that broke real downloads: B 站 returns BOTH
    /// `baseUrl` AND `base_url` (and the same for `mimeType`,
    /// `frameRate`, `backupUrl`/`backup_url`). Serde's `#[serde(alias)]`
    /// barfs on duplicates, so we use `deserialize_with` to read into
    /// a `Value` first and pick whichever is present.
    #[test]
    fn dash_stream_accepts_both_names_at_once() {
        let raw = r#"{
            "id": 32,
            "baseUrl": "https://camel/v.m4s",
            "base_url": "https://snake/v.m4s",
            "backupUrl": ["https://camel/bak.m4s"],
            "backup_url": ["https://snake/bak.m4s"],
            "bandwidth": 100000,
            "mimeType": "video/mp4",
            "mime_type": "video/mp4",
            "codecs": "avc1.640033",
            "width": 1920,
            "height": 1080,
            "frameRate": "30.000",
            "frame_rate": "30.000",
            "startWithSap": 1,
            "start_with_sap": 1,
            "SegmentBase": {"Initialization": "0-916", "indexRange": "917-2732"},
            "segment_base": {"initialization": "0-916", "index_range": "917-2732"}
        }"#;
        let s: DashStream = serde_json::from_str(raw).expect("must not fail on dual-name duplicates");
        // snake_case wins (first in our key list).
        assert_eq!(s.base_url, "https://snake/v.m4s");
        assert_eq!(s.mime_type, "video/mp4");
        assert_eq!(s.frame_rate.as_deref(), Some("30.000"));
        assert!(s.backup_url.is_some());
        assert_eq!(s.backup_url.as_ref().unwrap().len(), 1);
        assert!(s.segment_base.is_some());
    }

    #[test]
    fn dash_stream_minimal_required() {
        // Most minimal possible DASH video stream. All non-required
        // fields should default.
        let raw = r#"{
            "id": 7,
            "baseUrl": "https://example/v.m4s"
        }"#;
        let s: DashStream = serde_json::from_str(raw).unwrap();
        assert_eq!(s.id, 7);
        assert_eq!(s.base_url, "https://example/v.m4s");
        assert_eq!(s.bandwidth, 0);
        assert_eq!(s.mime_type, "");
        assert_eq!(s.codecs, "");
        assert_eq!(s.width, None);
    }
}
