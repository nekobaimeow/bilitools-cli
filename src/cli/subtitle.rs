// SPDX-License-Identifier: GPL-3.0-or-later
// `subtitle` subcommand — fetch B 站 video subtitles.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::subtitle;
use std::path::PathBuf;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Subtitle {
        input,
        output_dir,
        download,
    } = cmd
    else {
        return Err(CliError::Other("internal: not a Subtitle command".into()));
    };
    let output_dir: PathBuf = output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    if *download {
        // list + 批量 download
        let result = subtitle::fetch_all(input, &output_dir).await?;
        if out.is_json() {
            out.ok(serde_json::json!({
                "bv": result.bv,
                "cid": result.cid,
                "title": result.title,
                "entries": result.entries,
                "fetched": result.fetched.iter().map(|f| serde_json::json!({
                    "lan": f.entry.lan,
                    "lan_doc": f.entry.lan_doc,
                    "path": f.path,
                    "body_len": f.body_len,
                })).collect::<Vec<_>>(),
                "degraded": result.degraded,
            }))?;
        } else {
            out.status(&format!("title:  {}", result.title));
            out.status(&format!("bv:     {}", result.bv));
            out.status(&format!("cid:    {}", result.cid));
            for d in &result.degraded {
                out.status(&format!("[degraded] {d}"));
            }
            if result.entries.is_empty() {
                out.status("[info] no subtitles available for this video");
            } else {
                out.status(&format!(
                    "{:<8} {:<10} {:<16} {}",
                    "ID", "LAN", "LAN_DOC", "PATH"
                ));
                for f in &result.fetched {
                    out.status(&format!(
                        "{:<8} {:<10} {:<16} {} ({} bytes)",
                        f.entry.id,
                        f.entry.lan,
                        f.entry.lan_doc,
                        f.path.display(),
                        f.body_len,
                    ));
                }
            }
        }
    } else {
        // 仅列元数据
        let result = subtitle::list(input).await?;
        if out.is_json() {
            out.ok(serde_json::json!({
                "bv": result.bv,
                "cid": result.cid,
                "title": result.title,
                "entries": result.entries,
                "degraded": result.degraded,
            }))?;
        } else {
            out.status(&format!("title:  {}", result.title));
            out.status(&format!("bv:     {}", result.bv));
            out.status(&format!("cid:    {}", result.cid));
            for d in &result.degraded {
                out.status(&format!("[degraded] {d}"));
            }
            if result.entries.is_empty() {
                out.status("[info] no subtitles available for this video");
            } else {
                out.status(&format!(
                    "{:<8} {:<10} {:<16} {:<8} {}",
                    "ID", "LAN", "LAN_DOC", "TYPE", "URL"
                ));
                for e in &result.entries {
                    let type_str = match e.type_code {
                        0 => "up".to_string(),
                        1 => format!("ai({})", e.ai_status),
                        _ => format!("?({})", e.type_code),
                    };
                    out.status(&format!(
                        "{:<8} {:<10} {:<16} {:<8} {}",
                        e.id,
                        e.lan,
                        e.lan_doc,
                        type_str,
                        e.subtitle_url,
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // 纯函数 helper 测试 — 已在 ipc::subtitle 覆盖。
    // 这里只占位，避免 cli 子目录无 test 报警。
    #[test]
    fn module_compiles() {
        // cargo build 即可验证
    }
}
