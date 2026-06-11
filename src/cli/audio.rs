// SPDX-License-Identifier: GPL-3.0-or-later
// `audio` subcommand — download audio track only (m4a) from a B 站 video.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::audio;
use std::path::PathBuf;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Audio {
        input,
        output_dir,
        quality,
    } = cmd
    else {
        return Err(CliError::Other("internal: not an Audio command".into()));
    };
    let output_dir: PathBuf = output_dir.clone();

    let result = audio::fetch_audio(input, &output_dir, *quality as i64).await?;

    if out.is_json() {
        out.ok(serde_json::json!({
            "bv": result.bv,
            "aid": result.aid,
            "cid": result.cid,
            "title": result.title,
            "quality_qn": result.quality_qn,
            "audio_codec": result.audio_codec,
            "audio_bandwidth": result.audio_bandwidth,
            "duration_sec": result.duration_sec,
            "segments_downloaded": result.segments_downloaded,
            "m4a_path": result.m4a_path,
            "m4a_bytes": result.m4a_bytes,
            "degraded": result.degraded,
        }))?;
    } else {
        out.status(&format!("title:        {}", result.title));
        out.status(&format!("bv:           {}", result.bv));
        out.status(&format!("aid/cid:      {}/{}", result.aid, result.cid));
        out.status(&format!("quality qn:   {}", result.quality_qn));
        out.status(&format!("audio codec:  {}", result.audio_codec));
        out.status(&format!("bitrate:      {} bps", result.audio_bandwidth));
        out.status(&format!(
            "duration:     {:.1}s",
            result.duration_sec
        ));
        for d in &result.degraded {
            out.status(&format!("[degraded] {d}"));
        }
        if result.m4a_bytes > 0 {
            out.status(&format!(
                "[ok] m4a:    {} ({} bytes, {} segments)",
                result.m4a_path.display(),
                result.m4a_bytes,
                result.segments_downloaded,
            ));
        } else {
            out.status(&format!(
                "[warn] m4a conversion failed; m4s files kept in {}",
                output_dir.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {
        // cargo build 即可验证
    }
}
