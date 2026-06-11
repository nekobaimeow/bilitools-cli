// SPDX-License-Identifier: GPL-3.0-or-later
// `search` subcommand.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::search::{self, SearchType, VideoResult};

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Search {
        keyword,
        r#type,
        page,
        page_size,
        limit,
    } = cmd
    else {
        return Err(CliError::Other("internal: not a Search command".into()));
    };
    let search_type: SearchType = r#type.parse().map_err(|e: CliError| e)?;
    let page = *page;
    let page_size = *page_size;

    match search_type {
        SearchType::Video => {
            let results = search::search_videos(keyword, page, page_size).await?;
            if out.is_json() {
                out.ok(serde_json::json!({
                    "keyword": results.keyword,
                    "page": results.page,
                    "page_size": results.page_size,
                    "total": results.total,
                    "results": results.results,
                }))?;
            } else {
                print_video_table(&results.results, *limit, &out);
            }
        }
        SearchType::Bangumi => {
            let results = search::search_bangumi(keyword, page, page_size).await?;
            if out.is_json() {
                out.ok(serde_json::json!({
                    "keyword": keyword,
                    "kind": "bangumi",
                    "results": results,
                }))?;
            } else {
                for r in results.iter().take(limit.unwrap_or(20) as usize) {
                    out.status(&format!(
                        "ss={}  rating={:.1}  eps={}  {}",
                        r.season_id, r.rating, r.ep_size, r.title
                    ));
                }
            }
        }
        SearchType::User => {
            let results = search::search_users(keyword, page, page_size).await?;
            if out.is_json() {
                out.ok(serde_json::json!({
                    "keyword": keyword,
                    "kind": "user",
                    "results": results,
                }))?;
            } else {
                for r in results.iter().take(limit.unwrap_or(20) as usize) {
                    out.status(&format!(
                        "uid={}  fans={}  videos={}  {}",
                        r.mid, r.fans, r.videos, r.uname
                    ));
                }
            }
        }
        _ => {
            return Err(CliError::Other(format!(
                "search type {:?} not yet implemented in CLI",
                search_type
            )))
        }
    }
    Ok(())
}

/// Render the leftmost ID column of a search result row.
///
/// For regular videos we show the BV id. For cheese courses we
/// synthesize `cheese:ss{N}` so the user can see at a glance that
/// this is a course page (which is not directly downloadable via
/// the BV-style playurl endpoint).
fn render_id_column(r: &VideoResult) -> String {
    match (r.kind.as_str(), r.bvid.as_deref(), r.ssid.as_deref()) {
        ("cheese", _, Some(ss)) => format!("cheese:ss{}", ss),
        (_, Some(bv), _) => bv.to_string(),
        // Fallback: classify_kind is meant to guarantee one or the
        // other, but if B 站 ever ships a row with neither we still
        // want a stable 14-char display.
        _ => "-".to_string(),
    }
}

fn print_video_table(results: &[VideoResult], limit: Option<u32>, out: &Output) {
    let n = limit.unwrap_or(20) as usize;
    if results.is_empty() {
        out.status("(no results)");
        return;
    }
    out.status(&format!(
        "{:<14} {:<7} {:<8} {:<14} TITLE",
        "BVID", "DUR", "PLAY", "AUTHOR"
    ));
    for r in results.iter().take(n) {
        let dur = format_duration(r.duration_sec);
        let id = render_id_column(r);
        out.status(&format!(
            "{:<14} {:<7} {:<8} {:<14} {}",
            id, dur, format_count(r.play), truncate(&r.author, 13), truncate(&r.title, 60)
        ));
    }
    if results.len() > n {
        out.status(&format!("(showing {} of {})", n, results.len()));
    }
}

fn format_duration(sec: i64) -> String {
    let h = sec / 3600;
    let m = (sec % 3600) / 60;
    let s = sec % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

fn format_count(n: i64) -> String {
    if n >= 100_000_000 {
        format!("{:.1}亿", n as f64 / 1e8)
    } else if n >= 10_000 {
        format!("{:.1}万", n as f64 / 1e4)
    } else {
        n.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_basic() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(45), "0:45");
        assert_eq!(format_duration(60), "1:00");
        assert_eq!(format_duration(125), "2:05");
        assert_eq!(format_duration(3600), "1:00:00");
        assert_eq!(format_duration(3725), "1:02:05");
    }

    #[test]
    fn format_count_units() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1000");
        assert_eq!(format_count(10_000), "1.0万");
        assert_eq!(format_count(1_234_567), "123.5万");
        assert_eq!(format_count(100_000_000), "1.0亿");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_chinese() {
        // 中文每个 char 算 1
        let s = "原神角色预告";
        assert_eq!(truncate(s, 100), s);
        assert_eq!(truncate(s, 3), "原神…");
    }

    fn fake_video_result(bvid: Option<&str>, ssid: Option<&str>, kind: &str) -> VideoResult {
        VideoResult {
            kind: kind.to_string(),
            bvid: bvid.map(str::to_string),
            ssid: ssid.map(str::to_string),
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
            arcurl: String::new(),
        }
    }

    #[test]
    fn render_id_column_video() {
        let r = fake_video_result(Some("BV1abc"), None, "video");
        assert_eq!(render_id_column(&r), "BV1abc");
    }

    #[test]
    fn render_id_column_cheese() {
        let r = fake_video_result(None, Some("959815180"), "cheese");
        // "cheese:ss959815180" is exactly 18 chars — wider than the
        // 14-char column, but we don't truncate (the user needs the
        // full ss id to copy). The column will visually overflow
        // for cheese rows; the alternative (truncating) hides data.
        assert_eq!(render_id_column(&r), "cheese:ss959815180");
    }
}
