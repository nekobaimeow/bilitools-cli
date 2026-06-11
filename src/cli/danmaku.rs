// SPDX-License-Identifier: GPL-3.0-or-later
// `danmaku` subcommand.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::danmaku::{self, DanmakuFormat, DanmakuSource};
use std::path::PathBuf;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Danmaku {
        input,
        output_dir,
        source,
        format,
        no_login_warn,
    } = cmd
    else {
        return Err(CliError::Other("internal: not a Danmaku command".into()));
    };
    let source: DanmakuSource = source.parse().map_err(|e: CliError| e)?;
    let format: DanmakuFormat = format.parse().map_err(|e: CliError| e)?;

    let output_dir: PathBuf = output_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."));

    if !*no_login_warn && !danmaku::has_login_cookie().await {
        out.status(
            "[warn] not logged in; live danmaku may be rate-limited. run `bilitools auth qrcode` first.",
        );
    }

    let result = danmaku::fetch_and_convert(input, &output_dir, source, format).await?;

    if out.is_json() {
        out.ok(serde_json::json!({
            "title": result.title,
            "cid": result.cid,
            "live_count": result.live_count,
            "history_count": result.history_count,
            "xml_path": result.xml_path,
            "ass_path": result.ass_path,
            "danmakufactory_used": result.danmakufactory_used,
            "degraded": result.degraded,
        }))?;
    } else {
        out.status(&format!("title:  {}", result.title));
        out.status(&format!("cid:    {}", result.cid));
        out.status(&format!(
            "live:   {} danmaku{}",
            result.live_count,
            if result.history_count > 0 {
                format!(", history: {}", result.history_count)
            } else {
                String::new()
            }
        ));
        if let Some(p) = &result.xml_path {
            out.status(&format!("xml:    {}", p.display()));
        }
        if let Some(p) = &result.ass_path {
            out.status(&format!("ass:    {}", p.display()));
        }
        if result.danmakufactory_used {
            out.status("[ok] DanmakuFactory converted XML → ASS");
        } else if matches!(format, DanmakuFormat::Ass | DanmakuFormat::Both) {
            out.status("[warn] ASS conversion skipped (DanmakuFactory not installed or failed)");
        }
        for d in &result.degraded {
            out.status(&format!("[degraded] {d}"));
        }
    }
    Ok(())
}
