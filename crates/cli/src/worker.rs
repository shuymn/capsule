//! One shell owns one worker, one current generation and one pending replacement.

use std::{io, os::fd::AsFd, path::PathBuf, sync::Arc};

use capsule_core::{
    acquire::Runner,
    git::acquire_context,
    limits::{
        EVENT_QUEUE_CAPACITY, GENERATION_DRAIN_TIMEOUT, GENERATION_TIMEOUT, MAX_FILE_JOBS,
        RUNTIME_SHUTDOWN_TIMEOUT,
    },
    plan::{ConfigPlan, acquire_condition, acquire_value},
    view::{self, ViewInput},
};
use capsule_protocol::session::{self, FrameReader, Request, Snapshot};
use tokio::{
    io::AsyncWriteExt,
    sync::{Notify, mpsc, watch},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::pipe::Pipe;

pub fn run() -> anyhow::Result<()> {
    // Avoid the terminal's foreground process group: shell exit must reach EOF
    // cleanup instead of terminating the worker before it reaps its commands.
    rustix::process::setpgid(None, None)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(MAX_FILE_JOBS)
        .build()?;
    let result = runtime.block_on(serve());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    result
}

mod machine;

use self::machine::{Change, Effect, Event, Input, LoadedPlan, Machine, Shown};

/// Serialize the shown display data with the current local time.
fn render(shown: &Shown<'_>) -> Result<Vec<u8>, session::Error> {
    let now = time::OffsetDateTime::now_local().ok();
    let lines = view::render(
        &shown.plan.config,
        &ViewInput {
            directory: &shown.context.directory,
            read_only: shown.context.read_only,
            git: shown.context.git.as_ref(),
            modules: shown.modules,
            cols: usize::from(shown.request.cols),
            last_exit_code: shown.request.last_exit_code,
            duration_ms: shown.request.duration_ms,
            keymap: &shown.request.keymap,
            time: now.map(|now| (now.hour(), now.minute(), now.second())),
        },
    );
    session::response(
        shown.request.generation,
        &lines.left1,
        &lines.left2,
        shown.complete,
    )
}

struct Acquisition {
    request: Arc<Request>,
    plan: Arc<LoadedPlan>,
    runner: Runner,
    cancel: CancellationToken,
    tx: mpsc::Sender<Event>,
}

async fn serve() -> anyhow::Result<()> {
    let credit = Arc::new(Notify::new());
    let input = Pipe::new(
        io::stdin().as_fd().try_clone_to_owned()?,
        Some(credit.clone()),
    )?;
    let output = Pipe::new(io::stdout().as_fd().try_clone_to_owned()?, None)?;
    let mut frames = FrameReader::new(input, session::MAX_REQUEST);
    let stop = CancellationToken::new();
    let (render_tx, render_rx) = watch::channel(None);
    let mut writer = tokio::spawn(write_frames(output, render_rx, credit, stop.clone()));
    let (events_tx, mut events_rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
    let runner = Runner::default();
    let mut generations = JoinSet::new();
    let mut active_cancel = CancellationToken::new();
    let mut machine = Machine::default();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let result = async {
        loop {
            let input = tokio::select! {
                _ = terminate.recv() => break,
                _ = hangup.recv() => break,
                result = &mut writer => { result??; break; }
                frame = frames.next() => {
                    let Some(frame) = frame? else { break; };
                    Input::Request(Request::decode(&frame)?)
                }
                event = events_rx.recv() => {
                    // `serve` holds a sender, so the channel never closes.
                    let Some(event) = event else { continue; };
                    Input::Event(event)
                }
                result = generations.join_next(), if !generations.is_empty() => {
                    let failed = matches!(result, Some(Err(_)));
                    if let Some(Err(error)) = result {
                        tracing::error!(%error, "generation task failed");
                    }
                    Input::Joined { failed }
                }
            };
            for effect in machine.step(input) {
                match effect {
                    Effect::Cancel => active_cancel.cancel(),
                    Effect::Spawn(request, plan) => {
                        active_cancel = CancellationToken::new();
                        generations.spawn(acquire_generation(
                            request,
                            plan,
                            runner.clone(),
                            active_cancel.clone(),
                            events_tx.clone(),
                        ));
                    }
                    Effect::Diagnostic(message) => {
                        eprintln!("capsule: {message}; retaining the last valid configuration");
                    }
                    Effect::Render => {
                        if let Some(shown) = machine.shown() {
                            render_tx.send_replace(Some(render(&shown)?));
                        }
                    }
                }
            }
        }
        anyhow::Ok(())
    }
    .await;
    active_cancel.cancel();
    stop.cancel();
    if tokio::time::timeout(GENERATION_DRAIN_TIMEOUT, async {
        while generations.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        generations.shutdown().await;
    }
    // The writer stays detached: after `stop` it ends without finishing a frame,
    // and runtime shutdown drops it.
    result
}

/// Deliver an event unless the generation is cancelled first; report delivery.
async fn send_event(
    tx: &mpsc::Sender<Event>,
    generation: u64,
    change: Change,
    cancel: &CancellationToken,
) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        result = tx.send(Event { generation, change }) => result.is_ok(),
    }
}

async fn acquire_generation(
    request: Arc<Request>,
    previous: Arc<LoadedPlan>,
    runner: Runner,
    cancel: CancellationToken,
    tx: mpsc::Sender<Event>,
) {
    // Identical commands in this generation share one execution.
    let runner = runner.deduplicating();
    let work = async {
        let (plan, diagnostic) =
            match load_plan(&request.snapshot, &runner, &cancel, &previous).await {
                Ok(plan) => (plan, None),
                Err(error) => (previous, Some(error.to_string())),
            };
        // Indexed module events require their Plan; without it, acquire nothing.
        if !send_event(
            &tx,
            request.generation,
            Change::Plan(plan.clone(), diagnostic),
            &cancel,
        )
        .await
        {
            return;
        }
        let mut jobs = JoinSet::new();
        let (context_request, context_runner, context_cancel, context_tx) =
            (request.clone(), runner.clone(), cancel.clone(), tx.clone());
        let git_enabled = !plan.config.view.git.disabled;
        jobs.spawn(async move {
            let result = acquire_context(
                &context_request.snapshot,
                &context_runner,
                &context_cancel,
                git_enabled,
            )
            .await;
            send_event(
                &context_tx,
                context_request.generation,
                Change::Context(result),
                &context_cancel,
            )
            .await;
        });
        let acquisition = Arc::new(Acquisition {
            request: request.clone(),
            plan,
            runner,
            cancel: cancel.clone(),
            tx: tx.clone(),
        });
        for index in 0..acquisition.plan.config.modules.len() {
            jobs.spawn(acquire_module(index, acquisition.clone()));
        }
        while let Some(result) = jobs.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "acquisition task failed");
            }
        }
    };
    tokio::pin!(work);
    tokio::select! {
        () = &mut work => {},
        () = tokio::time::sleep(GENERATION_TIMEOUT) => { cancel.cancel(); work.await; }
    }
    let _ = tx
        .send(Event {
            generation: request.generation,
            change: Change::Complete,
        })
        .await;
}

async fn acquire_module(index: usize, acquisition: Arc<Acquisition>) {
    let Acquisition {
        request,
        plan,
        runner,
        cancel,
        tx,
    } = acquisition.as_ref();
    let condition = acquire_condition(
        &plan.config.modules[index].when,
        &request.snapshot,
        runner,
        cancel,
    )
    .await;
    let matches = matches!(condition, Ok(true));
    send_event(
        tx,
        request.generation,
        Change::Condition(index, condition),
        cancel,
    )
    .await;
    if !matches {
        return;
    }
    let mut values = JoinSet::new();
    for value_index in 0..plan.config.modules[index].values.len() {
        let acquisition = acquisition.clone();
        values.spawn(async move {
            let Acquisition {
                request,
                plan,
                runner,
                cancel,
                tx,
            } = acquisition.as_ref();
            let value = acquire_value(
                &plan.config.modules[index].values[value_index],
                &request.snapshot,
                runner,
                cancel,
            )
            .await;
            send_event(
                tx,
                request.generation,
                Change::Value(index, value_index, value),
                cancel,
            )
            .await;
        });
    }
    while let Some(result) = values.join_next().await {
        if let Err(error) = result {
            tracing::error!(%error, "value task failed");
        }
    }
}

fn config_paths(snapshot: &Snapshot) -> Vec<PathBuf> {
    if let Some(path) = snapshot.env("XDG_CONFIG_HOME") {
        return vec![PathBuf::from(path).join("capsule/config.toml")];
    }
    snapshot.env("HOME").map_or_else(Vec::new, |home| {
        let home = PathBuf::from(home);
        vec![
            home.join(".config/capsule/config.toml"),
            home.join(".capsule/config.toml"),
        ]
    })
}

async fn load_plan(
    snapshot: &Snapshot,
    runner: &Runner,
    cancel: &CancellationToken,
    previous: &Arc<LoadedPlan>,
) -> anyhow::Result<Arc<LoadedPlan>> {
    let mut source = None;
    for path in config_paths(snapshot) {
        let path = snapshot.cwd().join(path);
        if let Some(bytes) = runner.file(&path, cancel).await? {
            source = Some(String::from_utf8(bytes)?);
            break;
        }
    }
    if source == previous.source {
        return Ok(previous.clone());
    }
    let config = source
        .as_deref()
        .map(ConfigPlan::parse)
        .transpose()?
        .unwrap_or_default();
    Ok(Arc::new(LoadedPlan { config, source }))
}

async fn write_frames(
    mut output: Pipe,
    mut rx: watch::Receiver<Option<Vec<u8>>>,
    credit: Arc<Notify>,
    stop: CancellationToken,
) -> io::Result<()> {
    loop {
        let frame = tokio::select! {
            () = stop.cancelled() => return Ok(()),
            () = credit.notified() => b"K\n".to_vec(),
            result = rx.changed() => {
                if result.is_err() { return Ok(()); }
                let Some(frame) = rx.borrow_and_update().clone() else { continue; };
                frame
            }
        };
        // Do not cancel a partly transmitted response when a newer one arrives.
        tokio::select! { () = stop.cancelled() => return Ok(()), result = output.write_all(&frame) => result? }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one generation and return its events in delivery order.
    async fn generation_events(cancelled: bool) -> anyhow::Result<Vec<Change>> {
        let request = Request::decode(b"Q\t1\t0\t\t80\tmain\t/")?;
        let config = ConfigPlan::parse(
            r#"
schema_version = 2
[[module]]
name = "tool"
format = "{value}"
[module.values]
value = [{ env = "TOOL" }]
"#,
        )?;
        let plan = Arc::new(LoadedPlan {
            config,
            source: None,
        });
        let cancel = CancellationToken::new();
        if cancelled {
            cancel.cancel();
        }
        let (tx, mut rx) = mpsc::channel(64);
        acquire_generation(Arc::new(request), plan, Runner::default(), cancel, tx).await;
        let mut changes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            changes.push(event.change);
        }
        Ok(changes)
    }

    #[tokio::test]
    async fn module_events_follow_a_delivered_plan_even_when_cancelled() -> anyhow::Result<()> {
        // A cancelled send races delivery at random, so repeat to cover both outcomes.
        for cancelled in std::iter::once(false).chain([true; 32]) {
            let changes = generation_events(cancelled).await?;
            let plan = changes
                .iter()
                .position(|change| matches!(change, Change::Plan(..)));
            for (index, change) in changes.iter().enumerate() {
                if matches!(change, Change::Condition(..) | Change::Value(..)) {
                    assert!(plan.is_some_and(|plan| plan < index));
                }
            }
            assert!(matches!(changes.last(), Some(Change::Complete)));
            if !cancelled {
                assert!(
                    changes
                        .iter()
                        .any(|change| matches!(change, Change::Condition(..)))
                );
            }
        }
        Ok(())
    }

    /// A generation-1 request at `home` that exports only `HOME`.
    fn home_request(home: &std::path::Path) -> anyhow::Result<Request> {
        let home = home.display();
        Ok(Request::decode(
            format!("Q\t1\t0\t\t80\tmain\t{home}\tHOME\t{home}").as_bytes(),
        )?)
    }

    #[tokio::test]
    async fn removing_the_configuration_accepts_the_defaults() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let previous = Arc::new(LoadedPlan {
            config: ConfigPlan::default(),
            source: Some("removed".to_owned()),
        });
        let request = home_request(home.path())?;
        let cancel = CancellationToken::new();
        let plan = load_plan(&request.snapshot, &Runner::default(), &cancel, &previous).await?;
        assert!(plan.source.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_fresh_worker_keeps_defaults_and_reports_an_invalid_configuration()
    -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        std::fs::create_dir(home.path().join(".capsule"))?;
        std::fs::write(
            home.path().join(".capsule/config.toml"),
            "schema_version = 2\nunknown = true\n",
        )?;
        let defaults = Arc::new(LoadedPlan::default());
        let (tx, mut rx) = mpsc::channel(64);
        let request = home_request(home.path())?;
        acquire_generation(
            Arc::new(request),
            defaults.clone(),
            Runner::default(),
            CancellationToken::new(),
            tx,
        )
        .await;
        let Some(Event {
            change: Change::Plan(plan, Some(reason)),
            ..
        }) = rx.recv().await
        else {
            anyhow::bail!("the first event is not a Plan with a diagnostic");
        };
        assert!(Arc::ptr_eq(&plan, &defaults));
        assert!(reason.contains("unknown"), "{reason}");
        Ok(())
    }
}
