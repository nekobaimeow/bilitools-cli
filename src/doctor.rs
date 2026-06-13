// SPDX-License-Identifier: GPL-3.0-or-later
//! Health check — verify that the runtime is ready to download.

use crate::backends::sidecar::{resolve, SidecarKind};
use crate::error::CliError;
use crate::ipc::storage::db;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub hint: Option<String>,
}

/// Run all health checks and return a structured report.
pub async fn run() -> Result<DoctorReport, CliError> {
    let mut checks = Vec::new();

    // 1. Database reachable
    match db::init().await {
        Ok(()) => checks.push(CheckResult {
            name: "database".into(),
            ok: true,
            detail: format!("SQLite at {}", db::db_path().to_string_lossy()),
            hint: None,
        }),
        Err(e) => checks.push(CheckResult {
            name: "database".into(),
            ok: false,
            detail: format!("{e}"),
            hint: Some("check BILITOOLS_DATA_DIR or filesystem permissions".into()),
        }),
    }

    // 2. aria2c
    checks.push(check_sidecar(SidecarKind::Aria2c));

    // 3. ffmpeg
    checks.push(check_sidecar(SidecarKind::FFmpeg));

    // 4. DanmakuFactory (optional)
    checks.push(check_sidecar(SidecarKind::DanmakuFactory));

    // 5. B 站 reachable
    checks.push(check_bilibili_reachable().await);

    // 6. python3 (for ASR)
    checks.push(check_python3());

    // 7. sensevoice CLI (for ASR)
    checks.push(check_sensevoice());

    // 8. funasr Python package (for ASR)
    checks.push(check_funasr().await);

    // 9. OCR models
    checks.push(check_ocr_models());

    let ok = checks.iter().all(|c| c.ok || c.name == "danmaku_factory"); // DanmakuFactory is optional
    Ok(DoctorReport { ok, checks })
}

fn check_sidecar(kind: SidecarKind) -> CheckResult {
    match resolve(kind, None) {
        Ok(p) => CheckResult {
            name: kind.name().to_string(),
            ok: true,
            detail: format!("found at {}", p.display()),
            hint: None,
        },
        Err(_) => CheckResult {
            name: kind.name().to_string(),
            ok: false,
            detail: "not found in PATH".into(),
            hint: Some(match kind {
                SidecarKind::Aria2c => {
                    "install aria2 (apt install aria2 / brew install aria2) or set sidecar.aria2c"
                        .into()
                }
                SidecarKind::FFmpeg => {
                    "install ffmpeg (apt install ffmpeg / brew install ffmpeg) or set sidecar.ffmpeg"
                        .into()
                }
                SidecarKind::DanmakuFactory => {
                    "download DanmakuFactory from https://github.com/hihkm/DanmakuFactory and set sidecar.danmakufactory"
                        .into()
                }
            }),
        },
    }
}

async fn check_bilibili_reachable() -> CheckResult {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return CheckResult {
                name: "bilibili_api".into(),
                ok: false,
                detail: format!("could not build HTTP client: {e}"),
                hint: None,
            }
        }
    };
    match client
        .get("https://api.bilibili.com/x/web-interface/nav")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => CheckResult {
            name: "bilibili_api".into(),
            ok: true,
            detail: format!("HTTP {}", r.status().as_u16()),
            hint: None,
        },
        Ok(r) => CheckResult {
            name: "bilibili_api".into(),
            ok: false,
            detail: format!("HTTP {}", r.status().as_u16()),
            hint: Some("bilibili may be blocked by your network; consider setting a proxy".into()),
        },
        Err(e) => CheckResult {
            name: "bilibili_api".into(),
            ok: false,
            detail: format!("{e}"),
            hint: Some("check your internet connection or proxy settings".into()),
        },
    }
}

fn check_python3() -> CheckResult {
    match which::which("python3") {
        Ok(p) => CheckResult {
            name: "python3".into(),
            ok: true,
            detail: format!("found at {}", p.display()),
            hint: None,
        },
        Err(_) => CheckResult {
            name: "python3".into(),
            ok: false,
            detail: "not found in PATH".into(),
            hint: Some("install: sudo apt install python3  (or brew install python3)".into()),
        },
    }
}

fn check_sensevoice() -> CheckResult {
    match which::which("sensevoice") {
        Ok(p) => CheckResult {
            name: "sensevoice".into(),
            ok: true,
            detail: format!("found at {}", p.display()),
            hint: None,
        },
        Err(_) => CheckResult {
            name: "sensevoice".into(),
            ok: false,
            detail: "not found in PATH".into(),
            hint: Some(
                "install:\n  git clone https://github.com/nekobaimeow/sensevoice-skill.git\n  cd sensevoice-skill && chmod +x sensevoice\n  ln -s $(pwd)/sensevoice ~/.local/bin/sensevoice"
                    .into(),
            ),
        },
    }
}

async fn check_funasr() -> CheckResult {
    let python = match which::which("python3") {
        Ok(p) => p,
        Err(_) => {
            return CheckResult {
                name: "funasr".into(),
                ok: false,
                detail: "python3 not found (required for funasr)".into(),
                hint: Some("install python3 first".into()),
            }
        }
    };
    match tokio::process::Command::new(&python)
        .args(["-c", "import funasr"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => CheckResult {
            name: "funasr".into(),
            ok: true,
            detail: "funasr Python package available".into(),
            hint: None,
        },
        _ => CheckResult {
            name: "funasr".into(),
            ok: false,
            detail: "funasr not installed for python3".into(),
            hint: Some("pip install funasr numpy soundfile".into()),
        },
    }
}

fn check_ocr_models() -> CheckResult {
    // Check the bundled models/ocr-fast/ directory (relative to the binary,
    // or from env BILITOOLS_OCR_MODEL_DIR). At package time we ship
    // PP-OCRv5_mobile_det_fp16.mnn + PP-OCRv5_mobile_rec_fp16.mnn + ppocr_keys_v5.txt.
    let candidates = [
        std::env::var("BILITOOLS_OCR_MODEL_DIR").ok().map(std::path::PathBuf::from),
        std::env::current_dir().ok().map(|d| d.join("models/ocr-fast")),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("models/ocr-fast"))),
    ];
    for maybe_dir in candidates.iter().flatten() {
        let det = maybe_dir.join("PP-OCRv5_mobile_det_fp16.mnn");
        let rec = maybe_dir.join("PP-OCRv5_mobile_rec_fp16.mnn");
        let keys = maybe_dir.join("ppocr_keys_v5.txt");
        if det.is_file() && rec.is_file() && keys.is_file() {
            return CheckResult {
                name: "ocr_models".into(),
                ok: true,
                detail: format!("found at {}", maybe_dir.display()),
                hint: None,
            };
        }
    }
    CheckResult {
        name: "ocr_models".into(),
        ok: false,
        detail: "PP-OCRv5 MNN models not found".into(),
        hint: Some(
            "set BILITOOLS_OCR_MODEL_DIR=/path/to/models/ocr-fast\n  \
             Models (det/rec fp16 MNN + keys) are bundled in the crate release archive."
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_returns_structured_report() {
        let r = run().await.unwrap();
        // Even if the network is down we still get a structured report.
        assert!(!r.checks.is_empty());
        assert!(r.checks.iter().any(|c| c.name == "database"));
    }
}
