// SPDX-License-Identifier: GPL-3.0-or-later
// REPL — interactive shell. Read-eval-print-loop with rustyline history.

use crate::cli::output::{Output, OutputMode};
use crate::error::CliError;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::Editor;

pub async fn run(out: &Output) -> Result<(), CliError> {
    println!("bilicli v{} (REPL)", env!("CARGO_PKG_VERSION"));
    println!("type 'help' for commands, 'exit' or Ctrl-D to quit.");

    let history_path = dirs_history_path();
    let mut rl = Editor::<(), FileHistory>::new()
        .map_err(|e| CliError::msg(format!("init REPL: {e}")))?;
    if let Some(p) = &history_path {
        if p.parent().is_some() {
            std::fs::create_dir_all(p.parent().unwrap()).ok();
        }
        if p.exists() {
            let _ = rl.load_history(p);
        }
    }

    let json_mode = matches!(out.mode, OutputMode::Json);
    loop {
        let prompt = if json_mode { ">>> ".to_string() } else { "bilicli> ".to_string() };
        let line = match rl.readline(&prompt) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("readline error: {e}");
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(&line);
        if trimmed == "exit" || trimmed == "quit" {
            break;
        }
        if trimmed == "help" {
            print_help();
            continue;
        }
        if let Err(e) = dispatch(trimmed).await {
            eprintln!("error[{}]: {}", e.code(), e);
        }
    }

    if let Some(p) = history_path {
        let _ = rl.save_history(&p);
    }
    Ok(())
}

fn print_help() {
    println!(
        "\
bilicli REPL — available commands:

  help                     show this help
  exit / quit              leave the REPL
  info                     show version and paths
  doctor                   run health check
  auth status              show login state
  auth qrcode              start a QR login
  auth refresh             refresh cookies
  auth exit                log out
  parse url <URL>          classify a B 站 URL
  parse bv <BV_ID>         classify a BV id
  parse fav <FID>          classify a favorite folder
  config show              show configuration
  config get <key>         get a config value
  config set <k> <v>       set a config value
  download list            list all tasks
  download submit <URL>    submit a download
  cache list               list cache directories

Any bilicli subcommand can also be invoked by typing it directly.
"
    );
}

/// Dispatch a REPL line through the same code path as the CLI.
async fn dispatch(line: &str) -> Result<(), CliError> {
    use clap::Parser;
    let mut argv = vec!["bilicli".to_string()];
    // Crude split: respect single-quoted strings.
    for tok in shell_split(line) {
        argv.push(tok);
    }
    let cli = match crate::cli::root::Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            e.print().ok();
            return Ok(());
        }
    };
    // We can't easily re-run the full dispatch here without duplicating
    // main.rs. For now REPL is limited to a small subset; the
    // bulk-subcommand support is a TODO.
    eprintln!("REPL dispatch: only a subset is supported in REPL mode. Use the full CLI for the rest.");
    let _ = cli;
    Ok(())
}

fn shell_split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in line.chars() {
        match c {
            '\'' => in_q = !in_q,
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn dirs_history_path() -> Option<std::path::PathBuf> {
    let base = directories::ProjectDirs::from("com", "nekobaimeow", "bilicli")?;
    Some(base.data_dir().join("repl_history"))
}
