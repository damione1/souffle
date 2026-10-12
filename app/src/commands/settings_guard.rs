use crate::audio::start_gate::CaptureStartGate;
use crate::state::AppState;
use crate::state_machine::AppStateMachine;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsCaptureError {
    CaptureBusy,
    RuntimeBusy,
    OperationFailed { message: String },
}

impl std::fmt::Display for SettingsCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CaptureBusy => formatter
                .write_str("Cannot change this setting while recording or starting a recording"),
            Self::RuntimeBusy => {
                formatter.write_str("Another operation is in progress. Try again shortly.")
            }
            Self::OperationFailed { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SettingsCaptureError {}

/// The canonical machine alone is still Ready while audio/engine start.
/// Used by Settings handlers before changing drafts or visible busy state.
pub fn settings_capture_busy(state: &AppState) -> Result<bool, SettingsCaptureError> {
    if state.capture_start.start_in_flight_or_locked() {
        return Ok(true);
    }
    match capture_permission(&state.machine) {
        Ok(()) => Ok(false),
        Err(SettingsCaptureError::CaptureBusy | SettingsCaptureError::RuntimeBusy) => Ok(true),
        Err(error @ SettingsCaptureError::OperationFailed { .. }) => Err(error),
    }
}

fn capture_permission(machine: &Mutex<AppStateMachine>) -> Result<(), SettingsCaptureError> {
    match machine.try_lock() {
        Ok(machine) => {
            if machine.is_recording() {
                Err(SettingsCaptureError::CaptureBusy)
            } else {
                Ok(())
            }
        }
        Err(std::sync::TryLockError::WouldBlock) => Err(SettingsCaptureError::RuntimeBusy),
        Err(std::sync::TryLockError::Poisoned(error)) => {
            Err(SettingsCaptureError::OperationFailed {
                message: format!("Lock poisoned: {error}"),
            })
        }
    }
}

/// Reset/rebind effects cannot race the capture reservation. The machine
/// snapshot lock is released before native work; every production capture
/// start reserves this gate before opening audio or entering Recording*.
pub(crate) fn with_capture_idle<T>(
    gate: &CaptureStartGate,
    machine: &Mutex<AppStateMachine>,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, SettingsCaptureError> {
    gate.try_with_start_boundary(|starting| {
        if starting {
            return Err(SettingsCaptureError::CaptureBusy);
        }
        capture_permission(machine)?;
        operation().map_err(|message| SettingsCaptureError::OperationFailed { message })
    })
    .unwrap_or(Err(SettingsCaptureError::RuntimeBusy))
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
    fn competing_settings_effects_and_machine_changes_refuse_without_waiting() {
        use std::sync::mpsc;
        use std::time::Duration;

        let gate = Arc::new(CaptureStartGate::new());
        let machine = Arc::new(Mutex::new(AppStateMachine::Idle));
        let check = || {
            let (tx, rx) = mpsc::channel();
            let gate = gate.clone();
            let machine = machine.clone();
            let worker = std::thread::spawn(move || {
                let result = with_capture_idle(&gate, &machine, || {
                    panic!("contended intent must refuse before its native effect")
                });
                let _ = tx.send(result);
            });
            (worker, rx)
        };
        let (worker, observed) = gate.with_start_boundary(|_| {
            let (worker, rx) = check();
            (worker, rx.recv_timeout(Duration::from_secs(1)))
        });
        worker.join().unwrap();
        assert_eq!(
            observed.unwrap(),
            Err::<(), _>(SettingsCaptureError::RuntimeBusy)
        );

        let held = machine.lock().unwrap();
        let (worker, rx) = check();
        let observed = rx.recv_timeout(Duration::from_secs(1));
        drop(held);
        worker.join().unwrap();
        assert_eq!(
            observed.unwrap(),
            Err::<(), _>(SettingsCaptureError::RuntimeBusy)
        );
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
