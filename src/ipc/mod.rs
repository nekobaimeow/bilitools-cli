// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC layer — ported business logic from BiliTools Rust backend.
//!
//! The original code lives under `BiliTools/src-tauri/src/` and is
//! tightly coupled to the Tauri framework. This module is a CLI
//! adaptation that preserves the algorithm (WBI signing, Buvid
//! fingerprint, aria2 RPC, FFmpeg invocations, queue scheduling,
//! SQLite persistence) while removing the GUI layer.

pub mod bilibili_api;
pub mod audio;
pub mod danmaku;
pub mod login;
pub mod aria2c;
pub mod ffmpeg;
pub mod media;
pub mod playurl;
pub mod queue;
pub mod review;
pub mod search;
pub mod shared;
pub mod storage;
pub mod subtitle;
/// Local ASR via external `sensevoice` CLI (Python + FunASR).
///
/// Build with `--features transcribe` to enable. The module is always
/// present in source so the tree builds either way, but its real
/// implementation is gated on the feature. With the feature off, calling
/// `transcribe::transcribe(...)` returns a friendly error.
pub mod transcribe;
