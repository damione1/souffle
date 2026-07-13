//! Per-session pipeline health tracking: lag, queue depth, dropped chunks.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::app_events::{HealthStatus, TranscriptionHealth};

/// How often health snapshots are emitted during a session.
const EMIT_INTERVAL: Duration = Duration::from_secs(1);
/// Chunk age above which the pipeline is considered lagging.
const LAG_THRESHOLD: Duration = Duration::from_millis(1500);
/// No frame processed for this long while audio is queued = stalled.
const STALL_THRESHOLD: Duration = Duration::from_secs(5);
/// How long audio chunks may keep arriving with zero engine frames ever
/// processed before the startup watchdog treats the session as stalled.
///
/// This closes a blind spot in the queue-depth check above: `STALL_THRESHOLD`
/// only fires once the audio channel backs up (`queue_depth > 0`). A session
/// whose mode never coalesces a processable frame, e.g. a diarized lane that
/// never fills, a wedged per-mode buffer, an engine that silently accepts
/// frames it never returns from `frame_ready()` for, drains every chunk as
/// it arrives into buffers instead of leaving them queued, so the channel
/// never backs up and that check never trips, even though capture is alive
/// (audio level bars keep moving) and the engine has never produced a single
/// frame. This uses a longer window than `STALL_THRESHOLD` since it must
/// also tolerate a session's legitimate startup latency (model warm-up,
/// silence prefix, first-frame buffering).
const STARTUP_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(20);

/// Tracks pipeline health for one session. Lives on the actor thread;
/// only the drop counter is shared (with the capture callback).
pub struct SessionHealth {
    session_id: u64,
    dropped: Arc<AtomicU64>,
    last_emit: Instant,
    /// Worst chunk age observed since the last emit.
    window_max_lag: Duration,
    /// Drop count at the last emit, to detect new drops per window.
    drops_at_last_emit: u64,
    last_frame_at: Instant,
    /// When an audio chunk was last dequeued for this session, so the
    /// startup watchdog can tell "capture is still alive" apart from "audio
    /// stopped arriving entirely" (a different failure, handled elsewhere).
    /// `None` until the first chunk arrives.
    last_chunk_at: Option<Instant>,
}

impl SessionHealth {
    /// Starts tracking and resets the shared drop counter for the new session.
    pub fn start(session_id: u64, dropped: Arc<AtomicU64>) -> Self {
        dropped.store(0, Ordering::Relaxed);
        let now = Instant::now();
        Self {
            session_id,
            dropped,
            last_emit: now,
            window_max_lag: Duration::ZERO,
            drops_at_last_emit: 0,
            last_frame_at: now,
            last_chunk_at: None,
        }
    }

    /// Record the age of a chunk as it is dequeued.
    pub fn note_chunk(&mut self, captured_at: Instant) {
        let lag = captured_at.elapsed();
        if lag > self.window_max_lag {
            self.window_max_lag = lag;
        }
        self.last_chunk_at = Some(Instant::now());
    }

    /// Record that a frame went through the engine (or was VAD-skipped).
    pub fn note_frame(&mut self) {
        self.last_frame_at = Instant::now();
    }

    pub fn dropped_chunks(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Returns a snapshot once per emit interval, None otherwise.
    pub fn tick(&mut self, queue_depth: usize) -> Option<TranscriptionHealth> {
        if self.last_emit.elapsed() < EMIT_INTERVAL {
            return None;
        }

        let dropped = self.dropped.load(Ordering::Relaxed);
        let new_drops = dropped > self.drops_at_last_emit;
        let queue_stalled = queue_depth > 0 && self.last_frame_at.elapsed() > STALL_THRESHOLD;
        // Startup watchdog: audio is actively arriving (a chunk within the
        // last STALL_THRESHOLD) but no engine frame has ever landed, or
        // frames stopped landing, for STARTUP_WATCHDOG_TIMEOUT. Independent
        // of queue depth, so it catches a session mode that drains chunks
        // into buffers without ever reaching `frame_ready()`.
        let capture_alive = self
            .last_chunk_at
            .is_some_and(|t| t.elapsed() <= STALL_THRESHOLD);
        let startup_stalled =
            capture_alive && self.last_frame_at.elapsed() > STARTUP_WATCHDOG_TIMEOUT;
        let stalled = queue_stalled || startup_stalled;

        let status = if stalled {
            HealthStatus::Stalled
        } else if self.window_max_lag >= LAG_THRESHOLD || new_drops {
            HealthStatus::Lagging
        } else {
            HealthStatus::Healthy
        };

        let snapshot = TranscriptionHealth {
            session_id: self.session_id as u32,
            status,
            queue_depth: queue_depth as u32,
            lag_ms: self.window_max_lag.as_millis() as u32,
            dropped_chunks: dropped as u32,
        };

        self.last_emit = Instant::now();
        self.window_max_lag = Duration::ZERO;
        self.drops_at_last_emit = dropped;
        Some(snapshot)
    }
}

/// How long a session must be continuously stalled before the recovery
/// ladder attempts an in-place engine reset.
const RESET_AFTER: Duration = Duration::from_secs(30);
/// How much additional continuous stall time, after the reset attempt, the
/// ladder gives the engine before ending the session.
const ABORT_AFTER: Duration = Duration::from_secs(30);

/// Action for `run_session_loop` to take in response to a stall episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallAction {
    /// First rung: try to unstick the engine in place.
    Reset,
    /// Second rung: the reset didn't help (or was never reached because the
    /// stall persisted); end the session.
    Abort,
}

/// Tracks continuous stall duration across health snapshots and decides
/// when to attempt recovery versus give up. Pure decision logic (no I/O, no
/// engine access), so it can be driven and unit-tested independently of
/// `run_session_loop`.
///
/// Fed by two distinct `SessionHealth` stall signals, both funneled into the
/// same `HealthStatus::Stalled` status so this ladder doesn't need to know
/// which one fired: the audio queue backing up with no frames processed
/// (`STALL_THRESHOLD`), and the startup watchdog (audio still arriving but
/// zero frames ever landing, or landing then stopping, for
/// `STARTUP_WATCHDOG_TIMEOUT`), independent of queue depth.
///
/// Known limitation: this only detects a session loop that is alive but not
/// producing frames. A stall where the engine blocks forever *inside* a
/// single `transcribe` call never returns control to the loop that drives
/// this struct, so the ladder cannot see or recover from that case. Covering
/// it would need a cross-thread watchdog, which is out of scope here.
pub struct StallRecovery {
    stalled_since: Option<Instant>,
    reset_attempted: bool,
}

impl StallRecovery {
    pub fn new() -> Self {
        Self {
            stalled_since: None,
            reset_attempted: false,
        }
    }

    /// Feed one health snapshot's status and the time it was taken. Returns
    /// the action to take, if any. Any non-stalled status clears the
    /// episode: frames are flowing again, so the next stall starts a fresh
    /// count.
    pub fn on_snapshot(&mut self, status: HealthStatus, now: Instant) -> Option<StallAction> {
        if status != HealthStatus::Stalled {
            self.stalled_since = None;
            self.reset_attempted = false;
            return None;
        }

        let stalled_since = *self.stalled_since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(stalled_since);

        if !self.reset_attempted {
            if elapsed >= RESET_AFTER {
                self.reset_attempted = true;
                return Some(StallAction::Reset);
            }
            return None;
        }

        if elapsed >= RESET_AFTER + ABORT_AFTER {
            return Some(StallAction::Abort);
        }
        None
    }
}

impl Default for StallRecovery {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_snapshot_before_interval() {
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        assert!(h.tick(0).is_none());
    }

    #[test]
    fn start_resets_drop_counter() {
        let counter = Arc::new(AtomicU64::new(42));
        let h = SessionHealth::start(1, Arc::clone(&counter));
        assert_eq!(h.dropped_chunks(), 0);
    }

    #[test]
    fn snapshot_reports_lagging_on_old_chunks() {
        let mut h = SessionHealth::start(7, Arc::new(AtomicU64::new(0)));
        h.note_chunk(Instant::now() - Duration::from_secs(3));
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(5).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Lagging);
        assert_eq!(snap.session_id, 7);
        assert_eq!(snap.queue_depth, 5);
        assert!(snap.lag_ms >= 1500);
    }

    #[test]
    fn snapshot_reports_stalled_when_no_frames_with_queue() {
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.last_frame_at = Instant::now() - Duration::from_secs(6);
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(10).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Stalled);
    }

    #[test]
    fn snapshot_reports_stalled_via_startup_watchdog_with_empty_queue() {
        // Chunks are actively arriving (capture is alive, so UI level bars
        // would be moving) but no engine frame has ever landed for well past
        // the startup watchdog window, and the queue is empty the whole time
        // (every chunk is being drained into a mode buffer, not backing up).
        // The old queue-depth-only check would never flag this.
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.last_frame_at = Instant::now() - Duration::from_secs(21);
        h.last_chunk_at = Some(Instant::now());
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Stalled);
        assert_eq!(snap.queue_depth, 0);
    }

    #[test]
    fn snapshot_not_stalled_before_startup_watchdog_timeout() {
        // Same shape as above but still within the startup grace period
        // (model warm-up, silence prefix, first-frame buffering all take
        // real time) must not fire early.
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.last_frame_at = Instant::now() - Duration::from_secs(19);
        h.last_chunk_at = Some(Instant::now());
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Healthy);
    }

    #[test]
    fn snapshot_not_stalled_via_startup_watchdog_when_capture_inactive() {
        // No frames AND no recent chunks: capture itself has gone quiet
        // (a different failure, handled elsewhere, e.g. AudioGone). The
        // startup watchdog must not also fire here; it only covers "capture
        // is alive but the engine never produces a frame".
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.last_frame_at = Instant::now() - Duration::from_secs(21);
        h.last_chunk_at = Some(Instant::now() - Duration::from_secs(10));
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Healthy);
    }

    #[test]
    fn snapshot_not_stalled_via_startup_watchdog_before_any_chunk() {
        // Session just started, no chunk has arrived yet at all: nothing
        // abnormal, still within ordinary startup latency.
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.last_frame_at = Instant::now() - Duration::from_secs(21);
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Healthy);
    }

    #[test]
    fn note_chunk_marks_capture_alive() {
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        assert!(h.last_chunk_at.is_none());
        h.note_chunk(Instant::now());
        assert!(h.last_chunk_at.is_some());
    }

    #[test]
    fn snapshot_reports_lagging_on_new_drops() {
        let counter = Arc::new(AtomicU64::new(0));
        let mut h = SessionHealth::start(1, Arc::clone(&counter));
        counter.store(3, Ordering::Relaxed);
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Lagging);
        assert_eq!(snap.dropped_chunks, 3);
    }

    #[test]
    fn snapshot_healthy_when_keeping_up() {
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        h.note_chunk(Instant::now());
        h.note_frame();
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, crate::app_events::HealthStatus::Healthy);
    }

    #[test]
    fn stall_recovery_no_action_while_not_stalled() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        assert_eq!(r.on_snapshot(HealthStatus::Healthy, t0), None);
        assert_eq!(r.on_snapshot(HealthStatus::Lagging, t0), None);
    }

    #[test]
    fn stall_recovery_no_action_before_reset_threshold() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        assert_eq!(r.on_snapshot(HealthStatus::Stalled, t0), None);
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(29)),
            None
        );
    }

    #[test]
    fn stall_recovery_resets_at_30s() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        r.on_snapshot(HealthStatus::Stalled, t0);
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(30)),
            Some(StallAction::Reset)
        );
    }

    #[test]
    fn stall_recovery_does_not_reset_twice_in_one_episode() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        r.on_snapshot(HealthStatus::Stalled, t0);
        r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(30));
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(45)),
            None
        );
    }

    #[test]
    fn stall_recovery_aborts_60s_after_initial_stall() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        r.on_snapshot(HealthStatus::Stalled, t0);
        r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(30));
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(60)),
            Some(StallAction::Abort)
        );
    }

    #[test]
    fn stall_recovery_rearms_after_healthy_snapshot() {
        let mut r = StallRecovery::new();
        let t0 = Instant::now();
        r.on_snapshot(HealthStatus::Stalled, t0);
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(30)),
            Some(StallAction::Reset)
        );
        // Frames resume: the episode is cleared entirely.
        let t1 = t0 + Duration::from_secs(31);
        assert_eq!(r.on_snapshot(HealthStatus::Healthy, t1), None);

        // A fresh stall needs its own full 30s before resetting again.
        assert_eq!(r.on_snapshot(HealthStatus::Stalled, t1), None);
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t1 + Duration::from_secs(30)),
            Some(StallAction::Reset)
        );
    }

    #[test]
    fn startup_watchdog_status_drives_the_same_recovery_ladder() {
        // End-to-end: a session where audio keeps arriving but the engine
        // never produces a single frame (queue never backs up, so the old
        // check would stay silent forever) must still reach Reset then
        // Abort through the exact same `StallRecovery` ladder as a
        // queue-depth stall.
        let mut h = SessionHealth::start(1, Arc::new(AtomicU64::new(0)));
        let mut r = StallRecovery::new();

        // Chunks arrive continuously; the engine never processes one.
        h.last_chunk_at = Some(Instant::now());
        h.last_frame_at = Instant::now() - Duration::from_secs(21);
        h.last_emit = Instant::now() - Duration::from_secs(2);
        let snap = h.tick(0).expect("snapshot due");
        assert_eq!(snap.status, HealthStatus::Stalled);
        let t0 = Instant::now();
        assert_eq!(r.on_snapshot(snap.status, t0), None);

        // Simulate later snapshots (chunk arrival kept refreshing, frames
        // still never landed) by re-driving `on_snapshot` directly at the
        // ladder's own thresholds, same as the queue-depth tests above.
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(30)),
            Some(StallAction::Reset)
        );
        assert_eq!(
            r.on_snapshot(HealthStatus::Stalled, t0 + Duration::from_secs(60)),
            Some(StallAction::Abort)
        );
    }
}
