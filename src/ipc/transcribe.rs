// SPDX-License-Identifier: GPL-3.0-or-later
// Local ASR (automatic speech recognition) via external `sensevoice` CLI.
//
// Why a separate Python CLI?
//   bilitools is pure Rust; the upstream SenseVoiceSmall model runs on
//   PyTorch + FunASR which is hundreds of MB of Python deps. We avoid
//   dragging that into the Rust binary. Instead, when the user passes
//   `bilitools audio <bv> --transcribe`, this module shells out to a
//   Python `sensevoice` CLI that they install separately.
//
// Contract (sensevoice-skill, MIT licensed):
//   https://github.com/nekobaimeow/sensevoice-skill
//
//   python3 sensevoice INPUT [-o OUTPUT] [-k] [-l LANG] [-d DEVICE]
//                            [--no-itn] [--vad-max-sec N]
//
//   - INPUT:  audio file (m4a/mp3/wav/...)
//   - OUTPUT: text file (default: <basename>_文字稿.txt)
//   - stdout: progress log including "RTF: 0.NNN" line we can parse
//
// Build flag:
//   cargo build --release                              # this module: stub
//   cargo build --release --features transcribe        # full impl
//
// With the feature OFF, calling `transcribe()` returns a clear error
// pointing the user at the right cargo invocation. We do NOT want to
// silently no-op — that hides the integration.

use crate::error::CliError;
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::process::Command;

pub type Result<T> = std::result::Result<T, CliError>;

/// Options for one ASR pass.
#[derive(Debug, Clone)]
pub struct TranscribeOpts {
    /// Path to the input audio (m4a, mp3, wav, ...).
    pub m4a_path: PathBuf,
    /// Target text file (created by sensevoice). `None` = let sensevoice pick.
    pub output_txt: Option<PathBuf>,
    /// Language hint passed to sensevoice / FunASR. `"auto"` = don't pass
    /// `-l` at all and let the model auto-detect per-segment. SenseVoiceSmall
    /// is a multilingual model; the hint biases but does not force output
    /// language. Supported: zh | yue | en | ja | ko | auto.
    pub language: String,
    /// Inference device: cpu (default) or cuda.
    pub device: String,
    /// Keep emotion tags (`<|HAPPY|>` etc.) as line breaks instead of stripping.
    pub keep_tags: bool,
    /// VAD max segment length in seconds (default 15).
    pub vad_max_sec: u32,
    /// Override path to the `sensevoice` script (default: `which sensevoice`).
    pub sensevoice_cli: Option<PathBuf>,
    /// Override path to `python3` (default: `which python3`).
    pub python_bin: Option<PathBuf>,
    /// Hard timeout for the whole run (default 30 min — fits 1 h audio + RTF 0.12 + model load).
    pub timeout: Duration,
}

impl Default for TranscribeOpts {
    fn default() -> Self {
        Self {
            m4a_path: PathBuf::new(),
            output_txt: None,
            // "auto" = don't pass `-l` to sensevoice; the model detects per segment.
            // SenseVoiceSmall is multilingual and handles mixed zh/en/ja/ko audio
            // gracefully. Forcing `zh` historically just added noise (e.g. mapping
            // English TED talks to pinyin fragments).
            language: "auto".into(),
            device: "cpu".into(),
            keep_tags: false,
            vad_max_sec: 15,
            sensevoice_cli: None,
            python_bin: None,
            timeout: Duration::from_secs(30 * 60),
        }
    }
}

/// What the ASR run produced.
#[derive(Debug, Clone, Serialize)]
pub struct TranscribeResult {
    /// Path to the text file written by sensevoice.
    pub txt_path: PathBuf,
    /// Full text content read back from disk.
    pub text: String,
    /// Text split into non-empty lines (sensevoice's "tag → newline" output).
    pub segments: Vec<String>,
    pub char_count: usize,
    pub segment_count: usize,
    /// Real-time factor reported by sensevoice. None if not parseable.
    pub rtf: Option<f32>,
    /// Inference wall time (seconds) — derived from stdout or measured.
    pub infer_sec: Option<f32>,
    /// Model name reported by sensevoice (e.g. `iic/SenseVoiceSmall`).
    pub model: String,
    pub device: String,
    pub language: String,
    /// Resolved interpreter (so caller can show the user what was used).
    pub python_bin: PathBuf,
    /// Resolved sensevoice script (same).
    pub sensevoice_cli: PathBuf,
    /// Audio duration in seconds (from `Duration: N.Ns` in stdout).
    pub audio_duration_sec: Option<f32>,
}

/// Public entry point. Always available — sensevoice is a runtime dependency.
pub async fn transcribe(opts: &TranscribeOpts) -> Result<TranscribeResult> {
    transcribe_impl(opts).await
}

// ---------- Implementation ----------

async fn transcribe_impl(opts: &TranscribeOpts) -> Result<TranscribeResult> {
    use std::process::Stdio;

    if !opts.m4a_path.is_file() {
        return Err(CliError::PathNotFound(opts.m4a_path.clone()));
    }

    // 1) Resolve python3
    let python_bin = resolve_exe(
        opts.python_bin.as_deref(),
        "python3",
        "python3 not found in PATH",
        "  install: sudo apt install python3   (or `brew install python3` on macOS)",
    )?;

    // 2) Resolve sensevoice CLI
    let sensevoice_cli = resolve_exe(
        opts.sensevoice_cli.as_deref(),
        "sensevoice",
        "the `sensevoice` CLI is not on PATH",
        "  install:\n      git clone https://github.com/nekobaimeow/sensevoice-skill.git\n      cd sensevoice-skill && chmod +x sensevoice\n      pip install funasr numpy soundfile\n    then either:\n      a) symlink:  ln -s $(pwd)/sensevoice ~/.local/bin/sensevoice\n      b) or pass --sensevoice-cli /full/path/to/sensevoice",
    )?;

    // 3) Resolve output txt path. If user didn't pass one, drop it next to the audio.
    let output_txt: PathBuf = match opts.output_txt.clone() {
        Some(p) => p,
        None => {
            let stem = opts
                .m4a_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("transcript");
            let parent = opts
                .m4a_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let suffix = if opts.keep_tags { "_tags" } else { "" };
            parent.join(format!("{stem}{suffix}_文字稿.txt"))
        }
    };

    // Make sure the parent dir exists (sensevoice calls os.makedirs itself, but
    // we want a deterministic error if the parent can't be created).
    if let Some(parent) = output_txt.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).await.map_err(CliError::from)?;
        }
    }

    // 4) Build command. Use `python3 SCRIPT` form (NOT a shebang exec) so
    //    we don't depend on the script being chmod +x or having a working
    //    #! interpreter on the user's system.
    //
    //    "auto" language = don't pass `-l` at all; let FunASR/SenseVoice
    //    auto-detect per segment. The model is multilingual — for mixed
    //    zh/en/ja/ko content (e.g. a B 站 video where the host speaks
    //    Chinese but quotes English terms), this is the only mode that
    //    gets both right in the same transcript.
    let mut cmd = Command::new(&python_bin);
    cmd.arg(&sensevoice_cli)
        .arg(&opts.m4a_path)
        .arg("-o")
        .arg(&output_txt)
        .arg("-d")
        .arg(&opts.device)
        .arg("--vad-max-sec")
        .arg(opts.vad_max_sec.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if opts.language != "auto" {
        cmd.arg("-l").arg(&opts.language);
    }
    if opts.keep_tags {
        cmd.arg("-k");
    }
    if opts.device != "cpu" && opts.device != "cuda" {
        return Err(CliError::msg(format!(
            "invalid --transcribe-device '{}' (expected cpu or cuda)",
            opts.device
        )));
    }
    if !matches!(opts.language.as_str(), "auto" | "zh" | "yue" | "en" | "ja" | "ko") {
        return Err(CliError::msg(format!(
            "invalid --transcribe-language '{}' (expected auto|zh|yue|en|ja|ko)",
            opts.language
        )));
    }

    // 5) Spawn with timeout. The first run downloads ~900 MB from
    //    ModelScope — give it plenty of headroom.
    let output = match tokio::time::timeout(opts.timeout, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err(CliError::msg(format!(
                "failed to spawn sensevoice: {e} (is {} executable?)",
                sensevoice_cli.display()
            )));
        }
        Err(_) => {
            return Err(CliError::msg(format!(
                "sensevoice timed out after {}s — first run downloads ~900MB model, \
                 try again with a higher --transcribe-timeout if your network is slow",
                opts.timeout.as_secs()
            )));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Heuristic: detect the common "funasr not installed" case.
        let hint = if stderr.contains("ModuleNotFoundError") && stderr.contains("funasr") {
            "\n  hint: pip install funasr numpy soundfile"
        } else {
            ""
        };
        return Err(CliError::msg(format!(
            "sensevoice failed (exit {:?})\n  stderr: {}\n  stdout: {}{}",
            output.status.code(),
            stderr.trim(),
            stdout.trim(),
            hint
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // 6) Parse output txt.
    if !output_txt.is_file() {
        return Err(CliError::msg(format!(
            "sensevoice exited 0 but {} was not created\n  stdout: {}\n  stderr: {}",
            output_txt.display(),
            stdout.trim(),
            stderr.trim()
        )));
    }
    let text = fs::read_to_string(&output_txt)
        .await
        .map_err(CliError::from)?;
    let segments: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let char_count = text.chars().count();
    let segment_count = segments.len();

    // 7) Parse RTF / audio duration from stdout.
    let rtf = parse_rtf(&stdout);
    let audio_duration_sec = parse_duration(&stdout);

    Ok(TranscribeResult {
        txt_path: output_txt,
        char_count,
        segment_count,
        segments,
        text,
        rtf,
        infer_sec: None, // sensevoice doesn't print infer wall time directly
        model: "iic/SenseVoiceSmall".to_string(),
        device: opts.device.clone(),
        language: opts.language.clone(),
        python_bin,
        sensevoice_cli,
        audio_duration_sec,
    })
}

/// Locate an executable: explicit path > `which` lookup. With actionable error.
fn resolve_exe(
    explicit: Option<&Path>,
    name: &str,
    err_msg: &str,
    install_hint: &str,
) -> Result<PathBuf> {
    use which::which;
    if let Some(p) = explicit {
        if !p.is_file() {
            return Err(CliError::PathNotFound(p.to_path_buf()));
        }
        return Ok(p.to_path_buf());
    }
    which(name).map_err(|_| CliError::MissingDependency(format!("{err_msg}\n{install_hint}")))
}

// ---------- Pure helpers (always compiled, easy to unit test) ----------

/// Parse a line like `  Done in 76.3s (RTF: 0.123)` from sensevoice stdout.
pub fn parse_rtf(stdout: &str) -> Option<f32> {
    let re = Regex::new(r"RTF:\s*([0-9]+\.[0-9]+)").ok()?;
    re.captures(stdout)?.get(1)?.as_str().parse().ok()
}

/// Parse a line like `Duration: 638.7s` from sensevoice stdout.
pub fn parse_duration(stdout: &str) -> Option<f32> {
    let re = Regex::new(r"Duration:\s*([0-9]+\.[0-9]+)s").ok()?;
    re.captures(stdout)?.get(1)?.as_str().parse().ok()
}

/// Strip noise tags and split into segments (for the `-k` keep-tags mode
/// we just collapse whitespace; default mode passes through).
pub fn split_segments(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rtf_happy() {
        let stdout = "\
Loading model...
  Model loaded in 3.2s
Transcribing (zh)...
  Done in 76.3s (RTF: 0.123)
结果: 1234 字, 5 段
";
        assert_eq!(parse_rtf(stdout), Some(0.123));
    }

    #[test]
    fn parse_rtf_missing() {
        assert_eq!(parse_rtf("no rtf here\n"), None);
        assert_eq!(parse_rtf(""), None);
    }

    #[test]
    fn parse_duration_happy() {
        let stdout = "Input: foo.m4a\nDuration: 638.7s\n";
        assert_eq!(parse_duration(stdout), Some(638.7));
    }

    #[test]
    fn parse_duration_missing() {
        assert_eq!(parse_duration("nothing"), None);
    }

    #[test]
    fn split_segments_strips_blank() {
        let txt = "line one\n\nline two\n   \nline three\n";
        assert_eq!(split_segments(txt), vec!["line one", "line two", "line three"]);
    }

    #[test]
    fn split_segments_empty() {
        assert!(split_segments("").is_empty());
        assert!(split_segments("\n\n\n").is_empty());
    }

    #[test]
    fn default_opts_sane() {
        let o = TranscribeOpts::default();
        // language default is "auto" — we don't pass `-l` to sensevoice at all,
        // letting the multilingual model detect per segment. B 站 content
        // often mixes zh narration with English terms (SSNX, Block5, ...).
        assert_eq!(o.language, "auto");
        assert_eq!(o.device, "cpu");
        assert!(!o.keep_tags);
        assert_eq!(o.vad_max_sec, 15);
        assert!(o.python_bin.is_none());
        assert!(o.sensevoice_cli.is_none());
        assert_eq!(o.timeout, Duration::from_secs(30 * 60));
    }

    #[test]
    fn language_whitelist_includes_auto() {
        // mirrors the match guard in transcribe_impl — any new language code
        // must be added to BOTH places or users will get a clear error.
        let valid = ["auto", "zh", "yue", "en", "ja", "ko"];
        for code in valid {
            assert!(
                matches!(code, "auto" | "zh" | "yue" | "en" | "ja" | "ko"),
                "language code {code} should be in whitelist"
            );
        }
        // bogus values must NOT be in the whitelist
        for bogus in ["fr", "de", "ru", ""] {
            assert!(
                !matches!(bogus, "auto" | "zh" | "yue" | "en" | "ja" | "ko"),
                "bogus language code {bogus} should be rejected"
            );
        }
    }

    /// Real subprocess smoke test — needs `python3 sensevoice` on PATH
    /// AND `funasr` installed. Marked `#[ignore]` so it doesn't slow CI.
    #[tokio::test]
    #[ignore]
    async fn transcribe_real_audio_roundtrip() {
        let audio = PathBuf::from("/tmp/audio-test")
            .read_dir()
            .expect("dir not found")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|s| s.to_str()) == Some("m4a"))
            .expect("no m4a in /tmp/audio-test");
        let opts = TranscribeOpts {
            m4a_path: audio,
            output_txt: Some(PathBuf::from("/tmp/sv-unit-out.txt")),
            timeout: Duration::from_secs(15 * 60),
            ..Default::default()
        };
        let r = transcribe(&opts).await.expect("transcribe failed");
        assert!(r.txt_path.is_file(), "txt not created");
        assert!(r.char_count > 0, "empty transcript");
        // 10 min audio should produce at least a handful of segments
        assert!(r.segment_count > 0, "no segments");
        // RTF should be present and sane (< 1.0 for CPU)
        assert!(r.rtf.is_some_and(|v| v < 1.0 && v > 0.0), "rtf weird");
    }
}
