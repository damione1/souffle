//! Shared permission observation/controller for onboarding and Settings.
//!
//! Both surfaces project the same snapshot and use the same bounded poll.
//! The controller owns only a weak Slint window handle; timers and async
//! completions hold a weak controller handle so closing the surface cannot
//! retain either one.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use souffle_lib::permissions::{PermState, PermissionKind, PermissionStatus};

use crate::{MainWindow, onboarding_ui};

const PERMISSION_POLL_INTERVAL: Duration = Duration::from_millis(600);
const REPAIR_COOLDOWN: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollTransition {
    Start,
    Stop,
    Unchanged,
}

#[derive(Debug, Default)]
struct PollLifecycle {
    active: bool,
    generation: u64,
}

impl PollLifecycle {
    fn update(&mut self, active: bool) -> PollTransition {
        if self.active == active {
            return PollTransition::Unchanged;
        }
        self.active = active;
        self.generation = self.generation.wrapping_add(1);
        if active {
            PollTransition::Start
        } else {
            PollTransition::Stop
        }
    }

    #[cfg(test)]
    fn timer_count(&self) -> usize {
        usize::from(self.active)
    }
}

#[derive(Clone)]
struct PermissionViewState {
    status: PermissionStatus,
    busy: [bool; 3],
}

impl Default for PermissionViewState {
    fn default() -> Self {
        Self {
            status: PermissionStatus {
                microphone: PermState::Unknown,
                system_audio: PermState::Unknown,
                accessibility: PermState::Unknown,
                calendar: PermState::Unknown,
            },
            busy: [false; 3],
        }
    }
}

impl PermissionViewState {
    fn observe(&mut self, status: PermissionStatus) {
        self.status = status;
    }
}

pub(crate) struct PermissionController {
    window: slint::Weak<MainWindow>,
    state: RefCell<PermissionViewState>,
    lifecycle: RefCell<PollLifecycle>,
    poll_timer: RefCell<Option<slint::Timer>>,
    cooldown_timer: RefCell<Option<slint::Timer>>,
    refresh_in_flight: Cell<bool>,
    repair_in_flight: Cell<bool>,
    /// Internal anti-spam state for the Accessibility repair. Unlike the
    /// `onboarding-repair-cooldown` window property (pure display), this must
    /// stay armed even when the surface closes mid-repair, so reopening the
    /// panel cannot immediately re-run `tccutil reset`.
    cooldown_active: Cell<bool>,
    observation_revision: Cell<u64>,
    applied_revision: Cell<u64>,
}

impl PermissionController {
    pub(crate) fn new(window: &MainWindow) -> Rc<Self> {
        Rc::new(Self {
            window: window.as_weak(),
            state: RefCell::new(PermissionViewState::default()),
            lifecycle: RefCell::new(PollLifecycle::default()),
            poll_timer: RefCell::new(None),
            cooldown_timer: RefCell::new(None),
            refresh_in_flight: Cell::new(false),
            repair_in_flight: Cell::new(false),
            cooldown_active: Cell::new(false),
            observation_revision: Cell::new(0),
            applied_revision: Cell::new(0),
        })
    }

    pub(crate) fn wire_foreground_refresh(self: &Rc<Self>, window: &MainWindow) {
        let controller = Rc::downgrade(self);
        window.window().on_winit_window_event(move |_, event| {
            if matches!(event, winit::event::WindowEvent::Focused(true))
                && let Some(controller) = controller.upgrade()
            {
                controller.refresh();
            }
            EventResult::Propagate
        });
    }

    pub(crate) fn status(&self) -> PermissionStatus {
        self.state.borrow().status.clone()
    }

    pub(crate) fn project(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = self.state.borrow();
        onboarding_ui::populate_permission_rows(&window, &state.status, state.busy);
        window
            .set_onboarding_accessibility_granted(state.status.accessibility == PermState::Granted);
    }

    fn surface_active(&self) -> bool {
        self.window.upgrade().is_some_and(|window| {
            window.get_settings_open()
                || (window.get_onboarding_open()
                    && window.get_onboarding_step().as_str() == "permissions")
        })
    }

    pub(crate) fn sync_activity(self: &Rc<Self>) {
        let active = self.surface_active();
        let transition = self.lifecycle.borrow_mut().update(active);
        match transition {
            PollTransition::Start => {
                let timer = slint::Timer::default();
                let controller = Rc::downgrade(self);
                timer.start(
                    slint::TimerMode::Repeated,
                    PERMISSION_POLL_INTERVAL,
                    move || {
                        if let Some(controller) = controller.upgrade() {
                            controller.refresh();
                        }
                    },
                );
                *self.poll_timer.borrow_mut() = Some(timer);
                // Re-project internal repair state on reopen: a repair still
                // in flight shows busy again, and a cooldown armed while the
                // surface was closed keeps the button disabled.
                if let Some(window) = self.window.upgrade() {
                    window.set_onboarding_repair_busy(self.repair_in_flight.get());
                    window.set_onboarding_repair_cooldown(self.cooldown_active.get());
                }
                self.refresh();
            }
            PollTransition::Stop => {
                *self.poll_timer.borrow_mut() = None;
                // Keep `cooldown_timer` alive: the anti-spam cooldown is
                // internal state and must expire on schedule even while the
                // surface is closed.
                if let Some(window) = self.window.upgrade() {
                    window.set_onboarding_repair_busy(false);
                    window.set_onboarding_repair_cooldown(false);
                    window.set_onboarding_repair_success(false);
                }
            }
            PollTransition::Unchanged => {}
        }
    }

    pub(crate) fn refresh(self: &Rc<Self>) {
        if !self.surface_active()
            || self.refresh_in_flight.get()
            || self.repair_in_flight.get()
            || self.state.borrow().busy.iter().any(|busy| *busy)
        {
            return;
        }
        self.refresh_in_flight.set(true);
        let generation = self.lifecycle.borrow().generation;
        let revision = self.begin_observation();
        let controller = Rc::downgrade(self);
        slint::spawn_local(async move {
            let result = souffle_lib::commands::get_permission_status().await;
            let Some(controller) = controller.upgrade() else {
                return;
            };
            controller.refresh_in_flight.set(false);
            if generation != controller.lifecycle.borrow().generation
                || !controller.surface_active()
                || controller.state.borrow().busy.iter().any(|busy| *busy)
            {
                return;
            }
            controller.apply_observation(revision, result);
        })
        .expect("slint event loop not running");
    }

    fn begin_observation(&self) -> u64 {
        let revision = self.observation_revision.get().wrapping_add(1);
        self.observation_revision.set(revision);
        revision
    }

    fn apply_observation(&self, revision: u64, result: Result<PermissionStatus, String>) {
        if revision < self.applied_revision.get() {
            return;
        }
        self.applied_revision.set(revision);
        let Some(window) = self.window.upgrade() else {
            return;
        };
        match result {
            Ok(status) => {
                self.state.borrow_mut().observe(status);
                window.set_onboarding_permissions_error("".into());
                self.project();
            }
            Err(error) => window.set_onboarding_permissions_error(error.into()),
        }
    }

    pub(crate) fn request(self: &Rc<Self>, kind: PermissionKind) {
        let Some(index) = onboarding_ui::row_index(kind) else {
            return;
        };
        self.state.borrow_mut().busy[index] = true;
        self.project();
        let revision = self.begin_observation();
        let controller = Rc::downgrade(self);
        slint::spawn_local(async move {
            let request = souffle_lib::commands::request_permission(kind).await;
            let observation = souffle_lib::commands::get_permission_status().await;
            let Some(controller) = controller.upgrade() else {
                return;
            };
            controller.state.borrow_mut().busy[index] = false;
            controller.apply_observation(revision, observation);
            if let Err(error) = request
                && let Some(window) = controller.window.upgrade()
            {
                window.set_onboarding_permissions_error(error.into());
            }
        })
        .expect("slint event loop not running");
    }

    pub(crate) fn open_settings(&self, kind: PermissionKind) {
        souffle_lib::commands::open_permission_settings(kind);
    }

    pub(crate) fn repair_accessibility(self: &Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.set_onboarding_repair_busy(true);
        window.set_onboarding_repair_success(false);
        self.repair_in_flight.set(true);
        let revision = self.begin_observation();
        let controller = Rc::downgrade(self);
        slint::spawn_local(async move {
            let repair = souffle_lib::commands::repair_accessibility_permission().await;
            // The repair result says what was attempted, never what TCC now
            // grants. Only a fresh observation may change the row to Granted.
            let observation = souffle_lib::commands::get_permission_status().await;
            let Some(controller) = controller.upgrade() else {
                return;
            };
            controller.repair_in_flight.set(false);
            let Some(window) = controller.window.upgrade() else {
                return;
            };
            window.set_onboarding_repair_busy(false);
            controller.apply_observation(revision, observation);
            match repair {
                Ok(_) => {
                    // The success banner is display-only: it must not reappear
                    // stale on the next open when the surface closed before
                    // the repair finished.
                    if controller.surface_active() {
                        window.set_onboarding_repair_success(true);
                    }
                    controller.start_repair_cooldown();
                }
                Err(error) => window.set_onboarding_permissions_error(error.into()),
            }
        })
        .expect("slint event loop not running");
    }

    fn start_repair_cooldown(self: &Rc<Self>) {
        // Arm the internal cooldown unconditionally: a repair that actually
        // ran must not be re-triggerable right away, even when the surface
        // closed before the async call resolved. Only the window property
        // (display) is gated on the surface being visible.
        self.cooldown_active.set(true);
        if self.surface_active()
            && let Some(window) = self.window.upgrade()
        {
            window.set_onboarding_repair_cooldown(true);
        }
        let timer = slint::Timer::default();
        let controller: Weak<Self> = Rc::downgrade(self);
        timer.start(slint::TimerMode::SingleShot, REPAIR_COOLDOWN, move || {
            if let Some(controller) = controller.upgrade() {
                controller.cooldown_active.set(false);
                if let Some(window) = controller.window.upgrade() {
                    window.set_onboarding_repair_cooldown(false);
                }
                *controller.cooldown_timer.borrow_mut() = None;
            }
        });
        *self.cooldown_timer.borrow_mut() = Some(timer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: PermState) -> PermissionStatus {
        PermissionStatus {
            microphone: state,
            system_audio: state,
            accessibility: state,
            calendar: state,
        }
    }

    #[test]
    fn simulated_observations_replace_every_permission_state() {
        let mut view = PermissionViewState::default();
        for state in [PermState::Granted, PermState::Denied, PermState::Unknown] {
            view.observe(status(state));
            assert_eq!(view.status.microphone, state);
            assert_eq!(view.status.system_audio, state);
            assert_eq!(view.status.accessibility, state);
            assert_eq!(view.status.calendar, state);
        }
    }

    #[test]
    fn denied_observation_wins_after_successful_repair_attempt() {
        let mut view = PermissionViewState::default();
        view.observe(status(PermState::Granted));
        view.observe(status(PermState::Denied));
        assert_eq!(view.status.accessibility, PermState::Denied);
    }

    #[test]
    fn poll_lifecycle_never_owns_more_than_one_timer_and_stops_on_close() {
        let mut lifecycle = PollLifecycle::default();
        assert_eq!(lifecycle.update(true), PollTransition::Start);
        assert_eq!(lifecycle.timer_count(), 1);
        assert_eq!(lifecycle.update(true), PollTransition::Unchanged);
        assert_eq!(lifecycle.timer_count(), 1);
        assert_eq!(lifecycle.update(false), PollTransition::Stop);
        assert_eq!(lifecycle.timer_count(), 0);
    }
}
