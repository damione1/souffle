//! Per-meeting summary lifecycle independent of which detail happens to be open.
//! Owned values are cloned under a short lock before any UI dispatch or await.
use souffle_lib::{commands::SummaryCommandError, summary::SummarizeProgress};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use uuid::Uuid;

/// A generation carries only the payload valid in its current lifecycle phase.
#[derive(Debug, Clone)]
pub enum SummaryJob {
    Running {
        generation: Uuid,
        progress: Option<SummarizeProgress>,
    },
    Succeeded {
        generation: Uuid,
    },
    Failed {
        generation: Uuid,
        error: SummaryCommandError,
    },
}
impl SummaryJob {
    /// Generation identity used to reject old progress/completion callbacks.
    pub fn generation(&self) -> Uuid {
        match self {
            Self::Running { generation, .. }
            | Self::Succeeded { generation }
            | Self::Failed { generation, .. } => *generation,
        }
    }
}
/// One in-flight summary per meeting; navigation does not change this store.
#[derive(Default)]
pub struct SummaryJobs {
    jobs: HashMap<String, SummaryJob>,
}
impl SummaryJobs {
    /// Process-local coordinator shared by every native detail projection.
    pub fn shared() -> Arc<Mutex<Self>> {
        static JOBS: OnceLock<Arc<Mutex<SummaryJobs>>> = OnceLock::new();
        JOBS.get_or_init(|| Arc::new(Mutex::new(Self::default())))
            .clone()
    }
    /// Reserve a generation, refusing duplicates while this meeting is running.
    pub fn begin(&mut self, id: &str) -> Option<Uuid> {
        match self.jobs.get(id) {
            Some(SummaryJob::Running { .. }) => return None,
            Some(SummaryJob::Succeeded { .. } | SummaryJob::Failed { .. }) | None => {}
        }
        let generation = Uuid::new_v4();
        self.jobs.insert(
            id.into(),
            SummaryJob::Running {
                generation,
                progress: None,
            },
        );
        Some(generation)
    }
    /// Snapshot for a newly opened/restored detail, including errors while away.
    pub fn get(&self, id: &str) -> Option<SummaryJob> {
        self.jobs.get(id).cloned()
    }
    /// Accept progress only while its exact generation is still running.
    pub fn progress(&mut self, id: &str, generation: Uuid, progress: SummarizeProgress) -> bool {
        match self.jobs.get_mut(id) {
            Some(SummaryJob::Running {
                generation: current,
                progress: value,
            }) if *current == generation => {
                *value = Some(progress);
                true
            }
            Some(
                SummaryJob::Running { .. }
                | SummaryJob::Succeeded { .. }
                | SummaryJob::Failed { .. },
            )
            | None => false,
        }
    }
    /// Retain terminal results even when their meeting is not on screen.
    pub fn complete(
        &mut self,
        id: &str,
        generation: Uuid,
        result: Result<(), SummaryCommandError>,
    ) -> bool {
        match self.jobs.get(id) {
            Some(SummaryJob::Running {
                generation: current,
                ..
            }) if *current == generation => {}
            Some(
                SummaryJob::Running { .. }
                | SummaryJob::Succeeded { .. }
                | SummaryJob::Failed { .. },
            )
            | None => return false,
        }
        self.jobs.insert(
            id.into(),
            match result {
                Ok(()) => SummaryJob::Succeeded { generation },
                Err(error) => SummaryJob::Failed { generation, error },
            },
        );
        true
    }
}
/// Project a snapshot after releasing its store lock. Only native display turns
/// the typed failure into text; the store keeps the original error variant.
pub fn project(window: &crate::MainWindow, id: &str) {
    let snapshot = SummaryJobs::shared()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id);
    match snapshot {
        Some(SummaryJob::Running {
            generation,
            progress,
        }) => {
            window.set_meeting_detail_summary_generation_id(generation.to_string().into());
            window.set_meeting_detail_summary_is_generating(true);
            window.set_meeting_detail_summary_generation_progress(
                progress.map(|p| p.text).unwrap_or_default().into(),
            );
            window.set_meeting_detail_summary_generation_error("".into());
        }
        Some(SummaryJob::Succeeded { generation }) => {
            window.set_meeting_detail_summary_generation_id(generation.to_string().into());
            window.set_meeting_detail_summary_is_generating(false);
            window.set_meeting_detail_summary_generation_progress("".into());
            window.set_meeting_detail_summary_generation_error("".into());
        }
        Some(SummaryJob::Failed { generation, error }) => {
            window.set_meeting_detail_summary_generation_id(generation.to_string().into());
            window.set_meeting_detail_summary_is_generating(false);
            window.set_meeting_detail_summary_generation_progress("".into());
            window.set_meeting_detail_summary_generation_error(error.to_string().into());
        }
        None => {
            window.set_meeting_detail_summary_generation_id("".into());
            window.set_meeting_detail_summary_is_generating(false);
            window.set_meeting_detail_summary_generation_progress("".into());
            window.set_meeting_detail_summary_generation_error("".into());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summary_jobs_survive_navigation_reject_duplicates_and_stale_results() {
        let mut jobs = SummaryJobs::default();
        let first = jobs.begin("meeting-a").unwrap();
        let progress = SummarizeProgress {
            text: "partial summary".into(),
            done: false,
            stage: souffle_lib::summary::SummarizeStage::Final,
            current: None,
            total: None,
        };
        assert!(jobs.progress("meeting-a", first, progress.clone()));
        // Leave for B, then return to A: progress and the running reservation survive.
        assert!(jobs.get("meeting-b").is_none());
        assert!(
            matches!(jobs.get("meeting-a"),Some(SummaryJob::Running{progress:Some(value),..}) if value.text=="partial summary")
        );
        assert!(jobs.begin("meeting-a").is_none());
        assert!(jobs.complete(
            "meeting-a",
            first,
            Err(SummaryCommandError::Generation {
                detail: "provider failed while away".into()
            })
        ));
        assert!(matches!(
            jobs.get("meeting-a"),
            Some(SummaryJob::Failed { .. })
        ));
        let next = jobs.begin("meeting-a").unwrap();
        assert!(!jobs.complete("meeting-a", first, Ok(())));
        assert!(!jobs.progress("meeting-a", first, progress));
        assert_eq!(jobs.get("meeting-a").unwrap().generation(), next);
        assert!(jobs.complete("meeting-a", next, Ok(())));
        assert!(matches!(
            jobs.get("meeting-a"),
            Some(SummaryJob::Succeeded { .. })
        ));
    }
}
