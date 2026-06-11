// SPDX-License-Identifier: GPL-3.0-or-later
// 弹幕下载 + XML/ASS 转换
//
// 工作流（与 GUI 原版一致）：
//   1. 拿 cid（from BV/av by web-interface/view）
//   2. 拉 https://comment.bilibili.com/{cid}.xml  -- 实时弹幕
//   3. （可选）调 DanmakuFactory sidecar 把 XML 转 ASS
//
// 降级策略：
//   - 没装 DanmakuFactory → 只拉 XML 直存
//   - 网络拉不到 → 返回错误信息

use crate::backends::sidecar::SidecarKind;
use crate::error::CliError;
use crate::ipc::shared::{init_client, HEADERS};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub type Result<T> = std::result::Result<T, CliError>;

/// 弹幕源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DanmakuSource {
    /// 只下实时弹幕
    Live,
    /// 只下历史弹幕（需要登录态 + protobuf 解析）
    History,
    /// 都下（实时 + 历史合并）
    Both,
}

impl Default for DanmakuSource {
    fn default() -> Self {
        DanmakuSource::Live
    }
}

impl std::str::FromStr for DanmakuSource {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "live" | "l" => Ok(Self::Live),
            "history" | "h" | "hist" => Ok(Self::History),
            "both" | "all" | "b" => Ok(Self::Both),
            _ => Err(CliError::Parse(format!("unknown danmaku source: {s}"))),
        }
    }
}

/// 弹幕任务输出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DanmakuFormat {
    /// 原始 B 站 XML
    Xml,
    /// 转换后的 ASS 字幕文件
    Ass,
    /// 两个都生成
    #[default]
    Both,
}

impl std::str::FromStr for DanmakuFormat {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "xml" | "raw" => Ok(Self::Xml),
            "ass" | "subtitle" => Ok(Self::Ass),
            "both" | "all" => Ok(Self::Both),
            _ => Err(CliError::Parse(format!("unknown danmaku format: {s}"))),
        }
    }
}

/// 弹幕下载 + 转换结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DanmakuResult {
    pub title: String,
    pub cid: i64,
    pub live_count: usize,
    pub history_count: usize,
    pub xml_path: Option<PathBuf>,
    pub ass_path: Option<PathBuf>,
    pub danmakufactory_used: bool,
    pub degraded: Vec<String>,
}

/// 拉取实时弹幕 XML
async fn fetch_live_xml(cid: i64) -> Result<Vec<u8>> {
    let url = format!("https://comment.bilibili.com/{cid}.xml");
    let client = init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("live danmaku http for cid={cid}"),
        });
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(CliError::from)
}

/// 解析 `<d p="time,type,size,color,date,pool,user,id">text</d>` 计数
fn count_danmaku(xml: &[u8]) -> usize {
    let s = String::from_utf8_lossy(xml);
    s.matches("<d p=").count()
}

/// 从单个 B 站 XML 文档里抽 `<d ...>...</d>` 节点
fn extract_d_nodes(xml: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<d ") {
            // 找匹配的 </d>
            if let Some(end) = xml[i..].find("</d>") {
                let s = &xml[i..i + end + 4];
                out.push(s);
                i += end + 4;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 合并多个 XML
fn merge_xml(live: Option<Vec<u8>>, history: Option<Vec<u8>>) -> Vec<u8> {
    let mut doc = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><i><chatserver>chat.bilibili.com</chatserver><chatid>0</chatid><mission>0</mission><maxlimit>1500</maxlimit><state>0</state><real_name>0</real_name><source>k-v</source>"#,
    );
    for src in [&live, &history] {
        if let Some(b) = src {
            let s = String::from_utf8_lossy(b);
            for d in extract_d_nodes(&s) {
                doc.push_str(d);
            }
        }
    }
    doc.push_str("</i>");
    doc.into_bytes()
}

/// 调 DanmakuFactory 转 ASS
async fn run_danmakufactory(xml_path: &Path, ass_path: &Path) -> Result<()> {
    let exe = crate::backends::sidecar::resolve(SidecarKind::DanmakuFactory, None)
        .map_err(|e| CliError::Other(format!("DanmakuFactory not installed: {e}")))?;
    let status = tokio::process::Command::new(&exe)
        .arg("-i")
        .arg(xml_path)
        .arg("-o")
        .arg(ass_path)
        .arg("--ignore-warnings")
        .arg("-c")
        .arg("/dev/null")
        .status()
        .await
        .map_err(|e| CliError::Other(format!("spawn DanmakuFactory: {e}")))?;
    if !status.success() {
        return Err(CliError::Other(format!(
            "DanmakuFactory exited with {}",
            status
        )));
    }
    Ok(())
}

/// 从 BV/URL/av 拿到 (title, aid, cid)
pub async fn resolve_cid(input: &str) -> Result<(String, i64, i64)> {
    let bv = extract_bvid(input).ok_or_else(|| CliError::Parse(format!("no BV id in '{input}'")))?;
    let view = fetch_view(&bv).await?;
    let aid = view
        .aid
        .ok_or_else(|| CliError::Parse("no aid in view".into()))?;
    Ok((view.title, aid, view.cid))
}

/// 从 URL / 裸 ID 提取 BV 号（统一输出大写）
pub fn extract_bvid(input: &str) -> Option<String> {
    let lower = input.to_ascii_lowercase();
    if let Some(start) = lower.find("bv1") {
        let candidate = &input[start..];
        if candidate.len() >= 12 {
            let bvid = &candidate[..12];
            if bvid[2..].chars().all(|c| c.is_ascii_alphanumeric()) {
                // 把前两位 bv → BV
                let mut out = String::with_capacity(12);
                out.push_str("BV");
                out.push_str(&bvid[2..]);
                return Some(out);
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct ViewApiResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<ViewData>,
}

#[derive(Debug, Deserialize)]
struct ViewData {
    title: String,
    cid: i64,
    aid: Option<i64>,
}

/// 直调 web-interface/view 拿 title/cid/aid
pub async fn fetch_view(bvid: &str) -> Result<ViewData> {
    let url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");
    let client = init_client().await.map_err(|e| CliError::Other(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(CliError::from)?;
    if !resp.status().is_success() {
        return Err(CliError::Http {
            status: resp.status().as_u16(),
            message: format!("view http {}", resp.status()),
        });
    }
    let body: ViewApiResponse = resp.json().await.map_err(CliError::from)?;
    if body.code != 0 {
        return Err(CliError::Api {
            code: body.code as i64,
            message: body.message,
        });
    }
    body.data.ok_or_else(|| CliError::Parse("view empty data".into()))
}

/// 拉弹幕 + 转 ASS（顶层 API）
pub async fn fetch_and_convert(
    input: &str,
    output_dir: &Path,
    source: DanmakuSource,
    format: DanmakuFormat,
) -> Result<DanmakuResult> {
    let (title, _aid, cid) = resolve_cid(input).await?;
    fs::create_dir_all(output_dir).await.map_err(CliError::from)?;

    let mut degraded = Vec::new();
    let mut live_count = 0;
    let mut history_count = 0;
    let live_bytes = if matches!(source, DanmakuSource::Live | DanmakuSource::Both) {
        match fetch_live_xml(cid).await {
            Ok(b) => {
                live_count = count_danmaku(&b);
                Some(b)
            }
            Err(e) => {
                degraded.push(format!("live fetch failed: {e}"));
                None
            }
        }
    } else {
        None
    };
    let history_bytes = if matches!(source, DanmakuSource::History | DanmakuSource::Both) {
        degraded.push("history danmaku requires protobuf parser (not yet in CLI)".into());
        None
    } else {
        None
    };

    let merged = merge_xml(live_bytes, history_bytes);
    let xml_path = output_dir.join(format!("{cid}.xml"));
    let ass_path = output_dir.join(format!("{cid}.ass"));

    let mut xml_written = false;
    let mut ass_written: Option<PathBuf> = None;
    let mut df_used = false;

    if matches!(format, DanmakuFormat::Xml | DanmakuFormat::Both) {
        let mut f = fs::File::create(&xml_path).await.map_err(CliError::from)?;
        f.write_all(&merged).await.map_err(CliError::from)?;
        f.flush().await.map_err(CliError::from)?;
        xml_written = true;
    }

    if matches!(format, DanmakuFormat::Ass | DanmakuFormat::Both) {
        let tmp_xml = if xml_written {
            xml_path.clone()
        } else {
            let tmp = output_dir.join(format!("{cid}.input.xml"));
            let mut f = fs::File::create(&tmp).await.map_err(CliError::from)?;
            f.write_all(&merged).await.map_err(CliError::from)?;
            f.flush().await.map_err(CliError::from)?;
            tmp
        };
        match run_danmakufactory(&tmp_xml, &ass_path).await {
            Ok(_) => {
                df_used = true;
                ass_written = Some(ass_path.clone());
                if !xml_written {
                    let _ = fs::remove_file(&tmp_xml).await;
                }
            }
            Err(e) => {
                degraded.push(format!("DanmakuFactory failed: {e}"));
                if !xml_written {
                    let _ = fs::remove_file(&tmp_xml).await;
                }
            }
        }
    }

    if history_count == 0 {
        history_count = count_danmaku(&merged).saturating_sub(live_count);
    }

    Ok(DanmakuResult {
        title,
        cid,
        live_count,
        history_count,
        xml_path: if xml_written { Some(xml_path) } else { None },
        ass_path: ass_written,
        danmakufactory_used: df_used,
        degraded,
    })
}

/// HEADERS 是否持有有效登录 cookie（仅用于提示）
pub async fn has_login_cookie() -> bool {
    HEADERS.cookie().await.contains("SESSDATA=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bvid_bare() {
        assert_eq!(
            extract_bvid("BV1ZvEt6oEWR"),
            Some("BV1ZvEt6oEWR".to_string())
        );
    }

    #[test]
    fn extract_bvid_in_url() {
        assert_eq!(
            extract_bvid("https://www.bilibili.com/video/BV1ZvEt6oEWR?p=2"),
            Some("BV1ZvEt6oEWR".to_string())
        );
    }

    #[test]
    fn extract_bvid_lowercase() {
        assert_eq!(
            extract_bvid("bv1zvet6oewr"),
            Some("BV1zvet6oewr".to_string())
        );
    }

    #[test]
    fn extract_bvid_none() {
        assert_eq!(extract_bvid("https://example.com"), None);
    }

    #[test]
    fn extract_bvid_too_short() {
        assert_eq!(extract_bvid("BV1abc"), None);
    }

    #[test]
    fn count_danmaku_simple() {
        let xml = b"<?xml version=\"1.0\"?><i><d p=\"1,1,25,16777215,1,1,1,1\">hi</d><d p=\"2,1,25,16777215,1,1,1,2\">yo</d></i>";
        assert_eq!(count_danmaku(xml), 2);
        assert_eq!(count_danmaku(b"empty"), 0);
    }

    #[test]
    fn extract_d_nodes_basic() {
        let xml = r#"<?xml version="1.0"?><i><chatserver>x</chatserver><d p="1,1,25,1,1,1,1,1">hi</d><d p="2,1,25,1,1,1,1,2">yo</d></i>"#;
        let nodes = extract_d_nodes(xml);
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].contains("hi"));
        assert!(nodes[1].contains("yo"));
    }

    #[test]
    fn merge_xml_combines_both() {
        let live = br#"<?xml version="1.0"?><i><d p="1,1,25,1,1,1,1,1">live1</d></i>"#;
        let merged = merge_xml(Some(live.to_vec()), None);
        let s = String::from_utf8_lossy(&merged);
        assert!(s.contains("<i>"));
        assert!(s.contains("</i>"));
        assert!(s.contains("live1"));
    }

    #[test]
    fn merge_xml_no_inputs() {
        let merged = merge_xml(None, None);
        let s = String::from_utf8_lossy(&merged);
        assert!(s.contains("<i>"));
        assert!(s.contains("</i>"));
    }

    #[test]
    fn danmaku_source_from_str() {
        assert_eq!("live".parse::<DanmakuSource>().unwrap(), DanmakuSource::Live);
        assert_eq!("h".parse::<DanmakuSource>().unwrap(), DanmakuSource::History);
        assert_eq!("both".parse::<DanmakuSource>().unwrap(), DanmakuSource::Both);
        assert!("junk".parse::<DanmakuSource>().is_err());
    }

    #[test]
    fn danmaku_format_from_str() {
        assert_eq!("xml".parse::<DanmakuFormat>().unwrap(), DanmakuFormat::Xml);
        assert_eq!("ass".parse::<DanmakuFormat>().unwrap(), DanmakuFormat::Ass);
        assert_eq!("both".parse::<DanmakuFormat>().unwrap(), DanmakuFormat::Both);
    }
}
