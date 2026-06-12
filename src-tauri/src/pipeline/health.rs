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
        }
    }

    /// Record the age of a chunk as it is dequeued.
    pub fn note_chunk(&mut self, captured_at: Instant) {
        let lag = captured_at.elapsed();
        if lag > self.window_max_lag {
            self.window_max_lag = lag;
        }
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
        let stalled = queue_depth > 0 && self.last_frame_at.elapsed() > STALL_THRESHOLD;

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
}
