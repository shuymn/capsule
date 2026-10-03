//! One shell owns one worker, one current generation and one pending replacement.

use std::{io, os::fd::AsFd, path::PathBuf, sync::Arc, time::Duration};

use capsule_core::{
    acquire::{AcquireError, Runner},
    git::{ContextInfo, acquire_context, local_directory},
    plan::{ConfigPlan, ModuleObservation, Observation, acquire_condition, acquire_value},
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

const GENERATION_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run() -> anyhow::Result<()> {
    // Avoid the terminal's foreground process group: shell exit must reach EOF
    // cleanup instead of terminating the worker before it reaps its commands.
    rustix::process::setpgid(None, None)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(2)
        .build()?;
    let result = runtime.block_on(serve());
    runtime.shutdown_timeout(Duration::from_millis(500));
    result
}

struct State {
    request: Arc<Request>,
    plan: Arc<ConfigPlan>,
    modules: Vec<ModuleObservation>,
    context: ContextInfo,
    complete: bool,
}

impl State {
    fn new(request: Arc<Request>, plan: Arc<ConfigPlan>) -> Self {
        let context = ContextInfo {
            directory: local_directory(&request.snapshot),
            read_only: false,
            git: None,
        };
        Self {
            request,
            plan,
            modules: Vec::new(),
            context,
            complete: false,
        }
    }

    fn finish(&mut self) {
        for module in &mut self.modules {
            if matches!(module.condition, Observation::Pending) {
                module.condition = Observation::Failed(AcquireError::TimedOut);
            }
            for value in &mut module.values {
                if matches!(value, Observation::Pending) {
                    *value = Observation::Failed(AcquireError::TimedOut);
                }
            }
        }
        self.complete = true;
    }

    fn render(&self) -> Result<Vec<u8>, session::Error> {
        let now = time::OffsetDateTime::now_local().ok();
        let lines = view::render(
            &self.plan,
            &ViewInput {
                directory: &self.context.directory,
                read_only: self.context.read_only,
                git: self.context.git.as_ref(),
                modules: &self.modules,
                cols: usize::from(self.request.cols),
                last_exit_code: self.request.last_exit_code,
                duration_ms: self.request.duration_ms,
                keymap: &self.request.keymap,
                time: now.map(|now| (now.hour(), now.minute(), now.second())),
            },
        );
        session::response(
            self.request.generation,
            &lines.left1,
            &lines.left2,
            self.complete,
        )
    }
}

enum Change {
    Complete,
    Plan(Arc<ConfigPlan>, Option<String>),
    Context(Result<ContextInfo, AcquireError>),
    Condition(usize, Result<bool, AcquireError>),
    Value(usize, usize, Result<Option<String>, AcquireError>),
}

struct Event {
    generation: u64,
    change: Change,
}

struct Acquisition {
    request: Arc<Request>,
    plan: Arc<ConfigPlan>,
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
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let runner = Runner::default();
    let mut generations = JoinSet::new();
    let mut active_cancel = CancellationToken::new();
    let mut active_generation = 0;
    let mut pending: Option<Arc<Request>> = None;
    let mut state: Option<State> = None;
    let mut plan = Arc::new(ConfigPlan::default());
    let mut last_diagnostic = None;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let result = async {
        loop {
            if generations.is_empty() && let Some(request) = pending.take() {
                active_cancel = CancellationToken::new();
                active_generation = request.generation;
                generations.spawn(acquire_generation(request, plan.clone(), runner.clone(), active_cancel.clone(), events_tx.clone()));
            }
            tokio::select! {
                _ = terminate.recv() => break,
                _ = hangup.recv() => break,
                result = &mut writer => { result??; break; }
                frame = frames.next() => {
                    let Some(frame) = frame? else { break; };
                    let request = Request::decode(&frame)?;
                    match state.as_mut() {
                        Some(current) if request.generation < current.request.generation => continue,
                        Some(current) if request.generation == current.request.generation => {
                            let current_request = Arc::make_mut(&mut current.request);
                            current_request.cols = request.cols;
                            current_request.keymap = request.keymap;
                        }
                        _ => {
                            active_cancel.cancel();
                            let request = Arc::new(request);
                            pending = Some(request.clone());
                            state = Some(State::new(request, plan.clone()));
                        }
                    }
                }
                event = events_rx.recv() => {
                    if let Some(event) = event && let Some(current) = state.as_mut() && event.generation == current.request.generation {
                        apply_change(current, event.change, &mut plan, &mut last_diagnostic);
                    }
                }
                result = generations.join_next(), if !generations.is_empty() => {
                    if let Some(Err(error)) = result {
                        tracing::error!(%error, "generation task failed");
                        if let Some(current) = state.as_mut() && current.request.generation == active_generation {
                            current.finish();
                        }
                    }
                }
            }
            if let Some(current) = &state { render_tx.send_replace(Some(current.render()?)); }
        }
        anyhow::Ok(())
    }.await;
    active_cancel.cancel();
    stop.cancel();
    if tokio::time::timeout(Duration::from_secs(1), async {
        while generations.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        generations.shutdown().await;
    }
    if !writer.is_finished() {
        writer.await??;
    }
    result
}

fn apply_change(
    state: &mut State,
    change: Change,
    plan: &mut Arc<ConfigPlan>,
    last_diagnostic: &mut Option<String>,
) {
    match change {
        // The FIFO event follows all value events; joining the producer can race them.
        Change::Complete => state.finish(),
        Change::Plan(next, diagnostic) => {
            if diagnostic != *last_diagnostic {
                if let Some(message) = &diagnostic {
                    eprintln!("capsule: {message}; retaining the last valid configuration");
                }
                *last_diagnostic = diagnostic;
            }
            state.modules = next
                .modules
                .iter()
                .map(ModuleObservation::pending)
                .collect();
            state.plan = next.clone();
            *plan = next;
        }
        Change::Context(Ok(context)) => state.context = context,
        Change::Context(Err(error)) => tracing::debug!(%error, "context unavailable"),
        Change::Condition(index, value) => {
            state.modules[index].condition = match value {
                Ok(value) => Observation::Ready(value),
                Err(error) => Observation::Failed(error),
            }
        }
        Change::Value(module, value, result) => {
            state.modules[module].values[value] = match result {
                Ok(Some(value)) => Observation::Ready(value),
                Ok(None) => Observation::Missing,
                Err(error) => Observation::Failed(error),
            }
        }
    }
}

async fn send_event(
    tx: &mpsc::Sender<Event>,
    generation: u64,
    change: Change,
    cancel: &CancellationToken,
) {
    tokio::select! { () = cancel.cancelled() => {}, _ = tx.send(Event { generation, change }) => {} }
}

async fn acquire_generation(
    request: Arc<Request>,
    previous: Arc<ConfigPlan>,
    runner: Runner,
    cancel: CancellationToken,
    tx: mpsc::Sender<Event>,
) {
    let work = async {
        let (plan, diagnostic) = match load_plan(&request.snapshot, &runner, &cancel).await {
            Ok(plan) => (Arc::new(plan), None),
            Err(error) => (previous, Some(error.to_string())),
        };
        send_event(
            &tx,
            request.generation,
            Change::Plan(plan.clone(), diagnostic),
            &cancel,
        )
        .await;
        let mut jobs = JoinSet::new();
        let (context_request, context_runner, context_cancel, context_tx) =
            (request.clone(), runner.clone(), cancel.clone(), tx.clone());
        let git_enabled = !plan.view.git.disabled;
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
        for index in 0..acquisition.plan.modules.len() {
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
    let condition =
        acquire_condition(&plan.modules[index].when, &request.snapshot, runner, cancel).await;
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
    for value_index in 0..plan.modules[index].values.len() {
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
                &plan.modules[index].values[value_index],
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
) -> anyhow::Result<ConfigPlan> {
    for path in config_paths(snapshot) {
        let path = snapshot.cwd.join(path);
        if let Some(bytes) = runner.file(&path, cancel).await? {
            return Ok(ConfigPlan::parse(std::str::from_utf8(&bytes)?)?);
        }
    }
    Ok(ConfigPlan::default())
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
