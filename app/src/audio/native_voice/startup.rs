//! A configuration notification can follow the engine's automatic stop. Only
//! that confirmed, otherwise unchanged graph gets one owner-thread restart.
use serde::Serialize;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) const CONFIGURATION_WAIT: Duration = Duration::from_millis(500);

pub(super) struct Control<'a> {
    pub cancelled: &'a AtomicBool,
    pub deadline: Instant,
}
impl Control<'_> {
    pub fn check(&self) -> Result<(), Failure> {
        self.check_at(Instant::now())
    }

    fn check_at(&self, now: Instant) -> Result<(), Failure> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        if now >= self.deadline {
            return Err(Failure::DeadlineElapsed);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) enum ReadbackFailure {
    EngineStoppedAfterStart,
    InputVoiceProcessingDisabled,
    OutputVoiceProcessingDisabled,
    AgcEnabled,
    VoiceProcessingBypassed,
    ClientFormatChanged(String),
    DeviceChanged(String),
    NominalRateChanged(String),
    DefaultOutputChanged,
    DuckingChanged,
    ConfigurationChangedDuringReadback,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Snapshot {
    pub running: bool,
    pub input_voice_processing: bool,
    pub output_voice_processing: bool,
    pub agc: bool,
    pub bypassed: bool,
    pub ducking_matches: bool,
    pub client_formats: Result<(), ReadbackFailure>,
    pub devices: Result<(), ReadbackFailure>,
    pub configuration_before: u64,
    pub configuration_after: u64,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), ReadbackFailure> {
        // Running is checked last: a stopped engine with any changed contract
        // must never qualify for recovery.
        self.client_formats.clone()?;
        self.devices.clone()?;
        if !self.input_voice_processing {
            return Err(ReadbackFailure::InputVoiceProcessingDisabled);
        }
        if !self.output_voice_processing {
            return Err(ReadbackFailure::OutputVoiceProcessingDisabled);
        }
        if self.agc {
            return Err(ReadbackFailure::AgcEnabled);
        }
        if self.bypassed {
            return Err(ReadbackFailure::VoiceProcessingBypassed);
        }
        if !self.ducking_matches {
            return Err(ReadbackFailure::DuckingChanged);
        }
        if self.configuration_before != self.configuration_after {
            return Err(ReadbackFailure::ConfigurationChangedDuringReadback);
        }
        if !self.running {
            return Err(ReadbackFailure::EngineStoppedAfterStart);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Start(String),
    Readback(ReadbackFailure),
    ConfigurationNotificationTimedOut,
    Cancelled,
    DeadlineElapsed,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start(error) => write!(f, "startAndReturnError: {error}"),
            Self::Readback(reason) => write!(f, "native post-start readback failed: {reason:?}"),
            Self::ConfigurationNotificationTimedOut => write!(
                f,
                "native post-start readback failed: {:?}; configuration notification did not arrive within recovery budget",
                ReadbackFailure::EngineStoppedAfterStart
            ),
            Self::Cancelled => f.write_str("native start cancelled"),
            Self::DeadlineElapsed => f.write_str("native start deadline elapsed"),
        }
    }
}
impl std::error::Error for Failure {}

pub(super) fn wait_for_configuration(
    control: &Control<'_>,
    baseline: u64,
    mut count: impl FnMut() -> u64,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(Duration),
) -> Result<(), Failure> {
    let until = (now() + CONFIGURATION_WAIT).min(control.deadline);
    loop {
        let current = now();
        control.check_at(current)?;
        if count() != baseline {
            return Ok(());
        }
        let remaining = until.saturating_duration_since(current);
        if remaining.is_zero() {
            return Err(Failure::ConfigurationNotificationTimedOut);
        }
        pause(remaining.min(Duration::from_millis(5)));
    }
}

enum ReadbackState {
    Running,
    Stopped,
}

/// A notification can land while HAL getters are being read. Re-read the
/// complete contract once across the whole startup, without starting again.
fn validated_readback(
    stage: usize,
    reread_used: &mut bool,
    check: &mut impl FnMut() -> Result<(), Failure>,
    readback: &mut impl FnMut(usize) -> Snapshot,
) -> Result<ReadbackState, Failure> {
    let snapshot = readback(stage);
    check()?;
    let result = match snapshot.validate() {
        Err(ReadbackFailure::ConfigurationChangedDuringReadback) if !*reread_used => {
            check()?;
            *reread_used = true;
            let snapshot = readback(stage);
            check()?;
            snapshot.validate()
        }
        result => result,
    };
    match result {
        Ok(()) => Ok(ReadbackState::Running),
        Err(ReadbackFailure::EngineStoppedAfterStart) => Ok(ReadbackState::Stopped),
        Err(
            reason @ (ReadbackFailure::InputVoiceProcessingDisabled
            | ReadbackFailure::OutputVoiceProcessingDisabled
            | ReadbackFailure::AgcEnabled
            | ReadbackFailure::VoiceProcessingBypassed
            | ReadbackFailure::ClientFormatChanged(_)
            | ReadbackFailure::DeviceChanged(_)
            | ReadbackFailure::NominalRateChanged(_)
            | ReadbackFailure::DefaultOutputChanged
            | ReadbackFailure::DuckingChanged
            | ReadbackFailure::ConfigurationChangedDuringReadback),
        ) => Err(Failure::Readback(reason)),
    }
}

/// Inject the OS calls and bounded wait so tests never capture microphone PCM.
/// `wait` also checks the original cancellation/deadline before returning.
pub(super) fn run(
    recover: bool,
    mut check: impl FnMut() -> Result<(), Failure>,
    mut start: impl FnMut(usize) -> Result<(), Failure>,
    mut readback: impl FnMut(usize) -> Snapshot,
    mut wait: impl FnMut() -> Result<(), Failure>,
) -> Result<(), Failure> {
    let mut reread_used = false;
    check()?;
    start(0)?;
    match validated_readback(0, &mut reread_used, &mut check, &mut readback)? {
        ReadbackState::Running => return Ok(()),
        ReadbackState::Stopped => {
            if !recover {
                return Err(Failure::Readback(ReadbackFailure::EngineStoppedAfterStart));
            }
        }
    }
    wait()?;
    check()?;
    // The notification is evidence of a reconfiguration, not permission to
    // restart a changed route. Re-read the entire contract before restarting.
    match validated_readback(1, &mut reread_used, &mut check, &mut readback)? {
        ReadbackState::Running => return Ok(()),
        ReadbackState::Stopped => {}
    }
    check()?;
    start(1)?;
    match validated_readback(2, &mut reread_used, &mut check, &mut readback)? {
        ReadbackState::Running => Ok(()),
        ReadbackState::Stopped => Err(Failure::Readback(ReadbackFailure::EngineStoppedAfterStart)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn snapshot(running: bool) -> Snapshot {
        Snapshot {
            running,
            input_voice_processing: true,
            output_voice_processing: true,
            agc: false,
            bypassed: false,
            ducking_matches: true,
            client_formats: Ok(()),
            devices: Ok(()),
            configuration_before: 0,
            configuration_after: 0,
        }
    }

    fn changed_during_readback(running: bool) -> Snapshot {
        let mut result = snapshot(running);
        result.configuration_after = 1;
        result
    }

    #[test]
    fn configuration_change_during_readback_gets_one_full_reread_then_one_restart() {
        let begun = Instant::now();
        let clock = Cell::new(begun);
        let cancelled = AtomicBool::new(false);
        let control = Control {
            cancelled: &cancelled,
            deadline: begun + Duration::from_secs(8),
        };
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let waits = Cell::new(0);
        run(
            true,
            || control.check_at(clock.get()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |stage| {
                reads.set(reads.get() + 1);
                clock.set(clock.get() + Duration::from_millis(1));
                match (reads.get(), stage) {
                    (1, 0) => changed_during_readback(false),
                    (2, 0) | (3, 1) => snapshot(false),
                    (4, 2) => snapshot(true),
                    _ => panic!("unexpected readback stage"),
                }
            },
            || {
                waits.set(waits.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(starts.get(), 2);
        assert_eq!(reads.get(), 4);
        assert_eq!(waits.get(), 1);
    }

    #[test]
    fn second_inconsistent_readback_fails_without_restart() {
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let result = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |stage| {
                assert_eq!(stage, 0);
                reads.set(reads.get() + 1);
                changed_during_readback(false)
            },
            || panic!("inconsistent readback must not wait"),
        );
        assert_eq!(
            result,
            Err(Failure::Readback(
                ReadbackFailure::ConfigurationChangedDuringReadback
            ))
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn changed_device_on_second_readback_fails_without_restart() {
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let result = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |_| {
                reads.set(reads.get() + 1);
                if reads.get() == 1 {
                    changed_during_readback(false)
                } else {
                    let mut changed = snapshot(false);
                    changed.devices = Err(ReadbackFailure::DefaultOutputChanged);
                    changed
                }
            },
            || panic!("changed device must not wait"),
        );
        assert_eq!(
            result,
            Err(Failure::Readback(ReadbackFailure::DefaultOutputChanged))
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(reads.get(), 2);
    }

    #[test]
    fn cancellation_or_deadline_during_reread_prevents_restart() {
        for cancel in [true, false] {
            let begun = Instant::now();
            let clock = Cell::new(begun);
            let cancelled = AtomicBool::new(false);
            let control = Control {
                cancelled: &cancelled,
                deadline: begun + Duration::from_secs(8),
            };
            let starts = Cell::new(0);
            let reads = Cell::new(0);
            let result = run(
                true,
                || control.check_at(clock.get()),
                |_| {
                    starts.set(starts.get() + 1);
                    Ok(())
                },
                |_| {
                    reads.set(reads.get() + 1);
                    if reads.get() == 1 {
                        changed_during_readback(false)
                    } else {
                        if cancel {
                            cancelled.store(true, Ordering::Release);
                        } else {
                            clock.set(control.deadline);
                        }
                        snapshot(false)
                    }
                },
                || panic!("cancelled reread must not wait"),
            );
            assert_eq!(
                result,
                Err(if cancel {
                    Failure::Cancelled
                } else {
                    Failure::DeadlineElapsed
                })
            );
            assert_eq!(starts.get(), 1);
            assert_eq!(reads.get(), 2);
        }
    }

    #[test]
    fn reread_budget_is_shared_across_startup_phases() {
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let result = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |stage| {
                reads.set(reads.get() + 1);
                match (stage, reads.get()) {
                    (0, 1) => changed_during_readback(false),
                    (0, 2) => snapshot(false),
                    (1, 3) => changed_during_readback(false),
                    _ => panic!("readback budget exceeded"),
                }
            },
            || Ok(()),
        );
        assert_eq!(
            result,
            Err(Failure::Readback(
                ReadbackFailure::ConfigurationChangedDuringReadback
            ))
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn delayed_notification_restarts_same_graph_once_after_revalidating() {
        let begun = Instant::now();
        let clock = Cell::new(begun);
        let count = Cell::new(0);
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let cancelled = AtomicBool::new(false);
        let control = Control {
            cancelled: &cancelled,
            deadline: begun + Duration::from_secs(8),
        };
        run(
            true,
            || control.check_at(clock.get()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |_| {
                reads.set(reads.get() + 1);
                if reads.get() > 1 {
                    assert_eq!(count.get(), 1, "readback must wait for notification");
                }
                snapshot(starts.get() == 2)
            },
            || {
                wait_for_configuration(
                    &control,
                    0,
                    || count.get(),
                    || clock.get(),
                    |duration| {
                        clock.set(clock.get() + duration);
                        // Model Apple's notification arriving after the failed readback,
                        // with a fake clock rather than a scheduler-dependent sleep.
                        if clock.get().duration_since(begun) >= Duration::from_millis(150) {
                            count.set(1);
                        }
                    },
                )
            },
        )
        .unwrap();
        assert_eq!(starts.get(), 2);
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn permanently_stopped_engine_has_only_one_restart() {
        let starts = Cell::new(0);
        let failure = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |_| snapshot(false),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            failure,
            Failure::Readback(ReadbackFailure::EngineStoppedAfterStart)
        );
        assert_eq!(starts.get(), 2);
    }

    #[test]
    fn changed_contract_never_retries_even_when_engine_also_stopped() {
        let mut invalid = Vec::new();
        let mut s = snapshot(false);
        s.input_voice_processing = false;
        invalid.push(s);
        let mut s = snapshot(false);
        s.output_voice_processing = false;
        invalid.push(s);
        let mut s = snapshot(false);
        s.agc = true;
        invalid.push(s);
        let mut s = snapshot(false);
        s.bypassed = true;
        invalid.push(s);
        let mut s = snapshot(false);
        s.ducking_matches = false;
        invalid.push(s);
        let mut s = snapshot(false);
        s.configuration_after = 1;
        invalid.push(s);
        for reason in [
            ReadbackFailure::DeviceChanged("UID changed".into()),
            ReadbackFailure::NominalRateChanged("48000 -> 44100".into()),
            ReadbackFailure::DefaultOutputChanged,
        ] {
            let mut s = snapshot(false);
            s.devices = Err(reason);
            invalid.push(s);
        }
        let mut s = snapshot(false);
        s.client_formats = Err(ReadbackFailure::ClientFormatChanged(
            "mono -> stereo".into(),
        ));
        invalid.push(s);
        for s in invalid {
            let starts = Cell::new(0);
            let expected = s.validate().unwrap_err();
            let failure = run(
                true,
                || Ok(()),
                |_| {
                    starts.set(starts.get() + 1);
                    Ok(())
                },
                |_| s.clone(),
                || panic!("invalid contract waited for a retry"),
            )
            .unwrap_err();
            assert_eq!(failure, Failure::Readback(expected));
            assert_eq!(starts.get(), 1);
        }
    }

    #[test]
    fn route_change_while_waiting_refuses_restart() {
        let starts = Cell::new(0);
        let reads = Cell::new(0);
        let failure = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |_| {
                reads.set(reads.get() + 1);
                let mut s = snapshot(false);
                if reads.get() == 2 {
                    s.devices = Err(ReadbackFailure::DefaultOutputChanged);
                }
                s
            },
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(
            failure,
            Failure::Readback(ReadbackFailure::DefaultOutputChanged)
        );
        assert_eq!(starts.get(), 1);
    }

    #[test]
    fn nserror_does_not_wait_or_retry() {
        let starts = Cell::new(0);
        let failure = run(
            true,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Err(Failure::Start("NSError".into()))
            },
            |_| panic!("readback after failed start"),
            || panic!("retry after NSError"),
        )
        .unwrap_err();
        assert_eq!(failure, Failure::Start("NSError".into()));
        assert_eq!(starts.get(), 1);
    }

    #[test]
    fn cancellation_or_shared_deadline_before_retry_prevents_another_start() {
        for cancel in [true, false] {
            let begun = Instant::now();
            let clock = Cell::new(begun);
            let cancelled = AtomicBool::new(false);
            let control = Control {
                cancelled: &cancelled,
                deadline: begun + Duration::from_secs(8),
            };
            let starts = Cell::new(0);
            let result = run(
                true,
                || control.check_at(clock.get()),
                |_| {
                    starts.set(starts.get() + 1);
                    Ok(())
                },
                |_| snapshot(false),
                || {
                    if cancel {
                        cancelled.store(true, Ordering::Release);
                    } else {
                        clock.set(control.deadline);
                    }
                    Ok(())
                },
            );
            assert_eq!(
                result,
                Err(if cancel {
                    Failure::Cancelled
                } else {
                    Failure::DeadlineElapsed
                })
            );
            assert_eq!(starts.get(), 1);
        }
    }

    #[test]
    fn late_successful_start_is_rejected_after_deadline_or_cancellation() {
        for cancel in [true, false] {
            let begun = Instant::now();
            let clock = Cell::new(begun);
            let cancelled = AtomicBool::new(false);
            let control = Control {
                cancelled: &cancelled,
                deadline: begun + Duration::from_secs(8),
            };
            let result = run(
                true,
                || control.check_at(clock.get()),
                |_| {
                    if cancel {
                        cancelled.store(true, Ordering::Release);
                    } else {
                        clock.set(control.deadline);
                    }
                    Ok(())
                },
                |_| snapshot(true),
                || panic!("late successful start must not retry"),
            );
            assert_eq!(
                result,
                Err(if cancel {
                    Failure::Cancelled
                } else {
                    Failure::DeadlineElapsed
                })
            );
        }
    }

    #[test]
    fn every_successful_readback_checks_deadline_and_cancellation_before_acceptance() {
        for successful_stage in 0..3 {
            for cancel in [true, false] {
                let begun = Instant::now();
                let clock = Cell::new(begun);
                let cancelled = AtomicBool::new(false);
                let control = Control {
                    cancelled: &cancelled,
                    deadline: begun + Duration::from_secs(8),
                };
                let result = run(
                    true,
                    || control.check_at(clock.get()),
                    |_| Ok(()),
                    |stage| {
                        if stage == successful_stage {
                            if cancel {
                                cancelled.store(true, Ordering::Release);
                            } else {
                                clock.set(control.deadline);
                            }
                            snapshot(true)
                        } else {
                            snapshot(false)
                        }
                    },
                    || Ok(()),
                );
                assert_eq!(
                    result,
                    Err(if cancel {
                        Failure::Cancelled
                    } else {
                        Failure::DeadlineElapsed
                    })
                );
            }
        }
    }

    #[test]
    fn notification_wait_is_bounded_and_honors_cancellation_and_shared_deadline() {
        for (budget, cancel, expected) in [
            (
                Duration::from_secs(8),
                false,
                Failure::ConfigurationNotificationTimedOut,
            ),
            (Duration::from_millis(20), false, Failure::DeadlineElapsed),
            (Duration::from_secs(8), true, Failure::Cancelled),
        ] {
            let begun = Instant::now();
            let clock = Cell::new(begun);
            let cancelled = AtomicBool::new(false);
            let control = Control {
                cancelled: &cancelled,
                deadline: begun + budget,
            };
            let result = wait_for_configuration(
                &control,
                0,
                || 0,
                || clock.get(),
                |duration| {
                    clock.set(clock.get() + duration);
                    if cancel {
                        cancelled.store(true, Ordering::Release);
                    }
                },
            );
            assert_eq!(result, Err(expected));
            assert!(clock.get().duration_since(begun) <= CONFIGURATION_WAIT);
            assert!(clock.get() <= control.deadline);
        }
    }

    #[test]
    fn no_recovery_without_explicit_owner_control_and_selected_devices() {
        let starts = Cell::new(0);
        let result = run(
            false,
            || Ok(()),
            |_| {
                starts.set(starts.get() + 1);
                Ok(())
            },
            |_| snapshot(false),
            || panic!("uncontrolled retry"),
        );
        assert_eq!(
            result,
            Err(Failure::Readback(ReadbackFailure::EngineStoppedAfterStart))
        );
        assert_eq!(starts.get(), 1);
    }
}
