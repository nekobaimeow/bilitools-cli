// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC layer — ported business logic from BiliTools Rust backend.
//!
//! The original code lives under `BiliTools/src-tauri/src/` and is
//! tightly coupled to the Tauri framework. This module is a CLI
//! adaptation that preserves the algorithm (WBI signing, Buvid
//! fingerprint, aria2 RPC, FFmpeg invocations, queue scheduling,
//! SQLite persistence) while removing the GUI layer.

pub mod shared;
pub mod storage;

pub mod bilibili_api;
pub mod login;
pub mod aria2c;
pub mod ffmpeg;
pub mod media;
pub mod playurl;
pub mod queue;
