// SPDX-License-Identifier: GPL-3.0-or-later
// `setup` subcommand — one-shot environment bootstrap for analyze.
//
// Checks: python3, sensevoice, funasr, OCR models, ffmpeg.
// Installs missing deps where possible (pip install, git clone).
// For system packages (apt-get), prints instructions.

use crate::cli::output::Output;
use crate::error::CliError;
use std::path::PathBuf;
use tokio::process::Command;

pub async fn run(out: &Output) -> Result<(), CliError> {
    let mut ok = true;

    // ── 1. python3 ──
    let python = match which::which("python3") {
        Ok(p) => {
            out.status(&format!("[✓] python3: {}", p.display()));
            p
        }
        Err(_) => {
            out.status("[✗] python3: not found");
            out.status("  → install: sudo apt install python3  (or brew install python3)");
            ok = false;
            return Err(CliError::MissingDependency(
                "python3 required. Install it first, then re-run setup.".into(),
            ));
        }
    };

    // ── 2. pip + funasr ──
    match Command::new(&python)
        .args(["-c", "import funasr"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => {
            out.status("[✓] funasr: Python package OK");
        }
        _ => {
            out.status("[↓] funasr: installing...");
            let status = Command::new(&python)
                .args(["-m", "pip", "install", "funasr", "numpy", "soundfile"])
                .status()
                .await
                .map_err(|e| CliError::msg(format!("pip install: {e}")))?;
            if status.success() {
                out.status("[✓] funasr: installed");
            } else {
                out.status("[✗] funasr: pip install failed");
                out.status("  → run manually: pip install funasr numpy soundfile");
                ok = false;
            }
        }
    }

    // ── 3. ffmpeg ──
    let ffmpeg_ok = crate::backends::sidecar::resolve(
        crate::backends::sidecar::SidecarKind::FFmpeg,
        None,
    );
    match ffmpeg_ok {
        Ok(p) => { let _ = out.status(&format!("[✓] ffmpeg: {}", p.display())); }
        Err(_) => {
            let _ = out.status("[✗] ffmpeg: not found");
            let _ = out.status("  → install: sudo apt install ffmpeg  (or brew install ffmpeg)");
            ok = false;
        }
    }

    // ── 4. sensevoice CLI ──
    match which::which("sensevoice") {
        Ok(p) => { let _ = out.status(&format!("[✓] sensevoice: {}", p.display())); }
        Err(_) => {
            let _ = out.status("[↓] sensevoice: installing...");
            let install_dir = dirs_sensevoice();
            if !install_dir.exists() {
                let status = Command::new("git")
                    .args([
                        "clone",
                        "https://github.com/nekobaimeow/sensevoice-skill.git",
                    ])
                    .arg(&install_dir)
                    .status()
                    .await
                    .map_err(|e| CliError::msg(format!("git clone: {e}")))?;
                if !status.success() {
                    let _ = out.status("[✗] sensevoice: git clone failed");
                    let _ = out.status("  → manual: git clone https://github.com/nekobaimeow/sensevoice-skill.git");
                    ok = false;
                }
            }
            // chmod +x
            let script = install_dir.join("sensevoice");
            if script.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&script)
                        .map_err(|e| CliError::msg(format!("stat: {e}")))?
                        .permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&script, perms)
                        .map_err(|e| CliError::msg(format!("chmod: {e}")))?;
                }
                // Symlink to ~/.local/bin/
                let bin_dir = home_bin_dir();
                let _ = std::fs::create_dir_all(&bin_dir);
                let link = bin_dir.join("sensevoice");
                if !link.exists() {
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&script, &link).ok();
                    }
                }
                let _ = out.status(&format!("[✓] sensevoice: installed to {}", script.display()));
                let _ = out.status(&format!("  → symlink: {}", link.display()));
            }
        }
    }

    // ── 5. OCR models ──
    let ocr_dir = find_or_create_ocr_models_dir();
    let det = ocr_dir.join("PP-OCRv5_mobile_det_fp16.mnn");
    let rec = ocr_dir.join("PP-OCRv5_mobile_rec_fp16.mnn");
    let keys = ocr_dir.join("ppocr_keys_v5.txt");
    if det.is_file() && rec.is_file() && keys.is_file() {
        out.status(&format!("[✓] ocr_models: {}", ocr_dir.display()));
    } else {
        out.status("[↓] ocr_models: downloading PP-OCRv5 MNN models...");
        // Download from HuggingFace mirror
        let base = "https://huggingface.co/ppogg/ocr_models/resolve/main/PP-OCRv5/mnn";
        let files = [
            ("PP-OCRv5_mobile_det_fp16.mnn", &det),
            ("PP-OCRv5_mobile_rec_fp16.mnn", &rec),
            ("ppocr_keys_v5.txt", &keys),
        ];
        let client = reqwest::Client::new();
        let mut all_ok = true;
        for (name, path) in &files {
            if path.is_file() {
                continue;
            }
            let url = format!("{base}/{name}");
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let bytes = resp.bytes().await.map_err(CliError::from)?;
                    tokio::fs::write(path, &bytes)
                        .await
                        .map_err(CliError::from)?;
                    out.status(&format!("  [✓] downloaded {name}"));
                }
                _ => {
                    out.status(&format!("  [✗] failed to download {name}"));
                    all_ok = false;
                }
            }
        }
        if all_ok {
            out.status("[✓] ocr_models: installed");
        } else {
            out.status("[✗] ocr_models: some models failed to download");
            out.status("  → set BILITOOLS_OCR_MODEL_DIR=/path/to/models");
            ok = false;
        }
    }

    // ── Final ──
    if ok {
        out.status("\n[✓] All dependencies ready. Run `bilitools analyze <BV>` to use.");
    } else {
        out.status("\n[!] Some dependencies could not be installed. See above for manual steps.");
    }
    Ok(())
}

fn home_bin_dir() -> PathBuf {
    let home = dirs_next();
    home.join(".local/bin")
}

fn dirs_sensevoice() -> PathBuf {
    dirs_next().join("sensevoice-skill")
}

fn dirs_next() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        PathBuf::from(h)
    } else {
        PathBuf::from(".")
    }
}

fn find_or_create_ocr_models_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BILITOOLS_OCR_MODEL_DIR") {
        return PathBuf::from(d);
    }
    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let bundled = cwd.join("models/ocr-fast");
    if bundled.exists() {
        return bundled;
    }
    // Create in user's local data dir
    let data = dirs_next().join(".local/share/bilitools/models/ocr-fast");
    let _ = std::fs::create_dir_all(&data);
    data
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {}
}
