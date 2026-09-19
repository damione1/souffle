//! Opt-in Settings latency instrumentation (SOU-210).
//!
//! Enable with `SOUFFLE_SETTINGS_INSTRUMENT=1`. Every line is JSON prefixed
//! with `SOUFFLE_SETTINGS_METRIC ` so Instruments/signpost exports and stderr
//! captures can be joined without treating an accessibility notification as
//! a presentation timestamp.

use crate::{MainWindow, SettingsInteraction};
use serde_json::json;
use slint::{ComponentHandle, RenderingState};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum SettingsScenario {
    Open,
    Tab,
    Theme,
    Menu,
    Number,
}

impl SettingsScenario {
    fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Tab => "tab",
            Self::Theme => "theme",
            Self::Menu => "menu",
            Self::Number => "number",
        }
    }
}

impl From<SettingsInteraction> for SettingsScenario {
    fn from(value: SettingsInteraction) -> Self {
        match value {
            SettingsInteraction::Open => Self::Open,
            SettingsInteraction::Tab => Self::Tab,
            SettingsInteraction::Theme => Self::Theme,
            SettingsInteraction::Menu => Self::Menu,
            SettingsInteraction::Number => Self::Number,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum SettingsMilestone {
    Input,
    Handler,
    OsDbStart,
    OsDbEnd,
    Commit,
    Snapshot,
    AfterRender,
}

impl SettingsMilestone {
    fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Handler => "handler",
            Self::OsDbStart => "os_db_start",
            Self::OsDbEnd => "os_db_end",
            Self::Commit => "commit",
            Self::Snapshot => "snapshot",
            Self::AfterRender => "after_render",
        }
    }
}

struct TraceInner {
    sequence: u64,
}

pub(crate) struct SettingsTrace {
    id: Uuid,
    scenario: SettingsScenario,
    started: Instant,
    inner: Mutex<TraceInner>,
}

impl SettingsTrace {
    fn new(scenario: SettingsScenario) -> Arc<Self> {
        Arc::new(Self {
            id: Uuid::new_v4(),
            scenario,
            started: Instant::now(),
            inner: Mutex::new(TraceInner { sequence: 0 }),
        })
    }

    fn mark(&self, milestone: SettingsMilestone, detail: Option<&str>) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.sequence += 1;
        emit(json!({
            "kind": "milestone",
            "run_id": self.id.to_string(),
            "scenario": self.scenario.name(),
            "milestone": milestone.name(),
            "sequence": inner.sequence,
            "elapsed_us": self.started.elapsed().as_micros(),
            "detail": detail,
        }));
    }

    pub(crate) fn os_db_start(&self) {
        self.mark(SettingsMilestone::OsDbStart, None);
    }
    pub(crate) fn os_db_end(&self) {
        self.mark(SettingsMilestone::OsDbEnd, None);
    }
    pub(crate) fn commit(&self, committed: bool) {
        self.mark(
            SettingsMilestone::Commit,
            Some(if committed {
                "committed"
            } else {
                "not_committed"
            }),
        );
    }
}

#[derive(Default)]
struct InstrumentationState {
    input: HashMap<SettingsScenario, Arc<SettingsTrace>>,
    persistence: VecDeque<Arc<SettingsTrace>>,
    after_render: Vec<Arc<SettingsTrace>>,
}

static STATE: OnceLock<Mutex<InstrumentationState>> = OnceLock::new();

fn state() -> &'static Mutex<InstrumentationState> {
    STATE.get_or_init(|| Mutex::new(InstrumentationState::default()))
}

fn enabled() -> bool {
    std::env::var("SOUFFLE_SETTINGS_INSTRUMENT").as_deref() == Ok("1")
}

fn emit(value: serde_json::Value) {
    if enabled() {
        eprintln!("SOUFFLE_SETTINGS_METRIC {value}");
    }
}

pub(crate) fn input(interaction: SettingsInteraction) {
    if !enabled() {
        return;
    }
    let scenario = SettingsScenario::from(interaction);
    let trace = SettingsTrace::new(scenario);
    trace.mark(SettingsMilestone::Input, None);
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .input
        .insert(scenario, trace);
}

pub(crate) fn handler(scenario: SettingsScenario) {
    if !enabled() {
        return;
    }
    let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(trace) = state.input.remove(&scenario) {
        trace.mark(SettingsMilestone::Handler, None);
        state.persistence.push_back(trace);
    }
}

pub(crate) fn take_persistence_trace() -> Option<Arc<SettingsTrace>> {
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .persistence
        .pop_front()
}

pub(crate) fn snapshot(trace: Arc<SettingsTrace>) {
    trace.mark(SettingsMilestone::Snapshot, None);
    state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .after_render
        .push(trace);
}

pub(crate) fn snapshot_without_persistence(scenario: SettingsScenario) {
    if let Some(trace) = take_persistence_trace() {
        debug_assert_eq!(trace.scenario, scenario);
        snapshot(trace);
    }
}

fn after_render() {
    let traces = {
        let mut state = state().lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut state.after_render)
    };
    for trace in traces {
        trace.mark(SettingsMilestone::AfterRender, Some("not_presentation"));
    }
}

pub(crate) fn install(window: &MainWindow) {
    if !enabled() {
        return;
    }
    #[cfg(target_os = "macos")]
    let (surface, surface_evidence) = (
        "metal",
        "BackendSelector::require_metal succeeded before window creation",
    );
    #[cfg(not(target_os = "macos"))]
    let (surface, surface_evidence) = ("unverified", "no explicit surface selection");
    let size = window.window().size();
    emit(json!({
        "kind": "run_metadata",
        "sha": option_env!("SOUFFLE_BUILD_GIT_SHA").unwrap_or("unavailable"),
        "signature": std::env::var("SOUFFLE_SETTINGS_RUN_SIGNATURE").unwrap_or_else(|_| "unprovided".into()),
        "profile": option_env!("SOUFFLE_BUILD_PROFILE").unwrap_or("unavailable"),
        "dataset": std::env::var("SOUFFLE_SETTINGS_DATASET").unwrap_or_else(|_| "unprovided".into()),
        "screen": std::env::var("SOUFFLE_SETTINGS_SCREEN").unwrap_or_else(|_| "unprovided".into()),
        "window": { "width": size.width, "height": size.height },
        "model_state": std::env::var("SOUFFLE_SETTINGS_MODEL_STATE").unwrap_or_else(|_| "unprovided".into()),
        "backend": "winit",
        "renderer": "skia",
        "surface": surface,
        "surface_evidence": surface_evidence,
        "presentation_timing": "unavailable",
        "proofs": {
            "ui": "instrumented_not_measured",
            "kyutai": "not_run",
            "whisper": "not_run",
            "parakeet": "not_run",
            "apple_api": "not_run"
        }
    }));
    if let Err(error) = window
        .window()
        .set_rendering_notifier(|state, _graphics_api| {
            if matches!(state, RenderingState::AfterRendering) {
                after_render();
            }
        })
    {
        emit(
            json!({"kind": "capability", "after_render": "unavailable", "reason": error.to_string()}),
        );
    }
}
