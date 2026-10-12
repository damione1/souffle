use crate::audio::start_gate::CaptureStartGate;
use crate::lock_ext::MutexExt;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCaptureError {
    CaptureBusy,
    OperationFailed { message: String },
}

impl std::fmt::Display for SettingsCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureBusy => formatter
                .write_str("Cannot change this setting while recording or starting a recording"),
            Self::OperationFailed { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SettingsCaptureError {}

/// The canonical machine alone is still Ready while audio/engine start.
/// Used by Settings handlers before changing drafts or visible busy state.
pub fn settings_capture_busy(state: &AppState) -> Result<bool, String> {
    Ok(state.capture_start.start_in_flight_or_locked() || state.machine.acquire()?.is_recording())
}

/// Reset/rebind effects cannot race the capture reservation. The machine
/// snapshot lock is released before native work; every production capture
/// start reserves this gate before opening audio or entering Recording*.
pub(crate) fn with_capture_idle<T>(
    gate: &CaptureStartGate,
    machine: &Mutex<AppStateMachine>,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, SettingsCaptureError> {
    gate.with_start_boundary(|starting| {
        if starting
            || machine
                .acquire()
                .map_err(|message| SettingsCaptureError::OperationFailed { message })?
                .is_recording()
        {
            return Err(SettingsCaptureError::CaptureBusy);
        }
        operation().map_err(|message| SettingsCaptureError::OperationFailed { message })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};

    #[test]
    fn a_reserved_start_wins_before_any_setting_effect() {
        let gate = CaptureStartGate::new();
        gate.begin(1);
        let applied = AtomicBool::new(false);
        let result = with_capture_idle(&gate, &Mutex::new(AppStateMachine::Idle), || {
            applied.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(result.is_err());
        assert!(!applied.load(Ordering::SeqCst));
    }

    #[test]
    fn a_setting_effect_finishes_before_a_racing_capture_reservation() {
        let gate = Arc::new(CaptureStartGate::new());
        let barrier = Arc::new(Barrier::new(2));
        let effect_finished = Arc::new(AtomicBool::new(false));
        let worker_gate = gate.clone();
        let worker_barrier = barrier.clone();
        let worker_finished = effect_finished.clone();
        let worker = with_capture_idle(&gate, &Mutex::new(AppStateMachine::Idle), || {
            let worker = std::thread::spawn(move || {
                worker_barrier.wait();
                assert!(
                    worker_gate.start_in_flight_or_locked(),
                    "the effect must own the start boundary"
                );
                worker_barrier.wait();
                worker_gate.begin(1);
                assert!(
                    worker_finished.load(Ordering::SeqCst),
                    "start must wait for the authorized effect"
                );
            });
            barrier.wait();
            barrier.wait();
            effect_finished.store(true, Ordering::SeqCst);
            Ok(worker)
        })
        .unwrap();
        worker.join().unwrap();
        assert!(gate.start_in_flight());
    }
}
