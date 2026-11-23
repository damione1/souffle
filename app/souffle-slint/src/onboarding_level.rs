//! Live microphone level on the onboarding microphone step.

use crate::MainWindow;
use slint::{ComponentHandle, ModelRc, VecModel};
use souffle_lib::audio::level_probe::LevelProbe;
use souffle_lib::permissions::{PermState, snapshot};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Duration;

const BAR_COUNT: usize = 27;
const TICK: Duration = Duration::from_millis(66);
const SMOOTHING: f32 = 0.3;

#[derive(Default)]
pub struct LevelMonitor {
    running: Option<(String, LevelProbe)>,
    timer: slint::Timer,
}

impl LevelMonitor {
    /// Keep the probe in step with the wizard: running on the microphone step
    /// with access granted, stopped and zeroed everywhere else.
    pub fn sync(&mut self, window: &MainWindow, on_microphone_step: bool, uid: &str) {
        if !on_microphone_step || snapshot().microphone != PermState::Granted {
            self.stop(window);
            return;
        }
        if self
            .running
            .as_ref()
            .is_some_and(|(running_uid, _)| running_uid == uid)
        {
            return;
        }
        self.start(window, uid);
    }

    pub fn stop(&mut self, window: &MainWindow) {
        self.timer.stop();
        self.running = None;
        window.set_onboarding_audio_level(0.0);
        window.set_onboarding_audio_levels(ModelRc::default());
    }

    fn start(&mut self, window: &MainWindow, uid: &str) {
        let probe = LevelProbe::start(uid);
        let reader = probe.reader();
        self.running = Some((uid.to_string(), probe));
        let weak = window.as_weak();
        let history = RefCell::new(VecDeque::from(vec![0.0f32; BAR_COUNT]));
        let eased = RefCell::new(0.0f32);
        self.timer.start(slint::TimerMode::Repeated, TICK, move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !window.get_window_visible() {
                return;
            }
            let raw = f32::from_bits(reader.load(Ordering::Relaxed));
            let mut level = eased.borrow_mut();
            *level += (raw - *level) * SMOOTHING;
            let mut history = history.borrow_mut();
            history.pop_front();
            history.push_back(raw);
            window.set_onboarding_audio_level(*level);
            window.set_onboarding_audio_levels(ModelRc::new(VecModel::from(
                history.iter().copied().collect::<Vec<_>>(),
            )));
        });
    }
}
