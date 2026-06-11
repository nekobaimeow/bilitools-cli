// SPDX-License-Identifier: GPL-3.0-or-later
// B 站 search API 客户端
// 调 https://api.bilibili.com/x/web-interface/search/type

use crate::error::CliError;
use crate::ipc::shared;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, CliError>;

/// B 站 search_type 字段对应的资源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    /// 单个视频 (bv/av)
    Video,
    /// 番剧 / 影视
    Bangumi,
    /// 直播
    Live,
    /// 专栏
    Article,
    /// 用户
    User,
    /// 音乐
    Audio,
    /// 话题
    Topic,
}

impl SearchType {
    /// B 站 API 期望的 search_type 字符串
    pub fn as_param(&self) -> &'static str {
        match self {
            SearchType::Video => "video",
            SearchType::Bangumi => "media_bangumi",
            SearchType::Live => "live",
            SearchType::Article => "article",
            SearchType::User => "bili_user",
            SearchType::Audio => "audio",
            SearchType::Topic => "topic",
        }
    }
}

impl std::str::FromStr for SearchType {
    type Err = CliError;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "video" | "v" | "videos" => Ok(SearchType::Video),
            "bangumi" | "media_bangumi" | "pgc" => Ok(SearchType::Bangumi),
            "live" => Ok(SearchType::Live),
            "article" | "opus" | "cv" => Ok(SearchType::Article),
            "user" | "bili_user" | "u" => Ok(SearchType::User),
            "audio" | "music" | "au" => Ok(SearchType::Audio),
            "topic" => Ok(SearchType::Topic),
            _ => Err(CliError::Parse(format!("unknown search type: {s}"))),
        }
    }
}

/// 单条搜索结果（视频类型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResult {
    pub bvid: String,
    pub title: String,
    pub author: String,
    pub mid: i64,
    pub duration: String,
    pub duration_sec: i64,
    pub play: i64,
    pub pubdate: i64,
    pub description: String,
    pub pic: String,
    pub typename: String,
    /// 分区 ID；B 站 search entry 不再保证非空，所以用 `Option` 暴露。
    /// 多数 user-facing 场景可 `.unwrap_or(0)` 当作"未知分区"。
    pub tid: Option<i64>,
    pub arcurl: String,
}

/// 搜索响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub keyword: String,
    pub page: u32,
    pub page_size: u32,
    pub total: i64,
    pub results: Vec<VideoResult>,
}

/// 单条 Bangumi 搜索结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BangumiResult {
    pub season_id: i64,
    pub media_id: i64,
    pub title: String,
    pub subtitle: String,
    pub ep_size: i32,
    pub rating: f64,
    pub cover: String,
    pub is_finish: i32,
    pub url: String,
    pub season_type_name: String,
    pub styles: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SearchItem {
    Video(VideoResult),
    Bangumi(BangumiResult),
    User(UserResult),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResult {
    pub mid: i64,
    pub uname: String,
    pub usign: String,
    pub fans: i64,
    pub videos: i64,
    pub level: i32,
    pub avatar: String,
    pub room_id: i64,
}

#[derive(Debug, Default, Deserialize)]
struct RawVideoData {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    pagesize: Option<u32>,
    #[serde(default)]
    numResults: Option<i64>,
    #[serde(default)]
    num_pages: Option<u32>,
    #[serde(default)]
    result: Vec<RawVideoEntry>,
}

#[derive(Debug, Deserialize)]
struct RawVideoEntry {
    bvid: String,
    title: String,
    author: String,
    mid: i64,
    duration: String,
    play: i64,
    pubdate: i64,
    description: String,
    pic: String,
    typename: String,
    // B 站 search entry schema has drifted across 2025-2026: the `tid`
    // and `tag` fields are now sometimes `null` for non-standard uploads,
    // so we mark them as Option to keep the deserialize permissive.
    #[serde(default)]
    tid: Option<i64>,
    arcurl: String,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    like: Option<i64>,
    #[serde(default)]
    danmaku: Option<i64>,
    #[serde(default)]
    reply: Option<i64>,
    #[serde(default)]
    favorite: Option<i64>,
    #[serde(default)]
    coin: Option<i64>,
    #[serde(default)]
    share: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct RawBangumiData {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    pagesize: Option<u32>,
    #[serde(default)]
    numResults: Option<i64>,
    #[serde(default)]
    result: Vec<RawBangumiEntry>,
}

#[derive(Debug, Deserialize)]
struct RawBangumiEntry {
    season_id: i64,
    media_id: i64,
    title: String,
    subtitle: Option<String>,
    #[serde(default)]
    ep_size: Option<i32>,
    #[serde(default)]
    rating: Option<f64>,
    cover: String,
    #[serde(default)]
    is_finish: Option<i32>,
    url: String,
    #[serde(default)]
    season_type_name: Option<String>,
    #[serde(default)]
    styles: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUserData {
    #[serde(default)]
    page: Option<u32>,
    #[serde(default)]
    pagesize: Option<u32>,
    #[serde(default)]
    numResults: Option<i64>,
    #[serde(default)]
    result: Vec<RawUserEntry>,
}

#[derive(Debug, Deserialize)]
struct RawUserEntry {
    mid: i64,
    uname: String,
    #[serde(default)]
    usign: Option<String>,
    #[serde(default)]
    fans: Option<i64>,
    #[serde(default)]
    videos: Option<i64>,
    #[serde(default)]
    level: Option<i32>,
    #[serde(default)]
    upic: Option<String>,
    #[serde(default)]
    room_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawResp<T> {
    code: i64,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

/// 解析 `duration` 字段（"12:34" / "1:23:45"）成秒数
fn parse_duration(s: &str) -> i64 {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => parts[0].parse::<i64>().unwrap_or(0) * 60 + parts[1].parse::<i64>().unwrap_or(0),
        3 => parts[0].parse::<i64>().unwrap_or(0) * 3600
            + parts[1].parse::<i64>().unwrap_or(0) * 60
            + parts[2].parse::<i64>().unwrap_or(0),
        _ => s.parse().unwrap_or(0),
    }
}

/// 去除 B 站搜索结果中的 HTML 高亮标签
///
/// B 站 search API 在 2025-2026 之后直接返回 raw HTML 标签
/// `<em class="keyword">keyword</em>`（不再二次转义为 `&lt;em…&gt;`）。
/// 我们同时处理两种形态以保持向前兼容：旧的 `&lt;em…&gt;` 实体
/// （例如历史缓存或某些番剧接口）以及新的 raw `<em…>` 标签。
fn strip_em(s: &str) -> String {
    let s = s.replace("&lt;em class=\"keyword\"&gt;", "");
    let s = s.replace("&lt;/em&gt;", "");
    let s = s.replace("<em class=\"keyword\">", "");
    let s = s.replace("</em>", "");
    let s = s.replace("&amp;", "&");
    let s = s.replace("&quot;", "\"");
    s
}

/// 百分号编码
fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn get_json<T>(url_base: &str, params: &BTreeMap<String, String>) -> Result<RawResp<T>>
where
    T: for<'de> Deserialize<'de> + Default,
{
    // 搜索接口是 WBI-protected；签名失败时降级到无签名
    let url = match shared::wbi_sign(params).await {
        Ok((q, w_rid)) => format!("{url_base}?{q}&w_rid={w_rid}"),
        Err(e) => {
            // 不带签名也试一次
            let fallback: String = params
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            format!("{url_base}?{fallback}")
        }
    };
    let client = shared::init_client()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;
    // Disable reqwest's automatic gzip/deflate/br decompression for this
    // request. B 站 search returns chunked responses with mixed/identity
    // Content-Encoding that trip up reqwest's streaming decoder, surfacing
    // as "error decoding response body" before the JSON ever reaches
    // serde. Asking for identity lets us read the raw JSON bytes
    // ourselves.
    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(CliError::from)?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>")
        .to_string();
    let content_encoding = resp
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>")
        .to_string();
    let body = resp.bytes().await.map_err(CliError::from)?;
    tracing::debug!(
        "search http {} content-type={} content-encoding={} body_len={} body[0..120]={:?}",
        status,
        content_type,
        content_encoding,
        body.len(),
        String::from_utf8_lossy(&body[..body.len().min(120)])
    );
    if !status.is_success() {
        return Err(CliError::Http {
            status: status.as_u16(),
            message: format!("http {} for {}: {:?}", status, url_base, String::from_utf8_lossy(&body)),
        });
    }
    let raw: RawResp<T> = serde_json::from_slice(&body).map_err(|e| {
        CliError::Parse(format!(
            "json decode failed: {e}; body[0..200]={:?}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        ))
    })?;
    if raw.code != 0 {
        return Err(CliError::Api {
            code: raw.code,
            message: raw.message.unwrap_or_default(),
        });
    }
    Ok(raw)
}

/// 搜索视频
pub async fn search_videos(keyword: &str, page: u32, page_size: u32) -> Result<SearchResults> {
    if keyword.is_empty() {
        return Err(CliError::Parse("empty keyword".into()));
    }
    let url_base = "https://api.bilibili.com/x/web-interface/search/type";
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("search_type".into(), SearchType::Video.as_param().into());
    params.insert("keyword".into(), keyword.into());
    params.insert("page".into(), page.to_string());
    params.insert("pagesize".into(), page_size.to_string());
    let raw = get_json::<RawVideoData>(url_base, &params).await?;
    let data = raw.data.unwrap_or(RawVideoData {
        page: Some(1),
        pagesize: Some(page_size),
        numResults: Some(0),
        num_pages: Some(1),
        result: vec![],
    });
    let results = data
        .result
        .into_iter()
        .map(|e| VideoResult {
            bvid: e.bvid,
            title: strip_em(&e.title),
            author: e.author,
            mid: e.mid,
            duration: e.duration.clone(),
            duration_sec: parse_duration(&e.duration),
            play: e.play,
            pubdate: e.pubdate,
            description: strip_em(&e.description),
            pic: e.pic,
            typename: e.typename,
            tid: e.tid,
            arcurl: e.arcurl,
        })
        .collect();
    Ok(SearchResults {
        keyword: keyword.to_string(),
        page: data.page.unwrap_or(page),
        page_size: data.pagesize.unwrap_or(page_size),
        total: data.numResults.unwrap_or(0),
        results,
    })
}

/// 搜索番剧
pub async fn search_bangumi(
    keyword: &str,
    page: u32,
    page_size: u32,
) -> Result<Vec<BangumiResult>> {
    if keyword.is_empty() {
        return Err(CliError::Parse("empty keyword".into()));
    }
    let url_base = "https://api.bilibili.com/x/web-interface/search/type";
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("search_type".into(), SearchType::Bangumi.as_param().into());
    params.insert("keyword".into(), keyword.into());
    params.insert("page".into(), page.to_string());
    params.insert("pagesize".into(), page_size.to_string());
    let raw = get_json::<RawBangumiData>(url_base, &params).await?;
    let data = raw.data.unwrap_or(RawBangumiData {
        page: Some(1),
        pagesize: Some(page_size),
        numResults: Some(0),
        result: vec![],
    });
    Ok(data
        .result
        .into_iter()
        .map(|e| BangumiResult {
            season_id: e.season_id,
            media_id: e.media_id,
            title: strip_em(&e.title),
            subtitle: strip_em(&e.subtitle.unwrap_or_default()),
            ep_size: e.ep_size.unwrap_or(0),
            rating: e.rating.unwrap_or(0.0),
            cover: e.cover,
            is_finish: e.is_finish.unwrap_or(0),
            url: e.url,
            season_type_name: e.season_type_name.unwrap_or_default(),
            styles: e.styles.unwrap_or_default(),
        })
        .collect())
}

/// 搜索用户
pub async fn search_users(
    keyword: &str,
    page: u32,
    page_size: u32,
) -> Result<Vec<UserResult>> {
    if keyword.is_empty() {
        return Err(CliError::Parse("empty keyword".into()));
    }
    let url_base = "https://api.bilibili.com/x/web-interface/search/type";
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("search_type".into(), SearchType::User.as_param().into());
    params.insert("keyword".into(), keyword.into());
    params.insert("page".into(), page.to_string());
    params.insert("pagesize".into(), page_size.to_string());
    let raw = get_json::<RawUserData>(url_base, &params).await?;
    let data = raw.data.unwrap_or(RawUserData {
        page: Some(1),
        pagesize: Some(page_size),
        numResults: Some(0),
        result: vec![],
    });
    Ok(data
        .result
        .into_iter()
        .map(|e| UserResult {
            mid: e.mid,
            uname: strip_em(&e.uname),
            usign: strip_em(&e.usign.unwrap_or_default()),
            fans: e.fans.unwrap_or(0),
            videos: e.videos.unwrap_or(0),
            level: e.level.unwrap_or(0),
            avatar: e.upic.unwrap_or_default(),
            room_id: e.room_id.unwrap_or(0),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_minutes_seconds() {
        assert_eq!(parse_duration("12:34"), 12 * 60 + 34);
        assert_eq!(parse_duration("1:23:45"), 3600 + 23 * 60 + 45);
        assert_eq!(parse_duration("0:00"), 0);
        assert_eq!(parse_duration("42"), 42);
    }

    #[test]
    fn strip_em_removes_highlight() {
        assert_eq!(
            strip_em("&lt;em class=\"keyword\"&gt;原神&lt;/em&gt;角色预告"),
            "原神角色预告"
        );
    }

    #[test]
    fn strip_em_handles_raw_html_tags() {
        // B 站 search API since 2025 ships raw HTML in titles, not the
        // double-escaped `&lt;em…&gt;` entities.
        assert_eq!(
            strip_em("《<em class=\"keyword\">原神</em>》"),
            "《原神》"
        );
        assert_eq!(
            strip_em("<em class=\"keyword\">keyword</em> 标题"),
            "keyword 标题"
        );
    }

    #[test]
    fn strip_em_decodes_entities() {
        assert_eq!(strip_em("normal text &amp; here"), "normal text & here");
        assert_eq!(strip_em("&quot;hi&quot;"), "\"hi\"");
    }

    #[test]
    fn search_type_param() {
        assert_eq!(SearchType::Video.as_param(), "video");
        assert_eq!(SearchType::Bangumi.as_param(), "media_bangumi");
        assert_eq!(SearchType::User.as_param(), "bili_user");
    }

    #[test]
    fn search_type_from_str() {
        assert_eq!("video".parse::<SearchType>().unwrap(), SearchType::Video);
        assert_eq!("bangumi".parse::<SearchType>().unwrap(), SearchType::Bangumi);
        assert_eq!("PGC".parse::<SearchType>().unwrap(), SearchType::Bangumi);
        assert_eq!("user".parse::<SearchType>().unwrap(), SearchType::User);
        assert_eq!("opus".parse::<SearchType>().unwrap(), SearchType::Article);
        assert!("junk".parse::<SearchType>().is_err());
    }

    #[test]
    fn encode_handles_chinese() {
        assert_eq!(encode("原神"), "%E5%8E%9F%E7%A5%9E");
        assert_eq!(encode("BV1 abc"), "BV1+abc");
    }

    #[test]
    fn video_result_parses_raw() {
        // 模拟 raw entry → 转换
        let raw = RawVideoEntry {
            bvid: "BV1abc".into(),
            title: "&lt;em class=\"keyword\"&gt;原神&lt;/em&gt;".into(),
            author: "官方".into(),
            mid: 123,
            duration: "3:45".into(),
            play: 1000,
            pubdate: 1234567,
            description: "".into(),
            pic: "".into(),
            typename: "游戏".into(),
            tid: Some(33),
            arcurl: "https://www.bilibili.com/video/BV1abc".into(),
            tag: None,
            like: None,
            danmaku: None,
            reply: None,
            favorite: None,
            coin: None,
            share: None,
        };
        let v = VideoResult {
            bvid: raw.bvid,
            title: strip_em(&raw.title),
            author: raw.author,
            mid: raw.mid,
            duration: raw.duration.clone(),
            duration_sec: parse_duration(&raw.duration),
            play: raw.play,
            pubdate: raw.pubdate,
            description: strip_em(&raw.description),
            pic: raw.pic,
            typename: raw.typename,
            tid: raw.tid,
            arcurl: raw.arcurl,
        };
        assert_eq!(v.title, "原神");
        assert_eq!(v.duration_sec, 225);
    }

    #[test]
    fn search_type_video_bangumi_user_param_distinct() {
        // 确认 video / bangumi / user 的 search_type 字符串互不相同
        assert_ne!(
            SearchType::Video.as_param(),
            SearchType::Bangumi.as_param()
        );
        assert_ne!(SearchType::User.as_param(), SearchType::Video.as_param());
    }

    #[test]
    fn search_params_contain_required_keys() {
        // 确认构建 params 时所有键都填了
        let mut params: BTreeMap<String, String> = BTreeMap::new();
        params.insert("search_type".into(), "video".into());
        params.insert("keyword".into(), "原神".into());
        params.insert("page".into(), "1".into());
        params.insert("pagesize".into(), "20".into());
        for k in ["search_type", "keyword", "page", "pagesize"] {
            assert!(params.contains_key(k), "missing key: {k}");
        }
    }

    #[test]
    fn classify_kind_from_arcurl_video() {
        // Regular /video/BVxxx URL → "video", no ssid.
        let r = VideoResult {
            bvid: Some("BV1abc".into()),
            ssid: None,
            kind: "video",
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl: "https://www.bilibili.com/video/BV1abc".into(),
        };
        assert_eq!(r.kind, "video");
        assert_eq!(r.ssid, None);
    }

    #[test]
    fn classify_kind_from_arcurl_cheese() {
        // /cheese/play/ss{N} URL → "cheese", ssid == "N".
        let r = VideoResult {
            bvid: None,
            ssid: Some("959815180".into()),
            kind: "cheese",
            title: String::new(),
            author: String::new(),
            mid: 0,
            duration: String::new(),
            duration_sec: 0,
            play: 0,
            pubdate: 0,
            description: String::new(),
            pic: String::new(),
            typename: String::new(),
            tid: None,
            arcurl:
                "https://www.bilibili.com/cheese/play/ss959815180?query_from=0".into(),
        };
        assert_eq!(r.kind, "cheese");
        assert_eq!(r.ssid.as_deref(), Some("959815180"));
    }
}
