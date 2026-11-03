//! Model-based tests: random interleavings of requests, events, and task
//! completion against a reference model written from the behavioral contract
//! in docs/architecture.md.

use std::path::PathBuf;

use capsule_protocol::session::{self, Snapshot};
use proptest::prelude::*;

use super::*;

/// Request inputs that only re-render: columns, keymap, status, and duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Local {
    cols: u16,
    vicmd: bool,
    status: i32,
    duration: Option<u64>,
}

/// A value acquisition result.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    Ready(u8),
    Missing,
    Failed,
}

/// One input. Generations are generated relative to the current one (see
/// [`Op::resolve`]) so that current, stale, and future ones all occur often.
#[derive(Debug, Clone)]
enum Op {
    Request {
        generation: u64,
        cwd: u8,
        local: Local,
    },
    Plan {
        generation: u64,
        plan: usize,
        diagnostic: Option<u8>,
    },
    Context {
        generation: u64,
        ok: bool,
    },
    Condition {
        generation: u64,
        module: usize,
        result: Option<bool>,
    },
    Value {
        generation: u64,
        module: usize,
        value: usize,
        result: Outcome,
    },
    Complete {
        generation: u64,
    },
    Joined {
        failed: bool,
    },
}

impl Op {
    /// Turn the generated relative generation into an absolute one: `0` is the
    /// current generation, `1` the one before it, and `2` the one after it.
    fn resolve(&self, current: Option<u64>) -> Self {
        let current = current.unwrap_or(0);
        let absolute = |relative: u64| match relative {
            0 => current,
            1 => current.saturating_sub(1),
            _ => current + 1,
        };
        let mut op = self.clone();
        match &mut op {
            Self::Request { generation, .. }
            | Self::Plan { generation, .. }
            | Self::Context { generation, .. }
            | Self::Condition { generation, .. }
            | Self::Value { generation, .. }
            | Self::Complete { generation } => *generation = absolute(*generation),
            Self::Joined { .. } => {}
        }
        op
    }

    /// The generation an acquisition event belongs to.
    const fn event_generation(&self) -> Option<u64> {
        match self {
            Self::Plan { generation, .. }
            | Self::Context { generation, .. }
            | Self::Condition { generation, .. }
            | Self::Value { generation, .. }
            | Self::Complete { generation } => Some(*generation),
            Self::Request { .. } | Self::Joined { .. } => None,
        }
    }
}

fn op() -> impl Strategy<Value = Op> {
    // Relative generations; see `Op::resolve`. Events mostly target the
    // current generation and requests mostly start the next one.
    let generation = prop_oneof![6 => Just(0u64), 1 => Just(1), 1 => Just(2)];
    let request_generation = prop_oneof![2 => Just(0u64), 1 => Just(1), 3 => Just(2)];
    let local = (0..3u16, any::<bool>(), 0..2i32, prop::option::of(0..2u64)).prop_map(
        |(cols, vicmd, status, duration)| Local {
            cols,
            vicmd,
            status,
            duration,
        },
    );
    let outcome = prop_oneof![
        (0..2u8).prop_map(Outcome::Ready),
        Just(Outcome::Missing),
        Just(Outcome::Failed),
    ];
    prop_oneof![
        3 => (request_generation, 0..2u8, local)
            .prop_map(|(generation, cwd, local)| Op::Request { generation, cwd, local }),
        2 => (generation.clone(), 0..3usize, prop::option::of(0..2u8))
            .prop_map(|(generation, plan, diagnostic)| Op::Plan { generation, plan, diagnostic }),
        1 => (generation.clone(), any::<bool>()).prop_map(|(generation, ok)| Op::Context { generation, ok }),
        3 => (generation.clone(), 0..3usize, prop::option::of(any::<bool>()))
            .prop_map(|(generation, module, result)| Op::Condition { generation, module, result }),
        3 => (generation.clone(), 0..3usize, 0..3usize, outcome)
            .prop_map(|(generation, module, value, result)| Op::Value { generation, module, value, result }),
        2 => generation.prop_map(|generation| Op::Complete { generation }),
        2 => any::<bool>().prop_map(|failed| Op::Joined { failed }),
    ]
}

// Encodings shared by the fixture and the model.

fn cwd_path(cwd: u8) -> PathBuf {
    PathBuf::from(format!("/cwd{cwd}"))
}

fn keymap_name(vicmd: bool) -> &'static str {
    if vicmd { "vicmd" } else { "main" }
}

fn value_text(code: u8) -> String {
    format!("v{code}")
}

fn diagnostic_text(code: u8) -> String {
    format!("invalid {code}")
}

/// A comparable summary of an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Pending,
    Ready(String),
    Missing,
    Failed,
}

fn seen<T: ToString>(observation: &Observation<T>) -> Seen {
    match observation {
        Observation::Pending => Seen::Pending,
        Observation::Ready(value) => Seen::Ready(value.to_string()),
        Observation::Missing => Seen::Missing,
        Observation::Failed(_) => Seen::Failed,
    }
}

type Modules = Vec<(Seen, Vec<Seen>)>;

/// What a response shows, in comparable form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct View {
    generation: u64,
    cwd: PathBuf,
    local: Local,
    plan: usize,
    directory: String,
    acquired: bool,
    modules: Modules,
    complete: bool,
}

/// Effects in comparable form: plans by fixture index.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Did {
    Cancel,
    Spawn(u64, usize),
    Diagnostic(String),
    Render,
}

struct Fixture {
    plans: [Arc<LoadedPlan>; 3],
}

impl Fixture {
    fn new(default: Arc<LoadedPlan>) -> Result<Self, capsule_core::plan::ConfigError> {
        let plan = |source: &str| -> Result<Arc<LoadedPlan>, capsule_core::plan::ConfigError> {
            Ok(Arc::new(LoadedPlan {
                config: ConfigPlan::parse(source)?,
                source: Some(source.to_owned()),
            }))
        };
        Ok(Self {
            plans: [
                default,
                plan(
                    "schema_version = 2\n[[module]]\nname = 'a'\nformat = '{x}'\n[module.values]\nx = [{ env = 'X' }]\n",
                )?,
                plan(
                    "schema_version = 2\n[[module]]\nname = 'a'\nformat = '{x}{y}'\n[module.values]\nx = [{ env = 'X' }]\ny = [{ env = 'Y' }]\n\
                     [[module]]\nname = 'b'\nformat = '{x}{y}'\n[module.values]\nx = [{ env = 'X' }]\ny = [{ env = 'Y' }]\n",
                )?,
            ],
        })
    }

    fn index(&self, plan: &LoadedPlan) -> usize {
        self.plans
            .iter()
            .position(|candidate| std::ptr::eq(candidate.as_ref(), plan))
            .unwrap_or(usize::MAX)
    }

    fn shape(&self, plan: usize) -> Vec<usize> {
        self.plans[plan]
            .config
            .modules
            .iter()
            .map(|module| module.values.len())
            .collect()
    }

    fn input(&self, op: &Op) -> Result<Input, session::Error> {
        let event = |generation, change| Input::Event(Event { generation, change });
        Ok(match *op {
            Op::Request {
                generation,
                cwd,
                local,
            } => Input::Request(request(generation, cwd, local)?),
            Op::Plan {
                generation,
                plan,
                diagnostic,
            } => event(
                generation,
                Change::Plan(self.plans[plan].clone(), diagnostic.map(diagnostic_text)),
            ),
            Op::Context { generation, ok } => event(
                generation,
                Change::Context(if ok {
                    Ok(acquired_context())
                } else {
                    Err(AcquireError::TimedOut)
                }),
            ),
            Op::Condition {
                generation,
                module,
                result,
            } => event(
                generation,
                Change::Condition(module, result.ok_or(AcquireError::TimedOut)),
            ),
            Op::Value {
                generation,
                module,
                value,
                result,
            } => event(
                generation,
                Change::Value(
                    module,
                    value,
                    match result {
                        Outcome::Ready(code) => Ok(Some(value_text(code))),
                        Outcome::Missing => Ok(None),
                        Outcome::Failed => Err(AcquireError::TimedOut),
                    },
                ),
            ),
            Op::Complete { generation } => event(generation, Change::Complete),
            Op::Joined { failed } => Input::Joined { failed },
        })
    }

    fn did(&self, effect: &Effect) -> Did {
        match effect {
            Effect::Cancel => Did::Cancel,
            Effect::Spawn(request, plan) => Did::Spawn(request.generation, self.index(plan)),
            Effect::Diagnostic(message) => Did::Diagnostic(message.clone()),
            Effect::Render => Did::Render,
        }
    }

    fn view(&self, machine: &Machine) -> Option<View> {
        let shown = machine.shown()?;
        Some(View {
            generation: shown.request.generation,
            cwd: shown.request.snapshot.cwd().to_owned(),
            local: Local {
                cols: shown.request.cols,
                vicmd: shown.request.keymap == keymap_name(true),
                status: shown.request.last_exit_code,
                duration: shown.request.duration_ms,
            },
            plan: self.index(shown.plan),
            directory: shown.context.directory.clone(),
            acquired: shown.context.read_only,
            modules: shown
                .modules
                .iter()
                .map(|module| {
                    (
                        seen(&module.condition),
                        module.values.iter().map(seen).collect(),
                    )
                })
                .collect(),
            complete: shown.complete,
        })
    }
}

fn request(generation: u64, cwd: u8, local: Local) -> Result<Request, session::Error> {
    Ok(Request {
        generation,
        snapshot: Snapshot::new(cwd_path(cwd), Vec::new())?,
        cols: local.cols,
        last_exit_code: local.status,
        duration_ms: local.duration,
        keymap: keymap_name(local.vicmd).to_owned(),
    })
}

/// Acquired context is marked read-only so it is distinguishable from local data.
fn acquired_context() -> ContextInfo {
    ContextInfo {
        directory: "acquired".to_owned(),
        read_only: true,
        git: None,
    }
}

/// Observations and context of one generation.
#[derive(Debug, Clone)]
struct Settled {
    modules: Modules,
    directory: String,
    acquired: bool,
}

fn local_only(cwd: u8) -> Settled {
    Settled {
        modules: Vec::new(),
        directory: cwd_path(cwd).display().to_string(),
        acquired: false,
    }
}

#[derive(Debug, Clone)]
struct Current {
    generation: u64,
    cwd: u8,
    local: Local,
    plan: usize,
    observed: Settled,
    complete: bool,
    retained: Option<Settled>,
    started: bool,
}

/// The contract restated, independently of the implementation.
#[derive(Debug, Clone, Default)]
struct Model {
    current: Option<Current>,
    /// The worker's plan: the last one a generation of this worker delivered.
    plan: usize,
    /// The generation whose task runs.
    running: Option<u64>,
    last_diagnostic: Option<String>,
}

impl Model {
    fn step(&mut self, op: &Op, fixture: &Fixture) -> Vec<Did> {
        let mut did = Vec::new();
        let mut render = true;
        let current_generation = self.current.as_ref().map(|current| current.generation);
        let settled = self
            .current
            .as_ref()
            .is_some_and(|current| current.complete);
        match *op {
            // WHEN a request is older than the current one, it SHALL be ignored.
            Op::Request { generation, .. }
                if current_generation.is_some_and(|current| generation < current) =>
            {
                render = false;
            }
            // WHEN a request repeats the current generation, only local inputs SHALL change.
            Op::Request {
                generation, local, ..
            } if current_generation == Some(generation) => {
                if let Some(current) = self.current.as_mut() {
                    current.local = local;
                }
            }
            // WHEN a newer request arrives, the running generation SHALL be cancelled;
            // a settled display SHALL be retained only for an identical snapshot.
            Op::Request {
                generation,
                cwd,
                local,
            } => {
                did.push(Did::Cancel);
                let retained = self
                    .current
                    .take()
                    .filter(|previous| previous.cwd == cwd)
                    .and_then(|previous| {
                        if previous.complete {
                            Some(previous.observed)
                        } else {
                            previous.retained
                        }
                    });
                self.current = Some(Current {
                    generation,
                    cwd,
                    local,
                    plan: self.plan,
                    observed: local_only(cwd),
                    complete: false,
                    retained,
                    started: false,
                });
            }
            // WHEN an event belongs to another generation, or to one that has
            // settled, it SHALL change nothing.
            _ if op
                .event_generation()
                .is_some_and(|generation| current_generation != Some(generation) || settled) => {}
            Op::Plan {
                plan, diagnostic, ..
            } => {
                let diagnostic = diagnostic.map(diagnostic_text);
                if diagnostic != self.last_diagnostic {
                    did.extend(diagnostic.clone().map(Did::Diagnostic));
                    self.last_diagnostic = diagnostic;
                }
                if let Some(current) = self.current.as_mut() {
                    // An accepted plan change SHALL discard retained display.
                    if plan != current.plan {
                        current.retained = None;
                    }
                    current.plan = plan;
                    current.observed.modules = fixture
                        .shape(plan)
                        .into_iter()
                        .map(|values| (Seen::Pending, vec![Seen::Pending; values]))
                        .collect();
                }
                self.plan = plan;
            }
            Op::Context { ok, .. } => {
                if ok && let Some(current) = self.current.as_mut() {
                    current.observed.directory = "acquired".to_owned();
                    current.observed.acquired = true;
                }
            }
            Op::Condition { module, result, .. } => {
                if let Some(slot) = self
                    .current
                    .as_mut()
                    .and_then(|current| current.observed.modules.get_mut(module))
                {
                    slot.0 = result.map_or(Seen::Failed, |value| Seen::Ready(value.to_string()));
                }
            }
            Op::Value {
                module,
                value,
                result,
                ..
            } => {
                if let Some(slot) = self
                    .current
                    .as_mut()
                    .and_then(|current| current.observed.modules.get_mut(module))
                    .and_then(|module| module.1.get_mut(value))
                {
                    *slot = match result {
                        Outcome::Ready(code) => Seen::Ready(value_text(code)),
                        Outcome::Missing => Seen::Missing,
                        Outcome::Failed => Seen::Failed,
                    };
                }
            }
            Op::Complete { .. } => self.settle(),
            Op::Joined { failed } => {
                if self.running.take() == current_generation && failed {
                    self.settle();
                }
            }
        }
        // At most one generation SHALL run; the newest request starts next.
        if self.running.is_none()
            && let Some(current) = self.current.as_mut()
            && !current.started
        {
            current.started = true;
            self.running = Some(current.generation);
            did.push(Did::Spawn(current.generation, self.plan));
        }
        if render && self.current.is_some() {
            did.push(Did::Render);
        }
        did
    }

    /// WHEN a generation completes, unresolved observations SHALL become
    /// failures and its observations SHALL replace retained display.
    fn settle(&mut self) {
        if let Some(current) = self.current.as_mut() {
            for (condition, values) in &mut current.observed.modules {
                for slot in std::iter::once(condition).chain(values.iter_mut()) {
                    if *slot == Seen::Pending {
                        *slot = Seen::Failed;
                    }
                }
            }
            current.retained = None;
            current.complete = true;
        }
    }

    fn view(&self) -> Option<View> {
        let current = self.current.as_ref()?;
        // Before completion, show retained settled display or local data only.
        let shown = if current.complete {
            current.observed.clone()
        } else {
            current
                .retained
                .clone()
                .unwrap_or_else(|| local_only(current.cwd))
        };
        Some(View {
            generation: current.generation,
            cwd: cwd_path(current.cwd),
            local: current.local,
            plan: current.plan,
            directory: shown.directory,
            acquired: shown.acquired,
            modules: shown.modules,
            complete: current.complete,
        })
    }
}

// Contract clauses checked directly, so a failure names the broken clause even
// when the model is wrong in the same way as the machine. docs/architecture.md
// cites each one.

fn at_most_one_generation_starts(did: &[Did]) -> Result<(), TestCaseError> {
    let spawned = did
        .iter()
        .filter(|did| matches!(did, Did::Spawn(..)))
        .count();
    prop_assert!(spawned <= 1, "more than one generation started");
    Ok(())
}

/// Complete observations are settled; incomplete ones show only retained or local data.
fn partial_results_stay_hidden(after: Option<&View>) -> Result<(), TestCaseError> {
    let shows_pending = after.is_some_and(|view| {
        view.modules.iter().any(|(condition, values)| {
            *condition == Seen::Pending || values.contains(&Seen::Pending)
        })
    });
    prop_assert!(!shows_pending, "a response shows Pending");
    Ok(())
}

fn events_for_other_or_settled_generations_change_nothing(
    op: &Op,
    before: Option<&View>,
    after: Option<&View>,
) -> Result<(), TestCaseError> {
    if let (Some(generation), Some(before)) = (op.event_generation(), before)
        && (generation != before.generation || before.complete)
    {
        prop_assert_eq!(
            after,
            Some(before),
            "an event for another or a settled generation changed the display"
        );
    }
    Ok(())
}

fn local_inputs_rerender_without_reacquisition(
    op: &Op,
    before: Option<&View>,
    after: Option<&View>,
    did: &[Did],
) -> Result<(), TestCaseError> {
    if let (
        Op::Request {
            generation, local, ..
        },
        Some(before),
    ) = (op, before)
        && *generation == before.generation
    {
        prop_assert!(
            !did.iter()
                .any(|did| matches!(did, Did::Spawn(..) | Did::Cancel)),
            "a same-generation request started or cancelled acquisition"
        );
        let expected = View {
            local: *local,
            ..before.clone()
        };
        prop_assert_eq!(
            after,
            Some(&expected),
            "a same-generation request changed more than local inputs"
        );
    }
    Ok(())
}

fn changed_snapshot_discards_retained_display(
    op: &Op,
    before: Option<&View>,
    after: Option<&View>,
) -> Result<(), TestCaseError> {
    if let (
        Op::Request {
            generation, cwd, ..
        },
        Some(before),
    ) = (op, before)
        && *generation > before.generation
        && cwd_path(*cwd) != before.cwd
    {
        prop_assert!(
            after.is_some_and(|after| after.modules.is_empty() && !after.acquired),
            "a changed snapshot kept retained display"
        );
    }
    Ok(())
}

fn accepted_plan_change_discards_retained_display(
    op: &Op,
    before: Option<&View>,
    after: Option<&View>,
) -> Result<(), TestCaseError> {
    if let (Op::Plan { generation, .. }, Some(before), Some(after)) = (op, before, after)
        && *generation == before.generation
        && after.plan != before.plan
    {
        prop_assert!(
            after.modules.is_empty() && !after.acquired,
            "an accepted plan change kept retained display"
        );
    }
    Ok(())
}

proptest! {
    #[test]
    fn machine_follows_the_reference_model(ops in prop::collection::vec(op(), 0..40)) {
        let mut machine = Machine::default();
        let fixture = Fixture::new(machine.plan.clone()).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let mut model = Model::default();
        for op in &ops {
            let before = fixture.view(&machine);
            let op = &op.resolve(before.as_ref().map(|view| view.generation));
            let did = machine.step(fixture.input(op)?).iter().map(|effect| fixture.did(effect)).collect::<Vec<_>>();
            let after = fixture.view(&machine);
            let (before, after) = (before.as_ref(), after.as_ref());
            at_most_one_generation_starts(&did)?;
            partial_results_stay_hidden(after)?;
            events_for_other_or_settled_generations_change_nothing(op, before, after)?;
            local_inputs_rerender_without_reacquisition(op, before, after, &did)?;
            changed_snapshot_discards_retained_display(op, before, after)?;
            accepted_plan_change_discards_retained_display(op, before, after)?;
            prop_assert_eq!(&did, &model.step(op, &fixture), "effects after {:?}", op);
            let expected = model.view();
            prop_assert_eq!(after, expected.as_ref(), "display after {:?}", op);
        }
    }
}

#[test]
fn events_queued_before_a_failed_task_cannot_change_its_settled_display()
-> Result<(), Box<dyn std::error::Error>> {
    let local = Local {
        cols: 80,
        vicmd: false,
        status: 0,
        duration: None,
    };
    let mut machine = Machine::default();
    let fixture = Fixture::new(machine.plan.clone())?;
    machine.step(Input::Request(request(1, 0, local)?));
    machine.step(Input::Joined { failed: true });
    let settled = fixture.view(&machine);
    assert!(settled.as_ref().is_some_and(|view| view.complete));
    // `select!` may join the failed task before receiving what it queued.
    for op in [
        Op::Plan {
            generation: 1,
            plan: 2,
            diagnostic: None,
        },
        Op::Condition {
            generation: 1,
            module: 0,
            result: Some(true),
        },
    ] {
        machine.step(fixture.input(&op)?);
        assert_eq!(fixture.view(&machine), settled);
    }
    Ok(())
}
