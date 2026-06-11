// SPDX-License-Identifier: GPL-3.0-or-later
// CLI root — clap entrypoint. Defines all subcommands and global flags.

use clap::{Parser, Subcommand};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[derive(Parser, Debug)]
#[command(
    name = NAME,
    version = VERSION,
    about = "A pure-Rust CLI port of BiliTools — download videos, audio, and more from Bilibili.",
    long_about = None,
)]
pub struct Cli {
    /// Output machine-readable JSON instead of human-friendly text.
    #[arg(long, short = 'j', global = true)]
    pub json: bool,

    /// Path to a config TOML file (optional; defaults to <data_dir>/config.toml).
    #[arg(long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Override the data directory (defaults to XDG_DATA_HOME/com.btjawa.bilitools).
    #[arg(long, global = true)]
    pub data_dir: Option<std::path::PathBuf>,

    /// Log level: trace, debug, info, warn, error.
    #[arg(long, global = true, default_value = "info")]
    pub log_level: String,

    /// Disable ANSI colors in human mode.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Run a health check before executing.
    #[arg(long, global = true)]
    pub doctor: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show version + paths + build info.
    Info,
    /// Initialize the environment (buvid3/buvid4/ticket/uuid).
    Init,

    /// Authentication.
    #[command(subcommand)]
    Auth(AuthCmd),

    /// Parse a B 站 resource without downloading.
    #[command(subcommand)]
    Parse(ParseCmd),

    /// Download a resource.
    #[command(subcommand)]
    Download(DownloadCmd),

    /// Scheduled / cron-based downloads.
    #[command(subcommand)]
    Schedule(ScheduleCmd),

    /// View / modify configuration.
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Search B 站 for videos, bangumi, or users.
    Search {
        /// Search keyword (e.g. "原神", "4K演示").
        keyword: String,
        /// Resource type: video, bangumi, user, article, audio, live, topic.
        #[arg(long, short = 't', default_value = "video")]
        r#type: String,
        /// Page number (1-based).
        #[arg(long, default_value = "1")]
        page: u32,
        /// Results per page.
        #[arg(long, default_value = "20")]
        page_size: u32,
        /// Limit displayed rows in human mode.
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Fetch danmaku (XML or ASS) for a B 站 video.
    Danmaku {
        /// BV id, av id, or full URL.
        input: String,
        /// Output directory for the .xml / .ass files.
        #[arg(long, short = 'o')]
        output_dir: Option<std::path::PathBuf>,
        /// Source: live, history, or both.
        #[arg(long, short = 's', default_value = "live")]
        source: String,
        /// Output format: xml, ass, or both.
        #[arg(long, short = 'f', default_value = "both")]
        format: String,
        /// Skip the "not logged in" warning.
        #[arg(long)]
        no_login_warn: bool,
    },

    /// Cache management.
    #[command(subcommand)]
    Cache(CacheCmd),

    /// Database management.
    #[command(subcommand)]
    Db(DbCmd),

    /// Run a health check (auto-aria2, ffmpeg, DanmakuFactory, B 站).
    Doctor,

    /// Start an interactive REPL.
    Repl,
}

#[derive(Subcommand, Debug)]
pub enum AuthCmd {
    /// Start a QR-code login; writes the PNG to --output.
    Qrcode {
        /// Where to write the QR PNG.
        #[arg(long, short = 'o')]
        output: Option<std::path::PathBuf>,
    },
    /// Poll a pending QR login state once.
    QrcodePoll {
        /// The qrcode_key returned by `auth qrcode`.
        key: String,
    },
    /// Cancel an ongoing QR login poll.
    QrcodeCancel,
    /// Print current login state.
    Status,
    /// Refresh the cookie using the stored refresh_token.
    Refresh,
    /// Logout (clear cookies).
    Exit,
}

#[derive(Subcommand, Debug)]
pub enum ParseCmd {
    /// Parse any B 站 URL or bare ID.
    Url { input: String },
    /// Parse a BV id.
    Bv { id: String },
    /// Parse an av id.
    Av { id: String },
    /// Parse a season (ss) id.
    Bangumi { id: String },
    /// Parse an episode (ep) id.
    Episode { id: String },
    /// Parse a favorite folder (fid) id.
    Fav { id: String },
    /// Parse watch later.
    Watchlater,
    /// Parse a user space.
    User { id: String },
}

#[derive(Subcommand, Debug)]
pub enum DownloadCmd {
    /// Submit a single resource for download.
    Submit {
        /// Resource URL or bare ID.
        input: String,
        /// Override output directory.
        #[arg(long)]
        output_dir: Option<std::path::PathBuf>,
        /// Override quality (e.g. 80, 112, 116).
        #[arg(long)]
        quality: Option<u32>,
        /// Override audio bitrate code (e.g. 30280, 30251).
        #[arg(long)]
        abr: Option<u32>,
    },
    /// Submit a batch of resources (one URL per line).
    Batch {
        /// File with one URL per line (- for stdin).
        file: std::path::PathBuf,
    },
    /// List all tasks.
    List {
        /// Filter by status (pending, running, paused, completed, failed, cancelled).
        #[arg(long)]
        status: Option<String>,
    },
    /// Show task details.
    Show { id: String },
    /// Cancel a task.
    Cancel { id: String },
    /// Pause a task.
    Pause { id: String },
    /// Resume a paused task.
    Resume { id: String },
    /// Retry a failed task.
    Retry { id: String },
    /// Open a task's output directory.
    Open { id: String },
    /// Run a task: download missing segments and merge to mp4.
    /// Resumable — re-run to continue a partial download.
    Run { id: String },
    /// Run all pending tasks sequentially.
    RunAll,
}

#[derive(Subcommand, Debug)]
pub enum ScheduleCmd {
    /// List all scheduled tasks.
    List,
    /// Add a new scheduled task.
    Add {
        /// Cron expression (6 fields: sec min hour day month dow).
        cron: String,
        /// Resource URL or bare ID.
        input: String,
    },
    /// Remove a scheduled task.
    Remove { id: String },
    /// Trigger a scheduled task immediately.
    Run { id: String },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show the current configuration.
    Show,
    /// Get a single field (dotted path).
    Get { key: String },
    /// Set a single field.
    Set { key: String, value: String },
    /// Reset to defaults.
    Reset,
    /// Print the on-disk config file path.
    Path,
}

#[derive(Subcommand, Debug)]
pub enum CacheCmd {
    /// List cache directories and their sizes.
    List,
    /// Show a single cache's size.
    Size { key: String },
    /// Clean a cache directory.
    Clean { key: String },
    /// Open a cache directory with the system file manager.
    Open { key: String },
}

#[derive(Subcommand, Debug)]
pub enum DbCmd {
    /// Export the database to a file.
    Export { file: std::path::PathBuf },
    /// Import a database from a file (overwrites current).
    Import { file: std::path::PathBuf },
    /// List all tasks in the database.
    Tasks,
}
