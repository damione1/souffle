//! Capture of a recording session that is still starting (SOU-260).
//!
//! Capture begins on the click, before the engine is ready for the session:
//! a diarized Kyutai reset takes seconds, and whatever is said meanwhile is
//! buffered and fed to the engine once it is. A stop that lands during that
//! window (push-to-talk released early, Stop clicked on "Starting…") cannot
//! stop the session yet, since the session does not exist until the engine
//! answers; the UI replays it once it does (SOU-258). The microphone must
//! not keep recording until then, though: this gate stops capture at the
//! moment the user asked, whichever of "capture started" and "stop asked"
//! comes first, and remembers when that was so the meeting's duration ends
//! there too.

use std::sync::Mutex;

use chrono::{DateTime, Utc};

/// Shared by the start path (worker thread) and the UI's stop handler.
#[derive(Default)]
pub struct CaptureStartGate {
    state: Mutex<Option<StartingSession>>,
}

#[derive(Debug)]
struct StartingSession {
    session_id: u64,
    capture_running: bool,
    /// When the user stopped the session while it was starting.
    halted_at: Option<DateTime<Utc>>,
    /// The engine answered: the session exists and stops the normal way.
    settled: bool,
}

impl CaptureStartGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut Option<StartingSession>) -> T) -> T {
        match self.state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }

    /// A start for `session_id` is under way. Forgets any previous one.
    pub fn begin(&self, session_id: u64) {
        self.with_state(|state| {
            *state = Some(StartingSession {
                session_id,
                capture_running: false,
                halted_at: None,
                settled: false,
            });
        });
    }

    /// Start capture for `session_id` with `start`. If the user already
    /// stopped the session, `stop` runs right after, so capture never
    /// outlives the stop. Both run under the gate's lock: a concurrent
    /// [`Self::halt`] sees capture either not started (and leaves the stop to
    /// this call) or running (and stops it itself), never in between.
    pub fn start_capture(
        &self,
        session_id: u64,
        start: impl FnOnce() -> Result<(), String>,
        stop: impl FnOnce(),
    ) -> Result<(), String> {
        self.with_state(|state| {
            start()?;
            if let Some(session) = state.as_mut().filter(|s| s.session_id == session_id) {
                session.capture_running = true;
                if session.halted_at.is_some() {
                    stop();
                    // Capture ran until now, not until the earlier stop:
                    // the session's end must not precede its start, which
                    // the caller took just before this call.
                    session.halted_at = Some(Utc::now());
                }
            }
            Ok(())
        })
    }

    /// The user stopped while a start is in flight: stop capture now (or as
    /// soon as it starts). Returns whether a starting session took the stop;
    /// `false` means there is none, and the caller stops the normal way.
    pub fn halt(&self, stop: impl FnOnce()) -> bool {
        self.with_state(|state| {
            let Some(session) = state.as_mut().filter(|s| !s.settled) else {
                return false;
            };
            if session.halted_at.is_none() {
                session.halted_at = Some(Utc::now());
                if session.capture_running {
                    stop();
                }
            }
            true
        })
    }

    /// The engine is ready: from here on the session stops the normal way.
    pub fn settle(&self, session_id: u64) {
        self.with_state(|state| {
            if let Some(session) = state.as_mut().filter(|s| s.session_id == session_id) {
                session.settled = true;
            }
        });
    }

    /// The start failed: nothing is left to stop.
    pub fn abandon(&self, session_id: u64) {
        self.with_state(|state| {
            if state.as_ref().is_some_and(|s| s.session_id == session_id) {
                *state = None;
            }
        });
    }

    /// Called by the stop of the session: when it was stopped during its
    /// start, the time the user asked, which is when capture ended. Clears
    /// the gate either way.
    pub fn take_halted_at(&self) -> Option<DateTime<Utc>> {
        self.with_state(|state| state.take().and_then(|s| s.halted_at))
    }

    /// A start is under way and the engine has not answered yet.
    pub fn start_in_flight(&self) -> bool {
        self.with_state(|state| state.as_ref().is_some_and(|s| !s.settled))
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureStartGate;
    use std::cell::RefCell;

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Sent {
        Start,
        Stop,
    }

    fn start_capture(gate: &CaptureStartGate, session_id: u64, log: &RefCell<Vec<Sent>>) {
        gate.start_capture(
            session_id,
            || {
                log.borrow_mut().push(Sent::Start);
                Ok(())
            },
            || log.borrow_mut().push(Sent::Stop),
        )
        .unwrap();
    }

    #[test]
    fn a_stop_after_capture_started_stops_it_at_once() {
        let gate = CaptureStartGate::new();
        let log = RefCell::new(Vec::new());
        gate.begin(1);
        start_capture(&gate, 1, &log);

        assert!(gate.halt(|| log.borrow_mut().push(Sent::Stop)));
        // A second stop (tray + shortcut) does not stop twice.
        assert!(gate.halt(|| log.borrow_mut().push(Sent::Stop)));

        assert_eq!(*log.borrow(), vec![Sent::Start, Sent::Stop]);
        assert!(gate.take_halted_at().is_some());
    }

    #[test]
    fn a_stop_before_capture_started_stops_it_right_after_it_starts() {
        let gate = CaptureStartGate::new();
        let log = RefCell::new(Vec::new());
        gate.begin(1);

        assert!(gate.halt(|| log.borrow_mut().push(Sent::Stop)));
        assert!(log.borrow().is_empty(), "nothing to stop yet");
        let capture_started_at = chrono::Utc::now();
        start_capture(&gate, 1, &log);

        assert_eq!(*log.borrow(), vec![Sent::Start, Sent::Stop]);
        // The session ends when its capture did, never before it began.
        let ended = gate.take_halted_at().expect("halted");
        assert!(ended >= capture_started_at);
    }

    #[test]
    fn once_the_engine_answered_a_stop_goes_the_normal_way() {
        let gate = CaptureStartGate::new();
        let log = RefCell::new(Vec::new());
        gate.begin(1);
        start_capture(&gate, 1, &log);
        assert!(gate.start_in_flight());
        gate.settle(1);
        assert!(!gate.start_in_flight());

        assert!(!gate.halt(|| log.borrow_mut().push(Sent::Stop)));
        assert_eq!(*log.borrow(), vec![Sent::Start]);
        assert!(gate.take_halted_at().is_none());
    }

    #[test]
    fn no_start_under_way_takes_no_stop() {
        let gate = CaptureStartGate::new();
        assert!(!gate.halt(|| panic!("nothing to stop")));
        gate.begin(1);
        gate.abandon(1);
        assert!(!gate.halt(|| panic!("the start failed")));
    }

    #[test]
    fn a_new_start_forgets_the_previous_stop() {
        let gate = CaptureStartGate::new();
        gate.begin(1);
        assert!(gate.halt(|| {}));
        gate.begin(2);
        let log = RefCell::new(Vec::new());
        start_capture(&gate, 2, &log);
        assert_eq!(*log.borrow(), vec![Sent::Start]);
        assert!(gate.take_halted_at().is_none());
    }

    #[test]
    fn a_failed_capture_start_is_reported() {
        let gate = CaptureStartGate::new();
        gate.begin(1);
        let result = gate.start_capture(1, || Err("Audio start: closed".into()), || {});
        assert_eq!(result, Err("Audio start: closed".to_string()));
    }
}
