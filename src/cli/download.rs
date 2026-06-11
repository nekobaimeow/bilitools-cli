// SPDX-License-Identifier: GPL-3.0-or-later
// `download` subcommand.

use crate::cli::output::Output;
use crate::cli::root::DownloadCmd;
use crate::error::CliError;
use crate::ipc::bilibili_api;
use crate::ipc::media::parse;
use crate::ipc::storage::tasks::{self, Task, TaskStatus, TaskType};
use serde::Serialize;

pub async fn run(cmd: DownloadCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        DownloadCmd::Submit { input, output_dir, quality, abr } => {
            cmd_submit(&input, output_dir, quality, abr, out).await
        }
        DownloadCmd::Batch { file } => cmd_batch(&file, out).await,
        DownloadCmd::List { status } => cmd_list(status, out).await,
        DownloadCmd::Show { id } => cmd_show(&id, out).await,
        DownloadCmd::Cancel { id } => cmd_cancel(&id, out).await,
        DownloadCmd::Pause { id } => cmd_pause(&id, out).await,
        DownloadCmd::Resume { id } => cmd_resume(&id, out).await,
        DownloadCmd::Retry { id } => cmd_retry(&id, out).await,
        DownloadCmd::Open { id } => cmd_open(&id, out).await,
        DownloadCmd::Run { id } => cmd_run(&id, out).await,
        DownloadCmd::RunAll => cmd_run_all(out).await,
    }
}

#[derive(Serialize)]
struct SubmitOut {
    task_id: String,
    kind: String,
    id: String,
    status: String,
}

async fn cmd_submit(
    input: &str,
    output_dir: Option<std::path::PathBuf>,
    quality: Option<u32>,
    abr: Option<u32>,
    out: &Output,
) -> Result<(), CliError> {
    let res = parse(input)?;
    // Verify the resource is reachable AND pull the metadata (title,
    // cover, pages, available streams, …) so downstream code can act
    // on the task without re-fetching.
    let desc = bilibili_api::describe(&res).await?;
    let task = Task {
        id: uuid::Uuid::new_v4().to_string(),
        task_type: kind_to_task_type(res.kind),
        source: input.to_string(),
        options: serde_json::json!({
            "id": res.id,
            "kind": res.kind.as_str(),
            "output_dir": output_dir,
            "quality": quality,
            "abr": abr,
            "metadata": desc,
        }),
        status: TaskStatus::Pending,
        progress: 0.0,
        error: None,
        created_at: crate::ipc::shared::get_sec(),
        updated_at: crate::ipc::shared::get_sec(),
        completed_at: None,
    };
    tasks::insert(&task).await?;
    tasks::log_event(&task.id, "info", "submitted").await?;
    out.ok(SubmitOut {
        task_id: task.id,
        kind: res.kind.as_str().to_string(),
        id: res.id,
        status: task.status.as_str().to_string(),
    })
}

fn kind_to_task_type(k: crate::ipc::media::ResourceKind) -> TaskType {
    use crate::ipc::media::ResourceKind::*;
    match k {
        Video => TaskType::Video,
        Bangumi => TaskType::Bangumi,
        Episode => TaskType::Bangumi,
        Favorite => TaskType::Favorite,
        WatchLater => TaskType::WatchLater,
        Interactive => TaskType::Interactive,
        Audio => TaskType::Audio,
        Collection => TaskType::Other,
        Cheese => TaskType::Other,
        User => TaskType::Other,
        Live => TaskType::Other,
        Short => TaskType::Other,
        Unknown => TaskType::Other,
    }
}

async fn cmd_batch(file: &std::path::Path, out: &Output) -> Result<(), CliError> {
    let body = if file == &std::path::PathBuf::from("-") {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(file)?
    };
    let mut submitted = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Inline call to cmd_submit without re-routing through Output.
        let res = parse(line)?;
        let task = Task {
            id: uuid::Uuid::new_v4().to_string(),
            task_type: kind_to_task_type(res.kind),
            source: line.to_string(),
            options: serde_json::json!({"id": res.id, "kind": res.kind.as_str()}),
            status: TaskStatus::Pending,
            progress: 0.0,
            error: None,
            created_at: crate::ipc::shared::get_sec(),
            updated_at: crate::ipc::shared::get_sec(),
            completed_at: None,
        };
        tasks::insert(&task).await?;
        tasks::log_event(&task.id, "info", "submitted (batch)").await?;
        submitted.push(task.id);
    }
    out.ok(serde_json::json!({ "submitted": submitted }))
}

async fn cmd_list(filter: Option<String>, out: &Output) -> Result<(), CliError> {
    let all = tasks::load().await?;
    let rows: Vec<_> = if let Some(s) = filter {
        let s = TaskStatus::parse(&s)?;
        all.into_iter().filter(|t| t.status == s).collect()
    } else {
        all
    };
    let slim: Vec<_> = rows
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "type": t.task_type.as_str(),
                "source": t.source,
                "status": t.status.as_str(),
                "progress": t.progress,
                "error": t.error,
                "created_at": t.created_at,
            })
        })
        .collect();
    out.list(slim)
}

async fn cmd_show(id: &str, out: &Output) -> Result<(), CliError> {
    let t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    out.ok(serde_json::to_value(&t).unwrap_or(serde_json::Value::Null))
}

async fn cmd_cancel(id: &str, out: &Output) -> Result<(), CliError> {
    let mut t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    t.status = TaskStatus::Cancelled;
    t.updated_at = crate::ipc::shared::get_sec();
    tasks::update(&t).await?;
    tasks::log_event(id, "info", "cancelled").await?;
    out.status(format!("task {id} cancelled"))
}

async fn cmd_pause(id: &str, out: &Output) -> Result<(), CliError> {
    let mut t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    t.status = TaskStatus::Paused;
    t.updated_at = crate::ipc::shared::get_sec();
    tasks::update(&t).await?;
    tasks::log_event(id, "info", "paused").await?;
    out.status(format!("task {id} paused"))
}

async fn cmd_resume(id: &str, out: &Output) -> Result<(), CliError> {
    let mut t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    t.status = TaskStatus::Running;
    t.updated_at = crate::ipc::shared::get_sec();
    tasks::update(&t).await?;
    tasks::log_event(id, "info", "resumed").await?;
    out.status(format!("task {id} resumed"))
}

async fn cmd_retry(id: &str, out: &Output) -> Result<(), CliError> {
    let mut t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    t.status = TaskStatus::Pending;
    t.progress = 0.0;
    t.error = None;
    t.updated_at = crate::ipc::shared::get_sec();
    tasks::update(&t).await?;
    tasks::log_event(id, "info", "retry requested").await?;
    out.status(format!("task {id} re-queued"))
}

async fn cmd_open(id: &str, _out: &Output) -> Result<(), CliError> {
    let t = tasks::get(id)
        .await?
        .ok_or_else(|| CliError::TaskNotFound(id.to_string()))?;
    let out_dir = t
        .options
        .get("output_dir")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Downloads/bilitools"))
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
        });
    std::fs::create_dir_all(&out_dir).ok();
    crate::backends::http::open_path(&out_dir.to_string_lossy())?;
    Ok(())
}

async fn cmd_run(id: &str, out: &Output) -> Result<(), CliError> {
    let result = crate::ipc::queue::run_task(id).await?;
    out.ok(result)
}

async fn cmd_run_all(out: &Output) -> Result<(), CliError> {
    let all = tasks::load().await?;
    let mut results = Vec::new();
    for t in all {
        if matches!(
            t.status,
            TaskStatus::Pending | TaskStatus::Running | TaskStatus::Paused | TaskStatus::Failed
        ) {
            match crate::ipc::queue::run_task(&t.id).await {
                Ok(r) => results.push(r),
                Err(e) => results.push(crate::ipc::queue::RunResult {
                    task_id: t.id.clone(),
                    status: format!("error: {e}"),
                    output: None,
                    segments: vec![],
                    resumed: false,
                }),
            }
        }
    }
    out.ok(serde_json::json!({ "ran": results.len(), "results": results }))
}
