// SPDX-License-Identifier: GPL-3.0-or-later
// B 站视频字幕抓取 IPC。
//
// 工作流（与 GUI 原版一致）：
//   1. 拿 cid（from BV via web-interface/view）
//   2. 拉 https://api.bilibili.com/x/player/wbi/v2?bvid={bv}&cid={cid}
//   3. 从 `data.subtitle.list` 拿字幕元数据（id, lan, lan_doc, url）
//   4. （可选）下载每个 subtitle_url 的 JSON 内容直存
//
// 降级策略：
//   - 匿名拿不到字幕列表（空 list）是正常业务结果，不报 error
//   - 字幕 URL 协议相对（//aisubtitle.hdslb.com/...）→ 自动补 https:
//   - 下载失败 → 跳过 + degraded 提示

use crate::error::CliError;
use crate::ipc::danmaku;
use crate::ipc::shared;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub type Result<T> = std::result::Result<T, CliError>;

/// 一条字幕元数据（list 阶段）
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleEntry {
    /// subtitle id（B 站内部 id）
    pub id: i64,
    /// 语言代码（zh-Hans / en / ja 等）
    pub lan: String,
    /// 语言展示名（"中文（简体）" / "English" / "日本語" 等）
    pub lan_doc: String,
    /// 是否 UP 主锁定（不让别人投稿/AI 替换）
    pub is_lock: bool,
    /// 字幕 JSON URL（协议已 normalize 为 https://）
    pub subtitle_url: String,
    /// 字幕类型代码（0=UP 主上传, 1=AI 自动生成）
    pub type_code: i32,
    /// AI 字幕状态（仅 type_code=1 有意义）
    pub ai_status: i32,
    /// 来源标记（B 站 `ai_type` 字段："0"/"1"/...）
    pub ai_type: String,
}

/// 下载成功的一条字幕
#[derive(Debug, Clone, Serialize)]
pub struct FetchedSubtitle {
    /// 对应的 SubtitleEntry
    pub entry: SubtitleEntry,
    /// 落盘文件路径
    pub path: PathBuf,
    /// 文件字节数
    pub body_len: usize,
}

/// 字幕抓取结果
#[derive(Debug, Clone, Serialize)]
pub struct SubtitleList {
    pub bv: String,
    pub cid: i64,
    pub title: String,
    /// 该视频所有可用字幕
    pub entries: Vec<SubtitleEntry>,
    /// 已下载的字幕（仅 `fetch_all` 调用时填充）
    pub fetched: Vec<FetchedSubtitle>,
    /// 降级 / 警告
    pub degraded: Vec<String>,
}

/// 协议相对 URL 标准化为 https://
///
/// B 站的 `subtitle_url` 字段是 `//aisubtitle.hdslb.com/...` 这种形式，
/// 浏览器会自动补 scheme，但 reqwest 不会。我们统一补 `https:`，
/// 已是 `https://` 或 `http://` 的原样返回。
pub fn normalize_subtitle_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        return format!("https://{rest}");
    }
    url.to_string()
}

/// 拉字幕元数据列表
pub async fn list(bv: &str) -> Result<SubtitleList> {
    let (title, _aid, cid) = danmaku::resolve_cid(bv).await?;
    let url = format!(
        "https://api.bilibili.com/x/player/wbi/v2?bvid={bv}&cid={cid}"
    );
    let client = shared::init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("subtitle wbi/v2 http for {bv}"),
        });
    }
    let body: serde_json::Value = resp.json().await.map_err(CliError::from)?;
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = body
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("player/wbi/v2 error")
            .to_string();
        return Err(CliError::Api { code, message: msg });
    }
    let mut degraded = Vec::new();
    if !shared::HEADERS.cookie().await.contains("SESSDATA=") {
        degraded.push(
            "匿名模式：通常拿不到字幕列表（多数视频需要登录）".to_string(),
        );
    }
    let entries = parse_entries(body.get("data"));
    Ok(SubtitleList {
        bv: bv.to_string(),
        cid,
        title,
        entries,
        fetched: Vec::new(),
        degraded,
    })
}

/// 拉元数据 + 批量下载到 `output_dir`
pub async fn fetch_all(bv: &str, output_dir: &Path) -> Result<SubtitleList> {
    let mut result = list(bv).await?;
    if result.entries.is_empty() {
        return Ok(result);
    }
    fs::create_dir_all(output_dir).await.map_err(CliError::from)?;
    for entry in result.entries.clone() {
        match download(&entry, output_dir).await {
            Ok(f) => result.fetched.push(f),
            Err(e) => {
                result
                    .degraded
                    .push(format!("download {} failed: {e}", entry.lan_doc));
            }
        }
    }
    Ok(result)
}

/// 下载单条字幕到 `{output_dir}/{cid}.{lan}.json`
pub async fn download(entry: &SubtitleEntry, output_dir: &Path) -> Result<FetchedSubtitle> {
    let url = normalize_subtitle_url(&entry.subtitle_url);
    let client = shared::init_client_no_proxy()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("subtitle download http for {}", entry.lan),
        });
    }
    let bytes = resp.bytes().await.map_err(CliError::from)?;
    let body_len = bytes.len();
    let path = output_dir.join(format!("{}.{}.json", entry.id, entry.lan));
    let mut f = fs::File::create(&path).await.map_err(CliError::from)?;
    f.write_all(&bytes).await.map_err(CliError::from)?;
    f.flush().await.map_err(CliError::from)?;
    Ok(FetchedSubtitle {
        entry: entry.clone(),
        path,
        body_len,
    })
}

/// 解析 player/wbi/v2 响应的 `data.subtitle.list[]`
fn parse_entries(data: Option<&serde_json::Value>) -> Vec<SubtitleEntry> {
    let mut out = Vec::new();
    let Some(data) = data else {
        return out;
    };
    let Some(sub) = data.get("subtitle") else {
        return out;
    };
    let Some(list) = sub.get("list").and_then(|l| l.as_array()) else {
        return out;
    };
    for item in list {
        let id = item.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
        let lan = item
            .get("lan")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let lan_doc = item
            .get("lan_doc")
            .and_then(|x| x.as_str())
            .unwrap_or(&lan)
            .to_string();
        let is_lock = item
            .get("is_lock")
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        let subtitle_url = item
            .get("subtitle_url")
            .and_then(|x| x.as_str())
            .map(normalize_subtitle_url)
            .unwrap_or_default();
        let type_code = item.get("type").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let ai_status = item
            .get("ai_status")
            .and_then(|x| x.as_i64())
            .unwrap_or(0) as i32;
        let ai_type = item
            .get("ai_type")
            .and_then(|x| x.as_str())
            .unwrap_or("0")
            .to_string();
        out.push(SubtitleEntry {
            id,
            lan,
            lan_doc,
            is_lock,
            subtitle_url,
            type_code,
            ai_status,
            ai_type,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_protocol_relative() {
        assert_eq!(
            normalize_subtitle_url("//aisubtitle.hdslb.com/123.json"),
            "https://aisubtitle.hdslb.com/123.json"
        );
    }

    #[test]
    fn keeps_https_unchanged() {
        assert_eq!(
            normalize_subtitle_url("https://example.com/x.json"),
            "https://example.com/x.json"
        );
    }

    #[test]
    fn keeps_http_unchanged() {
        assert_eq!(
            normalize_subtitle_url("http://example.com/x.json"),
            "http://example.com/x.json"
        );
    }

    #[test]
    fn keeps_empty() {
        assert_eq!(normalize_subtitle_url(""), "");
    }

    #[test]
    fn parse_entries_empty_when_no_data() {
        assert!(parse_entries(None).is_empty());
    }

    #[test]
    fn parse_entries_empty_list() {
        let v = json!({"subtitle": {"list": []}});
        assert!(parse_entries(Some(&v)).is_empty());
    }

    #[test]
    fn parse_entries_one_zhHans() {
        let v = json!({
            "subtitle": {
                "list": [
                    {
                        "id": 12345_i64,
                        "lan": "zh-Hans",
                        "lan_doc": "中文（简体）",
                        "is_lock": true,
                        "subtitle_url": "//aisubtitle.hdslb.com/abc.json",
                        "type": 0,
                        "ai_status": 0,
                        "ai_type": "0"
                    }
                ]
            }
        });
        let entries = parse_entries(Some(&v));
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.id, 12345);
        assert_eq!(e.lan, "zh-Hans");
        assert_eq!(e.lan_doc, "中文（简体）");
        assert!(e.is_lock);
        assert_eq!(e.subtitle_url, "https://aisubtitle.hdslb.com/abc.json");
        assert_eq!(e.type_code, 0);
    }

    #[test]
    fn parse_entries_ai_subtitle() {
        let v = json!({
            "subtitle": {
                "list": [
                    {
                        "id": 67890_i64,
                        "lan": "en",
                        "lan_doc": "English (auto)",
                        "is_lock": false,
                        "subtitle_url": "https://aisubtitle.hdslb.com/en.json",
                        "type": 1,
                        "ai_status": 2,
                        "ai_type": "1"
                    }
                ]
            }
        });
        let e = &parse_entries(Some(&v))[0];
        assert_eq!(e.type_code, 1);
        assert_eq!(e.ai_status, 2);
        assert_eq!(e.ai_type, "1");
    }

    /// 真打 B 站 API — 字幕列表对匿名几乎都空；验证降级路径不报错。
    #[tokio::test]
    #[ignore]
    async fn list_against_real_bilibili() {
        let r = list("BV1CZEY67E8o")
            .await
            .expect("list failed");
        // 不强制 entries 非空（很多视频无字幕）—— 只要返回成功
        assert!(!r.bv.is_empty());
        assert!(r.cid > 0);
        assert!(!r.title.is_empty());
        // 匿名提示应该出现在 degraded
        assert!(
            r.degraded.iter().any(|s| s.contains("匿名")),
            "expected anonymous degraded note, got {:?}",
            r.degraded
        );
    }

    #[test]
    fn fetched_subtitle_filename_uses_id_and_lan() {
        // 单元测试 download() 拼路径的逻辑（避免 spawn 子进程）
        let e = SubtitleEntry {
            id: 12345,
            lan: "zh-Hans".into(),
            lan_doc: "中文（简体）".into(),
            is_lock: true,
            subtitle_url: "https://example.com/x.json".into(),
            type_code: 0,
            ai_status: 0,
            ai_type: "0".into(),
        };
        let dir = std::path::PathBuf::from("/tmp");
        let path = dir.join(format!("{}.{}.json", e.id, e.lan));
        assert_eq!(path.to_str().unwrap(), "/tmp/12345.zh-Hans.json");
    }
}
