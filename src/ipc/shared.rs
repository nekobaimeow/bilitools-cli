// SPDX-License-Identifier: GPL-3.0-or-later
// Originally: BiliTools `src-tauri/src/shared.rs`
// Adapted for CLI:
//   - Removed `tauri::AppHandle`, `tauri::Manager`, `tauri::Theme`, `WindowEffect`.
//   - Removed `tauri_plugin_http::reqwest` — replaced with `crate::backends::http::build_client`.
//   - Removed `tauri_plugin_shell::ShellExt` — replaced with `crate::backends::sidecar`.
//   - Removed `tauri_specta::Event` — replaced with `tracing` macros.
//   - `init_client_inner` now takes a `ProxyConfig` directly instead of reading from app.

use crate::backends::http::ProxyConfig;
use crate::error::CliError;
use crate::ipc::storage::cookies;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use rand::{distr::Alphanumeric, Rng};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36";

pub const DEFAULT_REFERER: &str = "https://www.bilibili.com/";
pub const DEFAULT_ORIGIN: &str = "https://www.bilibili.com";

pub static CONFIG: Lazy<ArcSwap<ProxyConfig>> =
    Lazy::new(|| ArcSwap::from_pointee(ProxyConfig::default()));

/// Globally-shared HTTP headers (Cookie, User-Agent, Referer, Origin).
/// Mirrors `HEADERS` in BiliTools.
pub static HEADERS: Lazy<Headers> = Lazy::new(Headers::new);

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct HeadersData {
    #[serde(rename = "Cookie")]
    pub cookie: String,
    #[serde(rename = "User-Agent")]
    pub user_agent: String,
    #[serde(rename = "Referer")]
    pub referer: String,
    #[serde(rename = "Origin")]
    pub origin: String,
}

pub struct Headers {
    map: RwLock<BTreeMap<String, String>>,
}

impl Default for Headers {
    fn default() -> Self {
        Self::new()
    }
}

impl Headers {
    pub fn new() -> Self {
        let mut map = BTreeMap::new();
        map.insert("User-Agent".into(), USER_AGENT.into());
        map.insert("Referer".into(), DEFAULT_REFERER.into());
        map.insert("Origin".into(), DEFAULT_ORIGIN.into());
        map.insert("Cookie".into(), String::new());
        Self {
            map: RwLock::new(map),
        }
    }

    /// Re-read cookies from storage and rebuild the Cookie header.
    /// Replaces `Headers::refresh` in BiliTools.
    pub async fn refresh(&self) -> Result<(), CliError> {
        let mut map = self.map.write().await;
        let cookies = cookies::load()
            .await?
            .iter()
            .map(|(name, value)| {
                format!(
                    "{}={}",
                    name,
                    value
                        .to_string()
                        .replace("\\\"", "")
                        .trim_matches('"')
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        map.insert("Cookie".into(), cookies);

        // In the GUI, this emits an event so the WebView reloads. The CLI
        // has no such listener, so we just trace it.
        tracing::debug!("HEADERS refreshed (cookie length = {})", map.get("Cookie").map(|s| s.len()).unwrap_or(0));
        Ok(())
    }

    pub async fn to_header_map(&self) -> Result<HeaderMap, CliError> {
        let mut headers = HeaderMap::new();
        let map = self.map.read().await;
        for (key, value) in &*map {
            let name = HeaderName::from_bytes(key.as_bytes())
                .map_err(|e| CliError::msg(format!("invalid header name '{key}': {e}")))?;
            let val = HeaderValue::from_str(value)
                .map_err(|e| CliError::msg(format!("invalid header value for '{key}': {e}")))?;
            headers.insert(name, val);
        }
        Ok(headers)
    }

    pub async fn cookie(&self) -> String {
        self.map
            .read()
            .await
            .get("Cookie")
            .cloned()
            .unwrap_or_default()
    }
}

/// Initialize a reqwest client with the current headers, optionally using a proxy.
pub async fn init_client() -> Result<reqwest::Client, CliError> {
    init_client_inner(true).await
}

pub async fn init_client_no_proxy() -> Result<reqwest::Client, CliError> {
    init_client_inner(false).await
}

pub async fn init_client_inner(use_proxy: bool) -> Result<reqwest::Client, CliError> {
    use crate::backends::http::build_client_builder;
    let proxy = CONFIG.load();
    let mut builder = build_client_builder(&proxy, use_proxy)?;
    // Always refresh headers before building a client so the global
    // Cookie header is in sync with the SQLite cookie store. Without
    // this, the first request after a fresh `bilitools` invocation
    // ships an empty Cookie and authenticated endpoints (playurl,
    // nav) reject with -101 ("未登录").
    HEADERS.refresh().await?;
    let headers = HEADERS.to_header_map().await?;
    builder = builder.default_headers(headers);
    Ok(builder.build()?)
}

/// Replace the global proxy configuration.
pub fn set_proxy(cfg: ProxyConfig) {
    CONFIG.store(Arc::new(cfg));
}

pub fn get_sec() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn get_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn random_string(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Equivalent of `tauri::AppHandle.path().temp_dir()`.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("bilitools")
}

pub fn ensure_temp_dir() -> std::io::Result<()> {
    let p = temp_dir();
    std::fs::create_dir_all(p)
}

/// If `auto_rename` is true and `path` exists, return a new path with `_1`, `_2`, ... suffix.
/// Equivalent of `get_unique_path` in BiliTools.
pub fn get_unique_path(mut path: PathBuf, auto_rename: bool) -> PathBuf {
    if !auto_rename {
        return path;
    }
    let mut count = 1;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_string());
    while path.exists() {
        path.set_file_name(match &ext {
            Some(ext) => format!("{stem}_{count}.{ext}"),
            None => format!("{stem}_{count}"),
        });
        count += 1;
    }
    path
}

/// Equivalent of `get_image` in BiliTools — fetch a URL and write its bytes to `path`.
pub async fn get_image(path: &Path, url: &str) -> Result<(), CliError> {
    let client = init_client().await?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(CliError::http(
            response.status().as_u16(),
            format!("Error while fetching thumb {url}"),
        ));
    }
    let bytes = response.bytes().await?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, &bytes).await?;
    Ok(())
}

/// Equivalent of `process_err` — log an error and return the original.
pub fn process_err<T: ToString, U>(e: T, name: &str) -> T {
    tracing::error!("{name}: {}", e.to_string());
    e
}

// =====================  WBI signing  =====================
//
// B 站 requires every "wbi" endpoint to be signed with a per-user
// mixin key derived from the `img_url` and `sub_url` published on the
// nav response. The mixin key + algorithm are public (documented at
// https://socialsisteryi.github.io/bilibili-API-collect/docs/misc/sign/).
// The algorithm hasn't changed since 2023.
//
// `get_wbi_keys()` fetches the nav response and caches the keys in a
// process-local `OnceCell`. The keys are valid for the lifetime of the
// session (B 站 rotates them only on logout).

use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

/// B 站's WBI mixin table — a fixed 64-entry permutation used to
/// shuffle the 32-char key.
const WBI_MIXIN_TABLE: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42,
    19, 29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51,
    30, 4, 22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// Cached `(img_key, sub_key)` tuple. Populated on first call to
/// `wbi_sign_with()` and never invalidated (matches B 站's session
/// lifecycle).
static WBI_KEYS: OnceLock<AsyncMutex<Option<(String, String)>>> = OnceLock::new();

fn wbi_keys_cell() -> &'static AsyncMutex<Option<(String, String)>> {
    WBI_KEYS.get_or_init(|| AsyncMutex::new(None))
}

/// Fetch (and cache) the per-session `img_key` + `sub_key`.
///
/// Endpoint: `https://api.bilibili.com/x/web-interface/nav`
///
/// Requires valid login cookies — B 站 returns the wrong keys (or -352
/// "风控校验失败") for anonymous clients.
pub async fn get_wbi_keys() -> Result<(String, String), CliError> {
    {
        let guard = wbi_keys_cell().lock().await;
        if let Some(k) = guard.as_ref() {
            return Ok(k.clone());
        }
    }
    let client = init_client_no_proxy().await?;
    let resp = client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(
            resp.status().as_u16(),
            String::from("nav api failed; cannot fetch wbi keys"),
        ));
    }
    let body: serde_json::Value = resp.json().await?;
    if body.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("nav api rejected")
            .to_string();
        return Err(CliError::api(code, msg));
    }
    let data = body
        .get("data")
        .ok_or_else(|| CliError::msg("nav api: no data"))?;
    let wbi_img = data
        .get("wbi_img")
        .ok_or_else(|| CliError::msg("nav api: no wbi_img"))?;
    let img_url = wbi_img
        .get("img_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::msg("nav api: no wbi_img.img_url"))?;
    let sub_url = wbi_img
        .get("sub_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::msg("nav api: no wbi_img.sub_url"))?;
    let img_key = extract_wbi_key(img_url)
        .ok_or_else(|| CliError::msg("nav api: bad img_url"))?;
    let sub_key = extract_wbi_key(sub_url)
        .ok_or_else(|| CliError::msg("nav api: bad sub_url"))?;
    let mut guard = wbi_keys_cell().lock().await;
    if guard.is_none() {
        *guard = Some((img_key.clone(), sub_key.clone()));
    }
    Ok((img_key, sub_key))
}

/// Reset the cached WBI keys (test-only helper). The next call to
/// `get_wbi_keys()` will re-fetch from the network.
#[cfg(test)]
pub async fn reset_wbi_keys_for_test() {
    let mut g = wbi_keys_cell().lock().await;
    *g = None;
}

/// Extract the filename (minus extension) from a B 站 face URL.
///
/// Example: `https://i0.hdslb.com/bfs/face/8b0f6e5c0edaffc5b4d7c0af6df8a7e8f8b8c8d8.png`
///          → `8b0f6e5c0edaffc5b4d7c0af6df8a7e8f8b8c8d8`
fn extract_wbi_key(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    let last = url.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    let stem = last.rsplit_once('.').map(|(s, _)| s).unwrap_or(last);
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_string())
}

/// Compute the per-request WBI mixin key (32 chars) by permuting the
/// concatenation of `img_key` and `sub_key` through `WBI_MIXIN_TABLE`.
///
/// Algorithm:
///   1. `raw = img_key (32 chars) + sub_key (32 chars)` = 64 chars total.
///   2. For `i` in `0..32`, output `raw[WBI_MIXIN_TABLE[i]]`.
///
/// The first 32 entries of the mixin table are all in `[0, 64)`, so
/// they index into either the img half (index 0..31) or the sub half
/// (index 32..63). The table was carefully chosen so no entry
/// exceeds 32 for the img half (entries 32+ are valid for sub).
pub fn compute_mixin_key(img_key: &str, sub_key: &str) -> String {
    let raw: Vec<char> = format!("{img_key}{sub_key}").chars().collect();
    if raw.len() < 64 {
        return String::new();
    }
    let mut out = String::with_capacity(32);
    for i in 0..32 {
        let idx = WBI_MIXIN_TABLE[i];
        if let Some(&c) = raw.get(idx) {
            out.push(c);
        }
    }
    out
}

/// URL-encode (percent-encode) the characters B 站 requires for
/// `w_rid` calculation. Per the docs these are:
///   `! ' ( ) *` are kept, but `,` and a few others are filtered.
fn wbi_filter(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '!' | '\'' | '(' | ')' | '*' => out.push(c),
            // ' ' is encoded as %20
            c if c.is_ascii_alphanumeric() || "~-_.".contains(c) => out.push(c),
            _ => {
                // UTF-8 percent-encode
                for b in c.to_string().as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

/// Sign a flat key→value map for use with a `wbi` endpoint.
///
/// Returns `(query_string_no_w_rid, w_rid)`. The caller is responsible
/// for appending `&wts=<ts>&w_rid=<w_rid>` to the final URL.
pub async fn wbi_sign(params: &std::collections::BTreeMap<String, String>) -> Result<(String, String), CliError> {
    let (img_key, sub_key) = get_wbi_keys().await?;
    let mixin = compute_mixin_key(&img_key, &sub_key);
    let wts = get_sec().to_string();
    let mut signed: std::collections::BTreeMap<String, String> = params.clone();
    signed.insert("wts".into(), wts.clone());
    // Build the encoded query string in sorted key order.
    let query: String = signed
        .iter()
        .map(|(k, v)| format!("{}={}", wbi_filter(k), wbi_filter(v)))
        .collect::<Vec<_>>()
        .join("&");
    // w_rid = md5(query + mixin_key)
    use md5::{Md5, Digest};
    let mut h = Md5::new();
    h.update(format!("{query}{mixin}").as_bytes());
    let w_rid = format!("{:x}", h.finalize());
    Ok((query, w_rid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn headers_init_has_default_keys() {
        let h = Headers::new();
        let map = h.map.blocking_read();
        assert!(map.contains_key("User-Agent"));
        assert!(map.contains_key("Referer"));
        assert!(map.contains_key("Origin"));
        assert!(map.contains_key("Cookie"));
        assert_eq!(map.get("User-Agent").unwrap(), USER_AGENT);
    }

    #[test]
    fn headers_to_map_succeeds() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let h = Headers::new();
            let map = h.to_header_map().await.unwrap();
            assert!(map.contains_key("user-agent"));
            assert!(map.contains_key("referer"));
            assert!(map.contains_key("origin"));
        });
    }

    #[test]
    fn get_sec_and_millis_sensible() {
        let s = get_sec();
        let m = get_millis();
        // 2025-01-01 ~ 1735689600; 2026 ~ 1767225600
        assert!(s > 1_700_000_000, "sec should be > 2023, got {s}");
        assert!(m > s * 1000, "millis should be > sec*1000");
        assert!(m - s * 1000 < 1000, "millis excess should be < 1000");
    }

    #[test]
    fn random_string_length() {
        assert_eq!(random_string(0).len(), 0);
        assert_eq!(random_string(8).len(), 8);
        assert_eq!(random_string(32).len(), 32);
    }

    #[test]
    fn random_string_differs() {
        let a = random_string(32);
        let b = random_string(32);
        assert_ne!(a, b);
    }

    #[test]
    fn get_unique_path_no_collision() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.txt");
        std::fs::write(&p, b"").unwrap();
        let unique = get_unique_path(p.clone(), true);
        assert_ne!(unique, p);
        assert!(unique.to_string_lossy().contains("x_1.txt"));
    }

    #[test]
    fn get_unique_path_disabled_returns_same() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("y.txt");
        std::fs::write(&p, b"").unwrap();
        let unique = get_unique_path(p.clone(), false);
        assert_eq!(unique, p);
    }

    #[test]
    fn temp_dir_is_bilitools_named() {
        let p = temp_dir();
        assert!(p.ends_with("bilitools"));
    }

    // =====================  WBI signing  =====================

    #[test]
    fn extract_wbi_key_strips_path_and_extension() {
        let url = "https://i0.hdslb.com/bfs/face/8b0f6e5c0edaffc5b4d7c0af6df8a7e8f8b8c8d8.png";
        assert_eq!(
            extract_wbi_key(url).as_deref(),
            Some("8b0f6e5c0edaffc5b4d7c0af6df8a7e8f8b8c8d8")
        );
    }

    #[test]
    fn extract_wbi_key_handles_no_extension() {
        let url = "https://example.com/keyabc";
        assert_eq!(extract_wbi_key(url).as_deref(), Some("keyabc"));
    }

    #[test]
    fn extract_wbi_key_handles_empty() {
        assert!(extract_wbi_key("").is_none());
        assert!(extract_wbi_key("https://example.com/").is_none());
    }

    /// Reference vector: this is a real (img_key, sub_key) pair from
    /// B 站's nav response, with a known-good mixin key. Locking the
    /// output of `compute_mixin_key` to a fixed value guards against
    /// off-by-one errors in the table-indexing algorithm.
    #[test]
    fn compute_mixin_key_matches_known_vector() {
        // From the B 站 wbi docs (https://socialsisteryi.github.io/bilibili-API-collect/docs/misc/sign/wbi.html#%E7%AE%97%E6%B3%95%E6%AD%A5%E9%AA%A4)
        // Test vector: img = "7cd084941338484aae1ad9425b84077c", sub = "4932caff0ff746eab6f01bf08b70ac45"
        // Expected mixin key = "ea1db124af3c7062474693fa704f4ff8"
        let img = "7cd084941338484aae1ad9425b84077c";
        let sub = "4932caff0ff746eab6f01bf08b70ac45";
        let mixin = compute_mixin_key(img, sub);
        assert_eq!(mixin, "ea1db124af3c7062474693fa704f4ff8");
    }

    #[test]
    fn compute_mixin_key_returns_32_chars() {
        let img = "a".repeat(32);
        let sub = "b".repeat(32);
        let mixin = compute_mixin_key(&img, &sub);
        assert_eq!(mixin.len(), 32);
    }

    #[test]
    fn compute_mixin_key_rejects_short_input() {
        let mixin = compute_mixin_key("short", "alsoshort");
        assert!(mixin.is_empty());
    }

    #[test]
    fn wbi_filter_keeps_safe_chars() {
        assert_eq!(wbi_filter("abc-DEF_123.~"), "abc-DEF_123.~");
    }

    #[test]
    fn wbi_filter_keeps_bang_paren_star_quote() {
        // Per the spec: ! ' ( ) * are kept as-is.
        assert_eq!(wbi_filter("hello!world"), "hello!world");
        assert_eq!(wbi_filter("(parens)"), "(parens)");
    }

    #[test]
    fn wbi_filter_encodes_unicode() {
        // '中' (U+4E2D) encodes to E4 B8 AD in UTF-8.
        assert_eq!(wbi_filter("中"), "%E4%B8%AD");
    }

    #[test]
    fn wbi_filter_encodes_space() {
        assert_eq!(wbi_filter("a b"), "a%20b");
    }
}
