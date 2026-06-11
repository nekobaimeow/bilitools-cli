// SPDX-License-Identifier: GPL-3.0-or-later
//! Application context — replacement for `tauri::AppHandle`.
//!
//! The GUI version uses `tauri::AppHandle` as a global handle that
//! gives access to paths, the HTTP client, the event bus, and so on.
//! The CLI port packages those into a single `AppContext` built at
//! startup and stored in a global `OnceCell` for the lifetime of
//! the process.

use crate::backends::http::ProxyConfig;
use crate::backends::paths::Paths;
use crate::error::CliError;
use crate::ipc::login;
use crate::ipc::shared::{init_client, init_client_no_proxy, set_proxy};
use crate::ipc::storage::{config, db};
use once_cell::sync::OnceCell;
use std::path::PathBuf;
use std::sync::Arc;

static CTX: OnceCell<Arc<AppContext>> = OnceCell::new();

#[derive(Debug)]
pub struct AppContext {
    pub paths: Paths,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub config_path: PathBuf,
    pub db_path: PathBuf,
    pub settings: tokio::sync::RwLock<config::Settings>,
    pub http: reqwest::Client,
    pub http_no_proxy: reqwest::Client,
    pub proxy: ProxyConfig,
}

impl AppContext {
    /// Build a new `AppContext`. Initializes the database, loads
    /// settings, and prepares shared HTTP clients.
    pub async fn build() -> Result<Arc<Self>, CliError> {
        let paths = Paths::new()?;
        let data_dir = paths.data_dir();
        let log_dir = paths.log_dir();
        let temp_dir = std::env::temp_dir().join("bilitools");
        let config_path = paths.config_file();
        let db_path = paths.db_path();

        // Ensure directories exist before any DB / file access.
        std::fs::create_dir_all(&data_dir).ok();
        std::fs::create_dir_all(&log_dir).ok();
        std::fs::create_dir_all(&temp_dir).ok();
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Initialize DB (idempotent).
        db::init().await?;

        // Bootstrap B 站 风控 fingerprint cookies. These are NOT persisted
        // (B 站 rotates them on a session scale and rejects requests that
        // ship a SESSDATA without the matching buvid3 / bili_ticket / _uuid).
        // We call them once at startup so every subsequent request — search,
        // download, danmaku — has a fully-formed Cookie header.
        //
        // If any of these fail we propagate the error to the caller. The
        // fingerprint APIs themselves are public; failures here usually mean
        // a network/proxy problem the user wants to see, not silently boot
        // with a broken cookie header.
        login::get_buvid().await?;
        login::get_bili_ticket().await?;
        login::get_uuid().await?;

        // Load settings, then construct HTTP clients with the right proxy.
        let settings = config::read().await;
        let proxy = ProxyConfig {
            address: settings.proxy.address.clone(),
            username: settings.proxy.username.clone(),
            password: settings.proxy.password.clone(),
        };
        set_proxy(proxy.clone());
        let http = init_client().await?;
        let http_no_proxy = init_client_no_proxy().await?;

        Ok(Arc::new(AppContext {
            paths,
            data_dir,
            log_dir,
            temp_dir,
            config_path,
            db_path,
            settings: tokio::sync::RwLock::new(settings),
            http,
            http_no_proxy,
            proxy,
        }))
    }

    /// Persist the current settings to the database.
    pub async fn save_settings(&self) -> Result<(), CliError> {
        let s = self.settings.read().await.clone();
        config::write(&s).await
    }

    /// Read-only access to settings.
    pub async fn settings(&self) -> tokio::sync::RwLockReadGuard<'_, config::Settings> {
        self.settings.read().await
    }

    /// Read-write access to settings.
    pub async fn settings_mut(&self) -> tokio::sync::RwLockWriteGuard<'_, config::Settings> {
        self.settings.write().await
    }
}

/// Build or get the global context. Idempotent.
pub async fn ctx() -> Result<&'static Arc<AppContext>, CliError> {
    if let Some(c) = CTX.get() {
        return Ok(c);
    }
    let c = AppContext::build().await?;
    // First writer wins; if another concurrent call raced us we just
    // discard our build.
    let _ = CTX.set(c);
    Ok(CTX.get().expect("context just initialized"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paths_in_context_consistent() {
        let _ = ctx().await; // just don't crash
    }

    /// `AppContext::build()` must bootstrap the B 站 风控 fingerprint
    /// cookies (`buvid3`, `bili_ticket`, `_uuid`) into the global HEADERS
    /// before any user-facing HTTP request runs. Without these, the nav
    /// API (and by extension WBI-protected search) returns HTTP 412.
    #[tokio::test]
    async fn context_bootstraps_fingerprint_cookies() {
        let _ = ctx().await.expect("ctx should build");
        let cookie = crate::ipc::shared::HEADERS.cookie().await;
        assert!(
            cookie.contains("buvid3="),
            "buvid3 missing from HEADERS — get_buvid() was not called during build; cookie={cookie}"
        );
        assert!(
            cookie.contains("bili_ticket="),
            "bili_ticket missing from HEADERS — get_bili_ticket() was not called during build; cookie={cookie}"
        );
        assert!(
            cookie.contains("_uuid="),
            "_uuid missing from HEADERS — get_uuid() was not called during build; cookie={cookie}"
        );
    }
}
