// SPDX-License-Identifier: GPL-3.0-or-later
// Path utilities — replacement for `tauri::Manager::path`.
//
// The CLI uses XDG / standard OS paths to remain GUI-tooling-free.
// Data dir uses our own namespace to avoid conflicts
// with the upstream BiliTools GUI.

use crate::error::CliError;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

const QUALIFIER: &str = "com";
const ORG: &str = "nekobaimeow";
const APP: &str = "bilicli";

/// Wrapper around `directories::ProjectDirs` so we can override data dir
/// via env var `BILICLI_DATA_DIR` (useful for tests and headless installs).
#[derive(Debug)]
pub struct Paths {
    project: ProjectDirs,
    override_data: Option<PathBuf>,
}

impl Paths {
    pub fn new() -> Result<Self, CliError> {
        let project = ProjectDirs::from(QUALIFIER, ORG, APP)
            .ok_or_else(|| CliError::msg("could not determine project directories"))?;
        Ok(Self {
            project,
            override_data: std::env::var_os("BILICLI_DATA_DIR").map(PathBuf::from),
        })
    }

    /// Construct Paths with an explicit data dir override. Used by tests
    /// to avoid race conditions when multiple tests set
    /// `BILICLI_DATA_DIR` concurrently.
    pub fn with_data_dir(data_dir: PathBuf) -> Result<Self, CliError> {
        let project = ProjectDirs::from(QUALIFIER, ORG, APP)
            .ok_or_else(|| CliError::msg("could not determine project directories"))?;
        Ok(Self {
            project,
            override_data: Some(data_dir),
        })
    }

    /// `XDG_DATA_HOME/com.nekobaimeow.bilicli` (Linux)
    /// `~/Library/Application Support/com.nekobaimeow.bilicli` (macOS)
    /// `%AppData%\com.nekobaimeow.bilicli` (Windows)
    pub fn data_dir(&self) -> PathBuf {
        if let Some(ref p) = self.override_data {
            return p.clone();
        }
        self.project.data_dir().to_path_buf()
    }

    /// `data_dir/Storage` — BiliTools GUI keeps its DB here.
    pub fn storage_dir(&self) -> PathBuf {
        self.data_dir().join("Storage")
    }

    /// `data_dir/Storage/storage.db`
    pub fn db_path(&self) -> PathBuf {
        self.storage_dir().join("storage.db")
    }

    /// `XDG_CONFIG_HOME/com.nekobaimeow.bilicli` (CLI-only config file, separate from DB)
    pub fn config_dir(&self) -> PathBuf {
        self.project.config_dir().to_path_buf()
    }

    /// `config_dir/bilicli.toml` — CLI-specific config (sidecar paths, etc).
    /// The bulk of settings still live in the SQLite db for GUI compat.
    pub fn config_file(&self) -> PathBuf {
        self.config_dir().join("bilicli.toml")
    }

    /// `XDG_DATA_HOME/com.nekobaimeow.bilicli/logs/`
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir().join("logs")
    }

    /// `XDG_CACHE_HOME/com.nekobaimeow.bilicli/`
    pub fn cache_dir(&self) -> PathBuf {
        self.project.cache_dir().to_path_buf()
    }

    /// `XDG_RUNTIME_DIR/com.nekobaimeow.bilicli/` — Aria2 RPC secret/socket go here.
    pub fn runtime_dir(&self) -> PathBuf {
        if let Some(runtime) = directories::UserDirs::new().and_then(|_| {
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        }) {
            return runtime.join(APP);
        }
        self.data_dir().join("runtime")
    }

    /// Ensure all needed directories exist.
    pub fn ensure(&self) -> Result<(), crate::error::CliError> {
        for d in [
            self.data_dir(),
            self.storage_dir(),
            self.config_dir(),
            self.log_dir(),
            self.cache_dir(),
        ] {
            std::fs::create_dir_all(&d)?;
        }
        Ok(())
    }

    /// Default `down_dir` — `$HOME/Downloads/bilicli`
    pub fn default_download_dir(&self) -> PathBuf {
        if let Some(home) = directories::UserDirs::new().and_then(|u| u.home_dir().to_path_buf().into()) {
            return home.join("Downloads").join(APP);
        }
        self.data_dir().join("downloads")
    }
}

/// Equivalent of `tauri::Manager::path().app_data_dir()`.
pub fn app_data_dir() -> Result<PathBuf, crate::error::CliError> {
    Ok(Paths::new()?.data_dir())
}

pub fn app_log_dir() -> Result<PathBuf, crate::error::CliError> {
    Ok(Paths::new()?.log_dir())
}

pub fn app_cache_dir() -> Result<PathBuf, crate::error::CliError> {
    Ok(Paths::new()?.cache_dir())
}

/// Recursively compute the size of a directory in bytes.
/// Equivalent of the walker in `tauri::command get_size`.
pub fn dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            if let Ok(m) = entry.metadata() {
                total += m.len();
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn paths_new_succeeds() {
        let p = Paths::new().expect("Paths::new should always succeed on supported OS");
        // data dir is non-empty
        assert!(!p.data_dir().as_os_str().is_empty());
        // storage dir is data_dir/Storage
        assert_eq!(p.storage_dir(), p.data_dir().join("Storage"));
        // db path is storage_dir/storage.db
        assert!(p.db_path().ends_with("storage.db"));
    }

    #[test]
    fn override_data_dir_via_env() {
        let tmp = env::temp_dir().join(format!(
            "bilicli-test-{}",
            uuid::Uuid::new_v4()
        ));
        env::set_var("BILICLI_DATA_DIR", &tmp);
        let p = Paths::new().unwrap();
        assert_eq!(p.data_dir(), tmp);
        env::remove_var("BILICLI_DATA_DIR");
    }

    #[test]
    fn ensure_creates_dirs() {
        let tmp = env::temp_dir().join(format!(
            "bilicli-ensure-{}",
            uuid::Uuid::new_v4()
        ));
        env::set_var("BILICLI_DATA_DIR", &tmp);
        let p = Paths::new().unwrap();
        p.ensure().expect("ensure should succeed");
        assert!(p.data_dir().is_dir());
        assert!(p.storage_dir().is_dir());
        assert!(p.log_dir().is_dir());
        env::remove_var("BILICLI_DATA_DIR");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn dir_size_on_temp_file() {
        let tmp = env::temp_dir().join(format!(
            "bilicli-size-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.txt"), b"hello world").unwrap();
        let size = dir_size(&tmp).unwrap();
        assert!(size >= 11, "expected at least 11 bytes, got {size}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
