//! Model-based tests: random interleavings of requests, events, and task
//! completion against a reference model written from the behavioral contract
//! in docs/architecture.md.

use std::{ffi::OsString, path::PathBuf};

use capsule_protocol::session::Snapshot;
use proptest::prelude::*;

use super::*;

/// One input, with generations drawn from a small range so that current,
/// stale, and future generations all occur.
#[derive(Debug, Clone)]
enum Op {
    Request {
        generation: u64,
        cwd: u8,
        cols: u16,
        keymap: bool,
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

/// A value acquisition result.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    Ready(u8),
    Missing,
    Failed,
}

fn op() -> impl Strategy<Value = Op> {
    let generation = 1..5u64;
    prop_oneof![
        3 => (generation.clone(), 0..2u8, 0..3u16, any::<bool>())
            .prop_map(|(generation, cwd, cols, keymap)| Op::Request { generation, cwd, cols, keymap }),
        2 => (generation.clone(), 0..3usize, prop::option::of(0..2u8))
            .prop_map(|(generation, plan, diagnostic)| Op::Plan { generation, plan, diagnostic }),
        1 => (generation.clone(), any::<bool>()).prop_map(|(generation, ok)| Op::Context { generation, ok }),
        3 => (generation.clone(), 0..3usize, prop::option::of(any::<bool>()))
            .prop_map(|(generation, module, result)| Op::Condition { generation, module, result }),
        3 => (generation.clone(), 0..3usize, 0..3usize, prop_oneof![(0..2u8).prop_map(Outcome::Ready), Just(Outcome::Missing), Just(Outcome::Failed)])
            .prop_map(|(generation, module, value, result)| Op::Value { generation, module, value, result }),
        2 => generation.prop_map(|generation| Op::Complete { generation }),
        2 => any::<bool>().prop_map(|failed| Op::Joined { failed }),
    ]
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
    cols: u16,
    keymap: String,
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

    fn input(&self, op: &Op) -> Input {
        let event = |generation, change| Input::Event(Event { generation, change });
        match op.clone() {
            Op::Request {
                generation,
                cwd,
                cols,
                keymap,
            } => Input::Request(request(generation, cwd, cols, keymap)),
            Op::Plan {
                generation,
                plan,
                diagnostic,
            } => event(
                generation,
                Change::Plan(
                    self.plans[plan].clone(),
                    diagnostic.map(|code| format!("invalid {code}")),
                ),
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
                        Outcome::Ready(code) => Ok(Some(format!("v{code}"))),
                        Outcome::Missing => Ok(None),
                        Outcome::Failed => Err(AcquireError::TimedOut),
                    },
                ),
            ),
            Op::Complete { generation } => event(generation, Change::Complete),
            Op::Joined { failed } => Input::Joined { failed },
        }
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
            cwd: shown.request.snapshot.cwd.clone(),
            cols: shown.request.cols,
            keymap: shown.request.keymap.clone(),
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

fn request(generation: u64, cwd: u8, cols: u16, keymap: bool) -> Request {
    Request {
        generation,
        snapshot: Snapshot {
            cwd: PathBuf::from(format!("/cwd{cwd}")),
            env: vec![
                (OsString::from("B"), OsString::new()),
                (OsString::from("A"), OsString::new()),
            ],
        },
        cols,
        last_exit_code: 0,
        duration_ms: None,
        keymap: if keymap { "vicmd" } else { "main" }.to_owned(),
    }
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

#[derive(Debug, Clone)]
struct Current {
    generation: u64,
    cwd: u8,
    cols: u16,
    keymap: bool,
    plan: usize,
    observed: Settled,
    complete: bool,
    retained: Option<Settled>,
}

/// The contract restated, independently of the implementation.
#[derive(Debug, Clone, Default)]
struct Model {
    current: Option<Current>,
    /// The worker's plan: the last one a generation of this worker delivered.
    plan: usize,
    pending: Option<(u64, u8, u16, bool)>,
    running: bool,
    active: u64,
    last_diagnostic: Option<String>,
}

fn local(cwd: u8) -> Settled {
    Settled {
        modules: Vec::new(),
        directory: format!("/cwd{cwd}"),
        acquired: false,
    }
}

impl Model {
    fn step(&mut self, op: &Op, fixture: &Fixture) -> Vec<Did> {
        let mut did = Vec::new();
        let mut render = true;
        let current_generation = self.current.as_ref().map(|current| current.generation);
        match *op {
            // WHEN a request is older than the current one, it SHALL be ignored.
            Op::Request { generation, .. }
                if current_generation.is_some_and(|current| generation < current) =>
            {
                render = false;
            }
            // WHEN a request repeats the current generation, only local inputs SHALL change.
            Op::Request {
                generation,
                cols,
                keymap,
                ..
            } if current_generation == Some(generation) => {
                if let Some(current) = self.current.as_mut() {
                    current.cols = cols;
                    current.keymap = keymap;
                }
            }
            // WHEN a newer request arrives, the running generation SHALL be cancelled;
            // a settled display SHALL be retained only for an identical snapshot.
            Op::Request {
                generation,
                cwd,
                cols,
                keymap,
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
                    cols,
                    keymap,
                    plan: self.plan,
                    observed: local(cwd),
                    complete: false,
                    retained,
                });
                self.pending = Some((generation, cwd, cols, keymap));
            }
            // WHEN an event belongs to another generation, or to one that has
            // settled, it SHALL change nothing.
            Op::Plan { generation, .. }
            | Op::Context { generation, .. }
            | Op::Condition { generation, .. }
            | Op::Value { generation, .. }
            | Op::Complete { generation }
                if current_generation != Some(generation)
                    || self
                        .current
                        .as_ref()
                        .is_some_and(|current| current.complete) => {}
            Op::Plan {
                plan, diagnostic, ..
            } => {
                let diagnostic = diagnostic.map(|code| format!("invalid {code}"));
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
                        Outcome::Ready(code) => Seen::Ready(format!("v{code}")),
                        Outcome::Missing => Seen::Missing,
                        Outcome::Failed => Seen::Failed,
                    };
                }
            }
            Op::Complete { .. } => self.settle(),
            Op::Joined { failed } => {
                self.running = false;
                if failed && current_generation == Some(self.active) {
                    self.settle();
                }
            }
        }
        // At most one generation SHALL run; the newest pending request starts next.
        if !self.running
            && let Some((generation, ..)) = self.pending.take()
        {
            self.running = true;
            self.active = generation;
            did.push(Did::Spawn(generation, self.plan));
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
                .unwrap_or_else(|| local(current.cwd))
        };
        Some(View {
            generation: current.generation,
            cwd: PathBuf::from(format!("/cwd{}", current.cwd)),
            cols: current.cols,
            keymap: if current.keymap { "vicmd" } else { "main" }.to_owned(),
            plan: current.plan,
            directory: shown.directory,
            acquired: shown.acquired,
            modules: shown.modules,
            complete: current.complete,
        })
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn machine_follows_the_reference_model(ops in prop::collection::vec(op(), 0..40)) {
        let mut machine = Machine::default();
        let fixture = Fixture::new(machine.plan.clone()).map_err(|error| TestCaseError::fail(error.to_string()))?;
        let mut model = Model::default();
        for op in &ops {
            let before = fixture.view(&machine);
            let did = machine.step(fixture.input(op)).iter().map(|effect| fixture.did(effect)).collect::<Vec<_>>();
            let after = fixture.view(&machine);
            prop_assert_eq!(&did, &model.step(op, &fixture), "effects after {:?}", op);
            prop_assert_eq!(&after, &model.view(), "display after {:?}", op);

            // Named invariants, restated so a failure names the broken rule.
            let current = before.as_ref().map(|view| view.generation);
            match op {
                Op::Plan { generation, .. }
                | Op::Context { generation, .. }
                | Op::Condition { generation, .. }
                | Op::Value { generation, .. }
                | Op::Complete { generation }
                    if current != Some(*generation) =>
                {
                    prop_assert_eq!(&after, &before, "a non-current event changed the display");
                }
                Op::Request { generation, .. } if current == Some(*generation) => {
                    prop_assert!(
                        !did.iter().any(|did| matches!(did, Did::Spawn(..) | Did::Cancel)),
                        "a same-generation request started or cancelled acquisition"
                    );
                }
                _ => {}
            }
            if let Some(view) = &after
                && view.complete
            {
                prop_assert!(
                    view.modules.iter().all(|(condition, values)| *condition != Seen::Pending && !values.contains(&Seen::Pending)),
                    "a complete generation shows Pending"
                );
            }
            prop_assert!(did.iter().filter(|did| matches!(did, Did::Spawn(..))).count() <= 1);
        }
    }
}

#[test]
fn environment_order_does_not_change_the_snapshot() {
    let mut machine = Machine::default();
    let first = machine.step(Input::Request(request(1, 0, 80, false)));
    let mut reordered = request(2, 0, 80, false);
    reordered.snapshot.env.reverse();
    let second = machine.step(Input::Request(reordered));
    let spawned = |effects: &[Effect]| {
        effects.iter().find_map(|effect| match effect {
            Effect::Spawn(request, _) => Some(request.snapshot.clone()),
            _ => None,
        })
    };
    assert!(spawned(&first).is_some());
    // The second request waits for the first task, but its snapshot is sorted.
    assert!(spawned(&second).is_none());
    let joined = machine.step(Input::Joined { failed: false });
    assert_eq!(spawned(&joined), spawned(&first));
}

#[test]
fn events_queued_before_a_failed_task_cannot_change_its_settled_display()
-> Result<(), capsule_core::plan::ConfigError> {
    let mut machine = Machine::default();
    let fixture = Fixture::new(machine.plan.clone())?;
    machine.step(Input::Request(request(1, 0, 80, false)));
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
        machine.step(fixture.input(&op));
        assert_eq!(fixture.view(&machine), settled);
    }
    Ok(())
}
