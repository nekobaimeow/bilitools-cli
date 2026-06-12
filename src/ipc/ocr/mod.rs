// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline OCR via PaddleOCR (PP-OCRv5 mobile, MNN backend).
//!
//! The real implementation is gated on the `ocr` cargo feature. With the
//! feature off, the public entry points return a friendly error pointing
//! at the missing feature flag — the module is otherwise present in the
//! source tree so the workspace builds cleanly either way.

/// Stub entry point used when the `ocr` feature is disabled.
pub fn feature_disabled_error() -> String {
    "OCR is not enabled in this build. Reinstall with:\n  \
     cargo install bilitools --features ocr\n  \
     or build from source with --features ocr.\n\
     \n\
     Then place PP-OCRv5 mobile MNN models (det/rec + ppocr_keys_v5.txt) in\n  \
     ./models/ocr-fast/  or set BILITOOLS_OCR_MODEL_DIR=/path/to/models."
        .into()
}

#[cfg(feature = "ocr")]
pub mod engine;
#[cfg(feature = "ocr")]
pub mod frames;
#[cfg(feature = "ocr")]
pub mod model_paths;
#[cfg(feature = "ocr")]
pub mod dedup;
