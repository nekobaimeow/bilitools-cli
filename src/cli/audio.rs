// SPDX-License-Identifier: GPL-3.0-or-later
// `audio` subcommand — download audio track only (m4a) from a B 站 video.
//
// Optional: with `--features transcribe`, also runs the external
// `sensevoice` Python CLI (https://github.com/nekobaimeow/sensevoice-skill)
// to produce a transcript next to the m4a.

use crate::cli::output::Output;
use crate::cli::root::Command;
use crate::error::CliError;
use crate::ipc::audio;
use std::path::PathBuf;
use std::time::Duration;

pub async fn run(cmd: &Command, out: &Output) -> Result<(), CliError> {
    let Command::Audio {
        input,
        output_dir,
        quality,
        transcribe,
        transcribe_language,
        transcribe_device,
        transcribe_keep_tags,
        sensevoice_cli,
    } = cmd
    else {
        return Err(CliError::Other("internal: not an Audio command".into()));
    };
    let output_dir: PathBuf = output_dir.clone();
    let want_transcribe = *transcribe;

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

    // Optional: run ASR over the downloaded m4a. Gated on build feature.
    if want_transcribe {
        run_transcribe(
            &result,
            out,
            transcribe_language,
            transcribe_device,
            *transcribe_keep_tags,
            sensevoice_cli.as_deref(),
        )
        .await?;
    }

    Ok(())
}

async fn run_transcribe(
    audio: &audio::AudioResult,
    out: &Output,
    language: &str,
    device: &str,
    keep_tags: bool,
    sensevoice_cli: Option<&std::path::Path>,
) -> Result<(), CliError> {
    use crate::ipc::transcribe::{self, TranscribeOpts};

    if audio.m4a_bytes == 0 {
        return Err(CliError::msg(
            "cannot --transcribe: m4a download failed (bytes=0)",
        ));
    }

    let opts = TranscribeOpts {
        m4a_path: audio.m4a_path.clone(),
        output_txt: None, // auto: <stem>_文字稿.txt next to the m4a
        language: language.to_string(),
        device: device.to_string(),
        keep_tags,
        vad_max_sec: 15,
        sensevoice_cli: sensevoice_cli.map(|p| p.to_path_buf()),
        python_bin: None,
        // ~1h audio + RTF 0.12 + first-run model download. Generous.
        timeout: Duration::from_secs(45 * 60),
    };

    out.status(&format!(
        "[transcribe] starting sensevoice (lang={language} device={device} keep_tags={keep_tags})..."
    ));

    let t0 = std::time::Instant::now();
    let result = transcribe::transcribe(&opts).await?;
    let elapsed = t0.elapsed().as_secs_f32();

    if out.is_json() {
        // Re-emit the whole envelope: audio + transcript.
        // We can't easily extend the prior json!() above, so just print a
        // second JSON line for the transcript (validators can `jq -s` it).
        out.ok(serde_json::json!({
            "transcript": {
                "txt_path": result.txt_path,
                "char_count": result.char_count,
                "segment_count": result.segment_count,
                "segments": result.segments,
                "text": result.text,
                "rtf": result.rtf,
                "audio_duration_sec": result.audio_duration_sec,
                "model": result.model,
                "device": result.device,
                "language": result.language,
                "elapsed_sec": elapsed,
                "python_bin": result.python_bin,
                "sensevoice_cli": result.sensevoice_cli,
            }
        }))?;
    } else {
        out.status(&format!(
            "[ok] transcript: {} ({} chars, {} segments)",
            result.txt_path.display(),
            result.char_count,
            result.segment_count
        ));
        if let Some(rtf) = result.rtf {
            out.status(&format!("[ok] model: {} on {}", result.model, result.device));
            out.status(&format!("[ok] rtf: {rtf:.3} (elapsed: {elapsed:.1}s)"));
        }
        if let Some(d) = result.audio_duration_sec {
            out.status(&format!("[ok] audio: {d:.1}s"));
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
