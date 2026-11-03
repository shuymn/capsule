//! The worker's state decisions without I/O: which request is current, which
//! acquisition generation runs, which events apply, and what is displayed.
//!
//! [`Machine::step`] consumes one [`Input`] and returns the [`Effect`]s the I/O
//! shell in `worker.rs` performs. [`Machine::shown`] selects the display data
//! that the next response renders.

use std::sync::Arc;

use capsule_core::{
    acquire::AcquireError,
    git::{ContextInfo, local_directory},
    plan::{ConfigPlan, ModuleObservation, Observation},
};
use capsule_protocol::session::Request;

#[derive(Debug, Default)]
pub(super) struct LoadedPlan {
    pub(super) config: ConfigPlan,
    // Compare the bounded source rather than compiled regexes. An unchanged
    // source reuses the Arc, including when a rejected reload keeps this plan.
    pub(super) source: Option<String>,
}

pub(super) enum Change {
    Complete,
    Plan(Arc<LoadedPlan>, Option<String>),
    Context(Result<ContextInfo, AcquireError>),
    Condition(usize, Result<bool, AcquireError>),
    Value(usize, usize, Result<Option<String>, AcquireError>),
}

pub(super) struct Event {
    pub(super) generation: u64,
    pub(super) change: Change,
}

pub(super) enum Input {
    /// A decoded request from the shell.
    Request(Request),
    /// An acquisition event from a generation task.
    Event(Event),
    /// The generation task finished; `failed` when it panicked or was aborted.
    Joined { failed: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    /// Cancel the running generation task, if any.
    Cancel,
    /// Start a generation task for this request with the worker's current plan.
    Spawn(Arc<Request>, Arc<LoadedPlan>),
    /// Report a configuration diagnostic once per change.
    Diagnostic(String),
    /// Send a response for [`Machine::shown`]; every input but a stale request
    /// renders.
    Render,
}

impl PartialEq for LoadedPlan {
    /// Plans are compared by identity, as reload acceptance is.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for LoadedPlan {}

struct Display {
    modules: Vec<ModuleObservation>,
    context: ContextInfo,
}

struct State {
    request: Arc<Request>,
    plan: Arc<LoadedPlan>,
    modules: Vec<ModuleObservation>,
    context: ContextInfo,
    /// Display data available without acquisition.
    local: ContextInfo,
    // Only a settled generation may supply display data. Never feed retained
    // observations back into acquisition or turn new Pending states into Ready.
    retained: Option<Display>,
    complete: bool,
}

impl State {
    fn new(request: Arc<Request>, plan: Arc<LoadedPlan>, previous: Option<Self>) -> Self {
        let retained = previous
            .filter(|previous| previous.request.snapshot == request.snapshot)
            .and_then(|previous| {
                if previous.complete {
                    Some(Display {
                        modules: previous.modules,
                        context: previous.context,
                    })
                } else {
                    previous.retained
                }
            });
        Self {
            context: local_context(&request),
            local: local_context(&request),
            request,
            plan,
            modules: Vec::new(),
            retained,
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
        self.retained = None;
        self.complete = true;
    }
}

fn local_context(request: &Request) -> ContextInfo {
    ContextInfo {
        directory: local_directory(&request.snapshot),
        read_only: false,
        git: None,
    }
}

/// The display data a response renders, borrowed from the machine.
pub(super) struct Shown<'a> {
    pub(super) request: &'a Request,
    pub(super) plan: &'a LoadedPlan,
    pub(super) context: &'a ContextInfo,
    pub(super) modules: &'a [ModuleObservation],
    pub(super) complete: bool,
}

pub(super) struct Machine {
    state: Option<State>,
    /// The newest request whose generation has not started.
    pending: Option<Arc<Request>>,
    /// The worker's current plan: the last one a generation delivered.
    plan: Arc<LoadedPlan>,
    last_diagnostic: Option<String>,
    /// Whether a generation task runs; at most one does.
    running: bool,
    active_generation: u64,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            state: None,
            pending: None,
            plan: Arc::new(LoadedPlan::default()),
            last_diagnostic: None,
            running: false,
            active_generation: 0,
        }
    }
}

impl Machine {
    pub(super) fn step(&mut self, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut render = true;
        match input {
            Input::Request(request) => render = self.accept(request, &mut effects),
            Input::Event(event) => {
                // A settled generation is final. After a failed task settles it,
                // events it queued before failing can still arrive.
                if let Some(current) = self.state.as_mut()
                    && event.generation == current.request.generation
                    && !current.complete
                {
                    apply_change(
                        current,
                        event.change,
                        &mut self.plan,
                        &mut self.last_diagnostic,
                        &mut effects,
                    );
                }
            }
            Input::Joined { failed } => {
                self.running = false;
                if failed
                    && let Some(current) = self.state.as_mut()
                    && current.request.generation == self.active_generation
                {
                    current.finish();
                }
            }
        }
        // Start the newest pending request once the previous task has ended.
        if !self.running
            && let Some(request) = self.pending.take()
        {
            self.running = true;
            self.active_generation = request.generation;
            effects.push(Effect::Spawn(request, self.plan.clone()));
        }
        if render && self.state.is_some() {
            effects.push(Effect::Render);
        }
        effects
    }

    /// Accept a request; `false` when it is older than the current one.
    fn accept(&mut self, mut request: Request, effects: &mut Vec<Effect>) -> bool {
        // Environment order is not part of the acquisition input.
        request.snapshot.env.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        match self.state.as_mut() {
            Some(current) if request.generation < current.request.generation => return false,
            Some(current) if request.generation == current.request.generation => {
                let current_request = Arc::make_mut(&mut current.request);
                current_request.cols = request.cols;
                current_request.keymap = request.keymap;
                current_request.last_exit_code = request.last_exit_code;
                current_request.duration_ms = request.duration_ms;
            }
            _ => {
                effects.push(Effect::Cancel);
                let request = Arc::new(request);
                self.pending = Some(request.clone());
                self.state = Some(State::new(request, self.plan.clone(), self.state.take()));
            }
        }
        true
    }

    /// What the next response displays: the complete generation, else the
    /// retained settled display, else local data only.
    pub(super) fn shown(&self) -> Option<Shown<'_>> {
        let state = self.state.as_ref()?;
        let (context, modules) = if state.complete {
            (&state.context, state.modules.as_slice())
        } else if let Some(retained) = &state.retained {
            (&retained.context, retained.modules.as_slice())
        } else {
            (&state.local, &[][..])
        };
        Some(Shown {
            request: &state.request,
            plan: &state.plan,
            context,
            modules,
            complete: state.complete,
        })
    }
}

fn apply_change(
    state: &mut State,
    change: Change,
    plan: &mut Arc<LoadedPlan>,
    last_diagnostic: &mut Option<String>,
    effects: &mut Vec<Effect>,
) {
    match change {
        // The FIFO event follows all value events; joining the producer can race them.
        Change::Complete => state.finish(),
        Change::Plan(next, diagnostic) => {
            if diagnostic != *last_diagnostic {
                if let Some(message) = &diagnostic {
                    effects.push(Effect::Diagnostic(message.clone()));
                }
                *last_diagnostic = diagnostic;
            }
            if !Arc::ptr_eq(&state.plan, &next) {
                state.retained = None;
            }
            state.modules = next
                .config
                .modules
                .iter()
                .map(ModuleObservation::pending)
                .collect();
            state.plan = next.clone();
            *plan = next;
        }
        Change::Context(Ok(context)) => state.context = context,
        Change::Context(Err(error)) => tracing::debug!(%error, "context unavailable"),
        // A generation sends indexed events only after its Plan sized
        // `state.modules`; ignore any other index rather than trust that order.
        Change::Condition(index, value) => {
            if let Some(module) = state.modules.get_mut(index) {
                module.condition = match value {
                    Ok(value) => Observation::Ready(value),
                    Err(error) => Observation::Failed(error),
                };
            }
        }
        Change::Value(module, value, result) => {
            if let Some(slot) = state
                .modules
                .get_mut(module)
                .and_then(|module| module.values.get_mut(value))
            {
                *slot = match result {
                    Ok(Some(value)) => Observation::Ready(value),
                    Ok(None) => Observation::Missing,
                    Err(error) => Observation::Failed(error),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests;
