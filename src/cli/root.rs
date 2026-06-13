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

    /// Override the data directory (defaults to XDG_DATA_HOME/com.nekobaimeow.bilicli).
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

    /// Fetch comments (reviews) for a B 站 video.
    Review {
        /// BV id, av id, or full URL.
        input: String,
        /// Sort: hot (default) or time.
        #[arg(long, short = 's', default_value = "hot")]
        sort: String,
        /// Page number (1-based).
        #[arg(long, default_value = "1")]
        page: u32,
        /// Results per page (max 30; 3-5 for anonymous).
        #[arg(long, default_value = "20")]
        ps: u32,
        /// Fetch sub-replies for a given root rpid instead of main.
        #[arg(long)]
        sub: Option<String>,
        /// Skip the "not logged in" warning.
        #[arg(long)]
        no_login_warn: bool,
    },

    /// Fetch subtitles (metadata or JSON files) for a B 站 video.
    Subtitle {
        /// BV id, av id, or full URL.
        input: String,
        /// Output directory for downloaded .json files.
        #[arg(long, short = 'o')]
        output_dir: Option<std::path::PathBuf>,
        /// Download subtitle JSON bodies to disk (default: list metadata only).
        #[arg(long, short = 'd')]
        download: bool,
    },

    /// Batch-fetch danmaku + reviews + subtitles for top-N search results.
    Harvest {
        /// Search keyword.
        keyword: String,
        /// Number of top results to process.
        #[arg(long, short = 'l', default_value = "5")]
        limit: u32,
        /// Output directory (one subdir per video).
        #[arg(long, short = 'o', default_value = "./harvest")]
        output_dir: std::path::PathBuf,
        /// Skip danmaku fetch.
        #[arg(long)]
        no_danmaku: bool,
        /// Skip review fetch.
        #[arg(long)]
        no_review: bool,
        /// Skip subtitle fetch.
        #[arg(long)]
        no_subtitle: bool,
        /// Review page size (default 20; auto-capped to 3 for anonymous).
        #[arg(long, default_value = "20")]
        review_ps: u32,
    },

    /// Fetch B站 popular/trending videos (综合热门).
    Hot {
        /// Page number (1-based).
        #[arg(long, default_value = "1")]
        page: u32,
        /// Results per page (max 50).
        #[arg(long, default_value = "20")]
        page_size: u32,
    },

    /// Download the audio track only (m4a) from a B 站 video.
    ///
    /// Use case: extract audio for offline listening or speech-to-text
    /// post-processing (e.g. whisper, MiniMax). The video stream is
    /// never downloaded.
    Audio {
        /// BV id, av id, or full URL.
        input: String,
        /// Output directory for the .m4a file.
        #[arg(long, short = 'o', default_value = ".")]
        output_dir: std::path::PathBuf,
        /// Video quality code for the DASH tier (default 80 = 1080P).
        /// Audio bitrate is chosen by B 站 independently of this.
        #[arg(long, short = 'q', default_value = "80")]
        quality: u32,
        /// Run local ASR after download using the external `sensevoice` CLI
        /// (https://github.com/nekobaimeow/sensevoice-skill).
        /// Requires `sensevoice` on PATH.
        /// First run downloads ~900 MB SenseVoiceSmall model from ModelScope.
        #[arg(long)]
        transcribe: bool,
        /// Language hint for ASR: auto | zh | yue | en | ja | ko.
        /// "auto" (default) lets the SenseVoiceSmall model detect per segment —
        /// recommended for B 站 content that mixes Chinese narration with
        /// English terms (model names, acronyms, etc.).
        #[arg(long, default_value = "auto")]
        transcribe_language: String,
        /// Inference device: cpu | cuda (default cpu).
        #[arg(long, default_value = "cpu")]
        transcribe_device: String,
        /// Keep emotion tags (<|HAPPY|> etc.) in the transcript.
        #[arg(long)]
        transcribe_keep_tags: bool,
        /// Override path to the `sensevoice` script (default: which sensevoice).
        #[arg(long)]
        sensevoice_cli: Option<std::path::PathBuf>,
    },

    /// Run offline OCR on a local image or video (PP-OCRv5 mobile via MNN).
    ///
    /// Use case: extract hard-coded text from screenshots, video title
    /// cards, on-screen watermarks, B-roll subtitles, etc. — text that
    /// B 站's own AI subtitle pipeline misses.
    Ocr {
        /// Image file path, or — with `--video` — a local video file.
        /// (B 站 BV/AV support requires you to download first; the error
        /// message will tell you the exact `bilicli download` command.)
        input: String,
        /// Treat `input` as a local video file. ffmpeg extracts frames
        /// at `--interval` spacing and OCRs each one.
        #[arg(long)]
        video: bool,
        /// Frame interval in seconds when `--video` is used.
        #[arg(long, default_value = "1.0")]
        interval: f32,
        /// Maximum number of frames to OCR from a video. 0 = unlimited
        /// (one OCR per second of video, which is the 1s baseline the
        /// v3 algorithm traverses). The v3 algorithm itself doesn't
        /// benefit from a low cap — it stops recursing when it runs
        /// out of budget, which truncates long videos. The user
        /// spec is "the algorithm is a traversal, run it over the
        /// whole video", so default is unlimited.
        #[arg(long, default_value = "0")]
        max_frames: u32,
        /// Minimum confidence to keep a detection (0.0-1.0).
        #[arg(long, default_value = "0.45")]
        min_conf: f32,
        /// Output directory for `ocr.json` (and frames, with `--keep-frames`).
        /// Defaults to `./ocr_out/<unix-timestamp>/`.
        #[arg(long, short = 'o')]
        output_dir: Option<std::path::PathBuf>,
        /// Keep extracted frames on disk after OCR (default: delete).
        #[arg(long)]
        keep_frames: bool,
        /// Dedup window in seconds. Raw detections in the same spatial
        /// region with similar text within this window are merged into
        /// one record (default: 3.0). Set to 0 to disable dedup.
        #[arg(long, default_value = "3.0")]
        dedup_window: f32,
        /// Bbox IoU threshold for "same spatial region" during dedup
        /// (default: 0.6). Lower = stricter (fewer merges).
        #[arg(long, default_value = "0.6")]
        dedup_iou: f32,
        /// Frame-sampling mode for video input:
        ///   adaptive  — binary-search recursion that stops a branch
        ///               early when adjacent samples are "basically the
        ///               same" content. Fewest OCRs, no missed content.
        ///               (default)
        ///   linear    — fixed `--interval` seconds. Predictable, more
        ///               thorough, more redundant.
        #[arg(long, value_enum, default_value_t = SampleMode::Adaptive)]
        sample_mode: SampleMode,
    },

    /// Full-content analysis — extract all text from a video in one shot.
    /// Pulls audio + ASR transcript, danmaku, subtitles, reviews, and
    /// optionally OCR. Outputs analysis.json + analysis.txt.
    Analyze {
        /// BV id, av id, or full URL.
        input: String,
        /// Output directory (default: ./analyze_output/<BV>-<slug>/).
        #[arg(long, short = 'o')]
        output_dir: Option<std::path::PathBuf>,
        /// Skip audio download + ASR transcription.
        #[arg(long)]
        no_audio: bool,
        /// Skip danmaku fetch.
        #[arg(long)]
        no_danmaku: bool,
        /// Skip subtitle fetch.
        #[arg(long)]
        no_subtitle: bool,
        /// Skip review/comment fetch.
        #[arg(long)]
        no_review: bool,
        /// Number of review pages to fetch (20 per page, default 1).
        #[arg(long, default_value = "1")]
        review_pages: u32,
        /// Skip OCR over video frames. By default, OCR runs on the downloaded video.
        #[arg(long)]
        no_ocr: bool,
        /// Language hint for ASR: auto | zh | yue | en | ja | ko.
        #[arg(long, default_value = "auto")]
        transcribe_language: String,
        /// Inference device: cpu | cuda.
        #[arg(long, default_value = "cpu")]
        transcribe_device: String,
        /// Keep emotion tags in transcript.
        #[arg(long)]
        transcribe_keep_tags: bool,
        /// OCR frame interval in seconds.
        #[arg(long, default_value = "1.0")]
        ocr_interval: f32,
        /// Max OCR frames (0 = unlimited).
        #[arg(long, default_value = "0")]
        ocr_max_frames: u32,
        /// Minimum OCR confidence (0.0-1.0).
        #[arg(long, default_value = "0.45")]
        ocr_min_conf: f32,
        /// OCR dedup window in seconds.
        #[arg(long, default_value = "3.0")]
        ocr_dedup_window: f32,
        /// Local video file path for OCR (auto-downloaded if not given).
        #[arg(long)]
        video_path: Option<std::path::PathBuf>,
    },

    /// Cache management.
    #[command(subcommand)]
    Cache(CacheCmd),

    /// Database management.
    #[command(subcommand)]
    Db(DbCmd),

    /// Run a health check (auto-aria2, ffmpeg, DanmakuFactory, B 站).
    Doctor,

    /// One-shot environment setup for analyze (python3, sensevoice, funasr, OCR models).
    Setup,

    /// Start an interactive REPL.
    Repl,
}

/// Frame-sampling strategy for the OCR subcommand.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleMode {
    /// Binary-search recursion with early-stop on duplicate content
    /// (fewest OCRs, no missed content).
    Adaptive,
    /// Fixed `--interval` seconds, exhaustive (more OCRs, more
    /// redundancy, but predictable).
    Linear,
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
