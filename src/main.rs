// SPDX-License-Identifier: GPL-3.0-or-later
// bilitools-cli binary entry point.

use bilitools::cli::output::{Output, OutputMode};
use bilitools::cli::root::{Cli, Command};
use bilitools::cli::{analyze, audio, auth, cache, config as cfg, danmaku, db as dbcmd, download, harvest, info, parse as par, repl, review, schedule, search, subtitle};
use bilitools::cli::ocr;
use bilitools::context;
use bilitools::doctor;
use bilitools::error::CliError;
use clap::Parser;

fn main() {
    let cli = Cli::parse();

    // 1. Initialize logging
    init_logging(&cli.log_level, cli.json);

    // 2. Apply data-dir override if requested
    if let Some(d) = &cli.data_dir {
        bilitools::ipc::storage::db::set_data_dir(Some(d.clone()))
            .expect("set_data_dir");
    }

    // 3. Initialize runtime (DB + settings + HTTP clients)
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("init tokio runtime");
    if let Err(e) = rt.block_on(async_run(cli)) {
        let mode = if std::env::args().any(|a| a == "--json" || a == "-j") {
            OutputMode::Json
        } else {
            OutputMode::Human
        };
        let out = Output::new(mode);
        let _ = out.err(&e);
        std::process::exit(1);
    }
}

async fn async_run(cli: Cli) -> Result<(), CliError> {
    let out = Output::new(if cli.json {
        OutputMode::Json
    } else {
        OutputMode::Human
    });

    // Always build the context first — it initializes the DB and settings.
    let _ctx = context::ctx().await?;

    if cli.doctor {
        let report = doctor::run().await?;
        out.ok(report)?;
        if matches!(cli.command, Some(Command::Doctor)) {
            return Ok(());
        }
    }
    let cmd = cli.command.unwrap_or(Command::Repl);
    match cmd {
        Command::Info => info::run(&out).await,
        Command::Init => cmd_init(&out).await,
        Command::Auth(c) => auth::run(c, &out).await,
        Command::Parse(c) => par::run(c, &out).await,
        Command::Download(c) => download::run(c, &out).await,
        Command::Schedule(c) => schedule::run(c, &out).await,
        Command::Config(c) => cfg::run(c, &out).await,
        Command::Cache(c) => cache::run(c, &out).await,
        Command::Db(c) => dbcmd::run(c, &out).await,
        Command::Search { .. } => search::run(&cmd, &out).await,
        Command::Danmaku { .. } => danmaku::run(&cmd, &out).await,
        Command::Review { .. } => review::run(&cmd, &out).await,
        Command::Subtitle { .. } => subtitle::run(&cmd, &out).await,
        Command::Harvest { .. } => harvest::run(&cmd, &out).await,
        Command::Audio { .. } => audio::run(&cmd, &out).await,
        Command::Analyze { .. } => analyze::run(&cmd, &out).await,
        Command::Ocr { .. } => ocr::run(&cmd, &out).await,
        Command::Doctor => {
            let report = doctor::run().await?;
            out.ok(report)
        }
        Command::Repl => repl::run(&out).await,
    }
}

async fn cmd_init(out: &Output) -> Result<(), CliError> {
    // Fingerprint cookies are already loaded by `context::ctx()` at startup
    // (see `AppContext::build`); this command now exists as a no-op alias
    // for backwards compatibility and to give the user a way to re-warm
    // the fingerprint after a long idle period.
    bilitools::ipc::shared::HEADERS.refresh().await?;
    let cookie = bilitools::ipc::shared::HEADERS.cookie().await;
    let summary = if cookie.contains("SESSDATA=") {
        "logged in + fingerprint present"
    } else {
        "fingerprint present (anonymous)"
    };
    out.status(summary)
}

fn init_logging(level: &str, _json: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("bilitools={level},bilitools_cli={level}")));
    fmt().with_env_filter(filter).with_target(false).init();
}
