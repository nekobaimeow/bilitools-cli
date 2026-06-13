// SPDX-License-Identifier: GPL-3.0-or-later
// B 站 high-level API — describe resources from the parser into a
// downloadable manifest. This is the network-aware layer that the
// `parse` subcommand exposes to the user.

use crate::error::CliError;
use crate::ipc::media::{ResourceKind, ResourceRef};
use crate::ipc::shared::init_client;
use serde::{Deserialize, Serialize};

/// A flat description of a downloadable item: title, cover URL, the
/// list of available streams, etc. Returned by `describe_resource`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceDescription {
    pub kind: ResourceKind,
    pub id: String,
    pub title: String,
    pub cover: Option<String>,
    pub duration: Option<f64>,
    pub pages: Vec<PageInfo>,
    /// Numeric aid (for BV inputs, decoded from the view response).
    /// Used by the playurl API which only accepts numeric aids.
    pub aid: Option<i64>,
    /// Sub-resources (e.g. episodes of a season).
    pub children: Vec<ResourceDescription>,
    /// Raw upstream JSON, for callers that need more.
    pub raw: serde_json::Value,
    /// Video description (B 站简介). `None` when unavailable.
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    pub cid: i64,
    pub title: String,
    pub duration: Option<f64>,
    pub part: Option<String>,
}

#[derive(Deserialize)]
struct ViewApiResponse {
    code: i64,
    message: String,
    data: Option<ViewData>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ViewData {
    title: String,
    pic: Option<String>,
    duration: Option<f64>,
    pages: Option<Vec<ViewPage>>,
    aid: Option<i64>,
    bvid: Option<String>,
    desc: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ViewPage {
    cid: i64,
    part: Option<String>,
    title: Option<String>,
    duration: Option<i64>,
}

#[derive(Deserialize)]
struct SeasonApiResponse {
    code: i64,
    message: String,
    result: Option<SeasonResult>,
}

#[derive(Deserialize)]
struct SeasonResult {
    title: String,
    cover: Option<String>,
    episodes: Vec<SeasonEpisode>,
}

#[derive(Deserialize)]
struct SeasonEpisode {
    id: i64,
    cid: i64,
    title: String,
    duration: Option<i64>,
    long_title: Option<String>,
}

/// Fetch a resource description from the B 站 web API.
///
/// This issues the same calls the GUI version does, normalized into
/// a single `ResourceDescription` for the CLI's `parse` subcommand.
pub async fn describe(res: &ResourceRef) -> Result<ResourceDescription, CliError> {
    match res.kind {
        ResourceKind::Video => describe_video(&res.id).await,
        ResourceKind::Bangumi | ResourceKind::Cheese => describe_season(&res.id).await,
        ResourceKind::Episode => describe_episode(&res.id).await,
        ResourceKind::Favorite => describe_favorite(&res.id).await,
        ResourceKind::WatchLater => describe_watch_later().await,
        ResourceKind::User => describe_user(&res.id).await,
        ResourceKind::Audio => describe_audio(&res.id).await,
        ResourceKind::Collection => describe_collection(&res.id).await,
        ResourceKind::Short | ResourceKind::Live | ResourceKind::Interactive | ResourceKind::Unknown => {
            Err(CliError::msg(format!(
                "resource kind '{}' is not yet supported by `describe`",
                res.kind.as_str()
            )))
        }
    }
}

async fn describe_video(id: &str) -> Result<ResourceDescription, CliError> {
    // Accept both BV... and av...
    let client = init_client().await?;
    let resp = if let Some(bv) = id.strip_prefix("BV") {
        client
            .get("https://api.bilibili.com/x/web-interface/view")
            .query(&[("bvid", bv)])
            .send()
            .await?
    } else if let Some(rest) = id.strip_prefix("av") {
        client
            .get("https://api.bilibili.com/x/web-interface/view")
            .query(&[("aid", rest)])
            .send()
            .await?
    } else {
        return Err(CliError::InvalidUrl(format!("not a video id: {id}")));
    };
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("view api failed")));
    }
    let body: ViewApiResponse = resp.json().await?;
    if body.code != 0 {
        return Err(CliError::api(body.code, body.message));
    }
    let data = body.data.ok_or_else(|| CliError::msg("view api: no data"))?;
    let pages = data
        .pages
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|p| PageInfo {
            cid: p.cid,
            title: p.title.unwrap_or_default(),
            duration: p.duration.map(|d| d as f64),
            part: p.part,
        })
        .collect();
    let raw = serde_json::to_value(&data).unwrap_or(serde_json::Value::Null);
    Ok(ResourceDescription {
        kind: ResourceKind::Video,
        id: id.to_string(),
        title: data.title,
        cover: data.pic,
        duration: data.duration,
        pages,
        aid: data.aid,
        children: Vec::new(),
        description: data.desc,
        raw,
    })
}

async fn describe_season(id: &str) -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/pugv/view/web/season")
        .query(&[("season_id", String::from(id.trim_start_matches("ss")))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("season api failed")));
    }
    let body: SeasonApiResponse = resp.json().await?;
    let result = body.result.ok_or_else(|| CliError::msg("season api: no result"))?;
    let children = result
        .episodes
        .into_iter()
        .map(|e| ResourceDescription {
            kind: ResourceKind::Episode,
            id: format!("ep{}", e.id),
            title: e.title,
            cover: None,
            duration: e.duration.map(|d| d as f64),
            pages: vec![PageInfo {
                cid: e.cid,
                title: e.long_title.unwrap_or_default(),
                duration: e.duration.map(|d| d as f64),
                part: None,
            }],
            aid: None,

            children: Vec::new(),
            description: None,
            raw: serde_json::Value::Null,
        })
        .collect();
    Ok(ResourceDescription {
        kind: ResourceKind::Bangumi,
        id: id.to_string(),
        title: result.title,
        cover: result.cover,
        duration: None,
        pages: Vec::new(),
        aid: None,
        children,
        description: None,
        raw: serde_json::Value::Null,
    })
}

async fn describe_episode(id: &str) -> Result<ResourceDescription, CliError> {
    // Treat an episode as a single video and route to describe_video
    // after looking up the bvid.
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/pugv/view/web/episode")
        .query(&[("ep_id", String::from(id.trim_start_matches("ep")))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("episode api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    let bvid = body
        .get("data")
        .and_then(|d| d.get("bvid"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::msg("episode api: no bvid"))?;
    describe_video(bvid).await.map(|mut d| {
        d.id = id.to_string();
        d.kind = ResourceKind::Episode;
        d
    })
}

async fn describe_favorite(fid: &str) -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/x/v3/fav/folder/created/list-all")
        .query(&[("up_mid", String::from(fid))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("favorite api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(ResourceDescription {
        kind: ResourceKind::Favorite,
        id: fid.to_string(),
        title: format!("Favorite folder {fid}"),
        cover: None,
        duration: None,
        pages: Vec::new(),
        aid: None,

        children: Vec::new(),
        description: None,
        raw: body,
    })
}

async fn describe_watch_later() -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/x/v2/history/toview")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("watchlater api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(ResourceDescription {
        kind: ResourceKind::WatchLater,
        id: String::new(),
        title: "Watch later".to_string(),
        cover: None,
        duration: None,
        pages: Vec::new(),
        aid: None,

        children: Vec::new(),
        description: None,
        raw: body,
    })
}

async fn describe_user(mid: &str) -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/x/space/acc/info")
        .query(&[("mid", String::from(mid))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("user api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    let name = body
        .get("data")
        .and_then(|d| d.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("User")
        .to_string();
    Ok(ResourceDescription {
        kind: ResourceKind::User,
        id: mid.to_string(),
        title: name,
        cover: None,
        duration: None,
        pages: Vec::new(),
        aid: None,

        children: Vec::new(),
        description: None,
        raw: body,
    })
}

async fn describe_audio(auid: &str) -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://www.bilibili.com/audio/music-service-c/web/song/info")
        .query(&[("sid", String::from(auid.trim_start_matches("au")))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("audio api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    let title = body
        .get("data")
        .and_then(|d| d.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or(auid)
        .to_string();
    Ok(ResourceDescription {
        kind: ResourceKind::Audio,
        id: auid.to_string(),
        title,
        cover: None,
        duration: None,
        pages: Vec::new(),
        aid: None,

        children: Vec::new(),
        description: None,
        raw: body,
    })
}

async fn describe_collection(lid: &str) -> Result<ResourceDescription, CliError> {
    let client = init_client().await?;
    let resp = client
        .get("https://api.bilibili.com/x/polymer/web-space/seasons_archives_list")
        .query(&[("mid", String::from(lid.trim_start_matches("lid")))])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(CliError::http(resp.status().as_u16(), String::from("collection api failed")));
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(ResourceDescription {
        kind: ResourceKind::Collection,
        id: lid.to_string(),
        title: format!("Collection {lid}"),
        cover: None,
        duration: None,
        pages: Vec::new(),
        aid: None,

        children: Vec::new(),
        description: None,
        raw: body,
    })
}

// =====================  Tests  =====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_info_default_constructs() {
        let p = PageInfo {
            cid: 12345,
            title: "p1".into(),
            duration: Some(120.0),
            part: Some("Part 1".into()),
        };
        assert_eq!(p.cid, 12345);
    }

    #[test]
    fn resource_description_default_constructs() {
        let d = ResourceDescription {
            kind: ResourceKind::Video,
            id: "BV1".into(),
            title: "t".into(),
            cover: None,
            duration: None,
            pages: vec![],
            aid: None,
            children: vec![],
            description: None,
            raw: serde_json::Value::Null,
        };
        assert_eq!(d.kind, ResourceKind::Video);
    }

    #[test]
    fn view_api_response_deserializes() {
        let s = r#"{"code":0,"message":"0","data":null}"#;
        let v: ViewApiResponse = serde_json::from_str(s).unwrap();
        assert_eq!(v.code, 0);
        assert!(v.data.is_none());
    }
}
