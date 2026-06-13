// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline OCR via PaddleOCR (PP-OCRv5 mobile, MNN backend).
//!
//! Always compiled in — no feature gate.

pub mod engine;
pub mod frames;
pub mod model_paths;
pub mod dedup;
pub mod adaptive;
#[cfg(test)]
mod bench;
