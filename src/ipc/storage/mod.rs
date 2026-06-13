// SPDX-License-Identifier: GPL-3.0-or-later
//! Storage layer — ported from BiliTools `src-tauri/src/storage/*`.
//!
//! The CLI uses the same SQLite schema as the GUI version so both
//! can read and write the same database file at
//! `$XDG_DATA_HOME/com.nekobaimeow.bilicli/Storage/storage.db`.

pub mod config;
pub mod cookies;
pub mod db;
pub mod migrate;
pub mod queue;
pub mod schedulers;
pub mod tasks;
