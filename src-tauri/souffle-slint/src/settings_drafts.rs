//! Recoverable Settings editor drafts.
//!
//! `SettingsCache` is the last observed durable snapshot. This module owns the
//! text the user has typed but SQLite has not committed yet. Keeping those two
//! lifecycles separate prevents a failed save from making an optimistic value
//! look canonical, and lets close/quit flush exactly the outstanding batch.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
use souffle_lib::settings::AppSettings;

use crate::settings_values::{SettingsCache, SettingsCommitStatus, save_outcome_commit_status};
use crate::{AppHandle, MainWindow};

const SETTINGS_DRAFT_DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftSavePhase {
    Clean,
    PendingDebounce,
    PendingImmediate,
    RetainedAfterFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitSavePhase {
    Idle,
    Saving,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextDraft {
    text: String,
    revision: u64,
}

#[derive(Debug, Clone)]
struct DraftBatch {
    polish_prompts: HashMap<String, TextDraft>,
    summary_prompts: HashMap<String, TextDraft>,
    summary_names: HashMap<String, TextDraft>,
}

enum QuitFlushDecision {
    NothingPending,
    Start(DraftBatch),
    AlreadySaving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitCompletion {
    Exit,
    ContinueFlush,
    RetainAndShow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftRemainder {
    Empty,
    Pending,
}

impl DraftBatch {
    fn apply_to(&self, settings: &mut AppSettings) {
        for template in &mut settings.dictation_polish_templates {
            if let Some(draft) = self.polish_prompts.get(&template.id) {
                template.prompt.clone_from(&draft.text);
            }
        }
        for template in &mut settings.summary_templates {
            if let Some(draft) = self.summary_prompts.get(&template.id) {
                template.prompt.clone_from(&draft.text);
            }
            if let Some(draft) = self.summary_names.get(&template.id) {
                template.name.clone_from(&draft.text);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.polish_prompts.is_empty()
            && self.summary_prompts.is_empty()
            && self.summary_names.is_empty()
    }
}

struct DraftState {
    next_revision: u64,
    phase: DraftSavePhase,
    quit_phase: QuitSavePhase,
    timer: Option<slint::Timer>,
    polish_prompts: HashMap<String, TextDraft>,
    summary_prompts: HashMap<String, TextDraft>,
    summary_names: HashMap<String, TextDraft>,
}

impl Default for DraftState {
    fn default() -> Self {
        Self {
            next_revision: 0,
            phase: DraftSavePhase::Clean,
            quit_phase: QuitSavePhase::Idle,
            timer: None,
            polish_prompts: HashMap::new(),
            summary_prompts: HashMap::new(),
            summary_names: HashMap::new(),
        }
    }
}

impl DraftState {
    fn next_text(&mut self, text: String) -> TextDraft {
        self.next_revision += 1;
        TextDraft {
            text,
            revision: self.next_revision,
        }
    }

    fn edit_polish_prompt(&mut self, id: String, text: String) {
        let draft = self.next_text(text);
        self.polish_prompts.insert(id, draft);
        self.phase = DraftSavePhase::PendingDebounce;
    }

    fn edit_summary_prompt(&mut self, id: String, text: String) {
        let draft = self.next_text(text);
        self.summary_prompts.insert(id, draft);
        self.phase = DraftSavePhase::PendingDebounce;
    }

    fn edit_summary_name(&mut self, id: String, text: String) {
        let draft = self.next_text(text);
        self.summary_names.insert(id, draft);
        self.phase = DraftSavePhase::PendingImmediate;
    }

    fn batch(&self) -> DraftBatch {
        DraftBatch {
            polish_prompts: self.polish_prompts.clone(),
            summary_prompts: self.summary_prompts.clone(),
            summary_names: self.summary_names.clone(),
        }
    }

    fn begin_timer_flush(&mut self) -> Option<DraftBatch> {
        match self.quit_phase {
            QuitSavePhase::Saving => None,
            QuitSavePhase::Idle => match self.phase {
                DraftSavePhase::PendingDebounce => {
                    self.phase = DraftSavePhase::Clean;
                    Some(self.batch())
                }
                DraftSavePhase::Clean
                | DraftSavePhase::PendingImmediate
                | DraftSavePhase::RetainedAfterFailure => None,
            },
        }
    }

    fn begin_explicit_flush(&mut self) -> Option<DraftBatch> {
        match self.quit_phase {
            QuitSavePhase::Saving => None,
            QuitSavePhase::Idle => match self.phase {
                DraftSavePhase::PendingDebounce
                | DraftSavePhase::PendingImmediate
                | DraftSavePhase::RetainedAfterFailure => {
                    self.phase = DraftSavePhase::Clean;
                    Some(self.batch())
                }
                DraftSavePhase::Clean => None,
            },
        }
    }

    fn begin_quit_flush(&mut self) -> QuitFlushDecision {
        match self.quit_phase {
            QuitSavePhase::Saving => QuitFlushDecision::AlreadySaving,
            QuitSavePhase::Idle => match self.phase {
                DraftSavePhase::Clean => QuitFlushDecision::NothingPending,
                DraftSavePhase::PendingDebounce
                | DraftSavePhase::PendingImmediate
                | DraftSavePhase::RetainedAfterFailure => {
                    let batch = self.batch();
                    self.phase = DraftSavePhase::Clean;
                    self.quit_phase = QuitSavePhase::Saving;
                    QuitFlushDecision::Start(batch)
                }
            },
        }
    }

    fn finish(&mut self, batch: &DraftBatch, status: SettingsCommitStatus) {
        match status {
            SettingsCommitStatus::Committed => {
                remove_committed(&mut self.polish_prompts, &batch.polish_prompts);
                remove_committed(&mut self.summary_prompts, &batch.summary_prompts);
                remove_committed(&mut self.summary_names, &batch.summary_names);
            }
            SettingsCommitStatus::NotCommitted => {}
        }
        if self.batch().is_empty() {
            self.phase = DraftSavePhase::Clean;
        } else if self.phase == DraftSavePhase::Clean {
            self.phase = DraftSavePhase::RetainedAfterFailure;
        }
    }

    fn finish_quit(&mut self, batch: &DraftBatch, status: SettingsCommitStatus) -> QuitCompletion {
        self.finish(batch, status);
        self.quit_phase = QuitSavePhase::Idle;
        match status {
            SettingsCommitStatus::Committed if self.batch().is_empty() => QuitCompletion::Exit,
            SettingsCommitStatus::Committed => QuitCompletion::ContinueFlush,
            SettingsCommitStatus::NotCommitted => QuitCompletion::RetainAndShow,
        }
    }

    fn discard_summary(&mut self, id: &str) -> DraftRemainder {
        self.summary_prompts.remove(id);
        self.summary_names.remove(id);
        if self.batch().is_empty() {
            self.phase = DraftSavePhase::Clean;
            DraftRemainder::Empty
        } else {
            DraftRemainder::Pending
        }
    }
}

fn remove_committed(current: &mut HashMap<String, TextDraft>, saved: &HashMap<String, TextDraft>) {
    current.retain(|id, draft| {
        saved
            .get(id)
            .is_none_or(|saved_draft| draft.revision != saved_draft.revision)
    });
}

fn begin_quit_request(state: &RefCell<DraftState>) -> QuitFlushDecision {
    let decision = { state.borrow_mut().begin_quit_flush() };
    match decision {
        QuitFlushDecision::Start(batch) => {
            state.borrow_mut().timer = None;
            QuitFlushDecision::Start(batch)
        }
        QuitFlushDecision::NothingPending => QuitFlushDecision::NothingPending,
        QuitFlushDecision::AlreadySaving => QuitFlushDecision::AlreadySaving,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftFlushResult {
    NothingPending,
    Committed,
    RetainedAfterFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsSubmission<T> {
    Committed,
    Retained(T),
}

pub(crate) fn settle_settings_submission<T>(
    draft: T,
    outcome: &SettingsSaveOutcome,
) -> SettingsSubmission<T> {
    match save_outcome_commit_status(outcome) {
        SettingsCommitStatus::Committed => SettingsSubmission::Committed,
        SettingsCommitStatus::NotCommitted => SettingsSubmission::Retained(draft),
    }
}

pub(crate) fn summary_editing_id(current: &str, settings: &AppSettings) -> String {
    if settings
        .summary_templates
        .iter()
        .any(|template| template.id == current)
    {
        current.to_string()
    } else {
        settings.default_summary_template_id.clone()
    }
}

pub(crate) struct SettingsDraftController {
    window: slint::Weak<MainWindow>,
    handle: AppHandle,
    cache: SettingsCache,
    state: RefCell<DraftState>,
    quit: Rc<dyn Fn()>,
}

impl SettingsDraftController {
    pub(crate) fn new(window: &MainWindow, handle: AppHandle, cache: SettingsCache) -> Rc<Self> {
        Rc::new(Self {
            window: window.as_weak(),
            handle,
            cache,
            state: RefCell::new(DraftState::default()),
            quit: Rc::new(|| std::process::exit(0)),
        })
    }

    pub(crate) fn edit_polish_prompt(self: &Rc<Self>, id: String, text: String) {
        self.state.borrow_mut().edit_polish_prompt(id, text);
        self.restart_timer();
    }

    pub(crate) fn edit_summary_prompt(self: &Rc<Self>, id: String, text: String) {
        self.state.borrow_mut().edit_summary_prompt(id, text);
        self.restart_timer();
    }

    pub(crate) fn edit_summary_name(&self, id: String, text: String) -> DraftFlushResult {
        self.state.borrow_mut().edit_summary_name(id, text);
        self.flush_explicit()
    }

    fn restart_timer(self: &Rc<Self>) {
        let timer = slint::Timer::default();
        let weak = Rc::downgrade(self);
        timer.start(
            slint::TimerMode::SingleShot,
            SETTINGS_DRAFT_DEBOUNCE,
            move || {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                controller.flush_timer();
            },
        );
        self.state.borrow_mut().timer = Some(timer);
    }

    fn flush_timer(&self) -> DraftFlushResult {
        let batch = self.state.borrow_mut().begin_timer_flush();
        self.save_batch(batch)
    }

    pub(crate) fn flush_explicit(&self) -> DraftFlushResult {
        let batch = self.state.borrow_mut().begin_explicit_flush();
        let result = self.save_batch(batch);
        self.state.borrow_mut().timer = None;
        result
    }

    fn save_batch(&self, batch: Option<DraftBatch>) -> DraftFlushResult {
        let Some(batch) = batch else {
            return DraftFlushResult::NothingPending;
        };
        let outcome = save_field(&self.handle, &self.cache, |settings| {
            batch.apply_to(settings)
        });
        self.publish_draft_status(&outcome);
        let status = save_outcome_commit_status(&outcome);
        self.state.borrow_mut().finish(&batch, status);
        match status {
            SettingsCommitStatus::Committed => DraftFlushResult::Committed,
            SettingsCommitStatus::NotCommitted => DraftFlushResult::RetainedAfterFailure,
        }
    }

    pub(crate) fn reapply_polish_prompt(&self, window: &MainWindow, id: &str) {
        let text = self
            .state
            .borrow()
            .polish_prompts
            .get(id)
            .map(|draft| draft.text.clone());
        if let Some(text) = text {
            window.set_settings_active_dictation_polish_prompt(text.into());
        }
    }

    pub(crate) fn reapply_summary_template(&self, window: &MainWindow, id: &str) {
        let state = self.state.borrow();
        if let Some(draft) = state.summary_names.get(id) {
            window.set_settings_editing_summary_template_name(draft.text.as_str().into());
        }
        if let Some(draft) = state.summary_prompts.get(id) {
            window.set_settings_editing_summary_template_prompt(draft.text.as_str().into());
        }
    }

    pub(crate) fn discard_summary_template(&self, id: &str) {
        match self.state.borrow_mut().discard_summary(id) {
            DraftRemainder::Empty => {
                if let Some(window) = self.window.upgrade() {
                    window.set_settings_draft_save_error("".into());
                }
            }
            DraftRemainder::Pending => {}
        }
    }

    fn publish_draft_status(&self, outcome: &SettingsSaveOutcome) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        window.set_settings_draft_save_error(Self::draft_retention_error(outcome).into());
    }

    /// Only a non-commit means the draft is still pending. Post-commit native
    /// effect or observation failures remain visible through the general
    /// Settings error without incorrectly claiming the draft was retained.
    fn draft_retention_error(outcome: &SettingsSaveOutcome) -> String {
        let detail = match outcome {
            SettingsSaveOutcome::Observed { result, .. } => match result {
                Ok(()) => String::new(),
                Err(error) => error.user_message(),
            },
            SettingsSaveOutcome::Unavailable { result, read_error } => match result {
                Ok(()) => format!("Stored settings are unknown: {read_error}"),
                Err(error) => format!(
                    "{}. Stored settings are unknown: {read_error}",
                    error.user_message()
                ),
            },
        };
        match save_outcome_commit_status(outcome) {
            SettingsCommitStatus::Committed => String::new(),
            SettingsCommitStatus::NotCommitted => detail,
        }
    }

    /// Quit is the one flush that did not exist before SOU-201. Keep its DB
    /// work off Slint's event loop, hide the window to prevent a newer edit
    /// racing the final snapshot, and abort quitting on a non-commit so the
    /// recoverable draft and visible error remain available.
    pub(crate) fn request_quit(self: &Rc<Self>) {
        let batch = match begin_quit_request(&self.state) {
            QuitFlushDecision::NothingPending => {
                (self.quit)();
                return;
            }
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::AlreadySaving => return,
        };
        if let Some(window) = self.window.upgrade() {
            let _ = window.hide();
        }

        let handle = self.handle.clone();
        let cached = self.cache.known_snapshot();
        let worker_batch = batch.clone();
        let worker = souffle_lib::async_runtime::spawn_blocking(move || {
            let mut settings = match cached {
                Some(settings) => settings,
                None => match souffle_lib::commands::get_settings(handle.clone()) {
                    Ok(settings) => settings,
                    Err(read_error) => {
                        return SettingsSaveOutcome::Unavailable {
                            result: Err(SettingsSaveError::NotCommitted {
                                message: "Settings could not be loaded before quitting".into(),
                            }),
                            read_error,
                        };
                    }
                },
            };
            worker_batch.apply_to(&mut settings);
            souffle_lib::commands::save_settings_observed(handle, settings)
        });

        let controller = Rc::clone(self);
        slint::spawn_local(async move {
            let outcome = match worker.await {
                Ok(outcome) => outcome,
                Err(error) => SettingsSaveOutcome::Unavailable {
                    result: Err(SettingsSaveError::NotCommitted {
                        message: format!("Settings save worker failed before quitting: {error}"),
                    }),
                    read_error: "The final durable settings snapshot is unavailable".into(),
                },
            };
            let status = save_outcome_commit_status(&outcome);
            controller.cache.observe_save_outcome(&outcome);
            controller.publish_draft_status(&outcome);
            let completion = controller.state.borrow_mut().finish_quit(&batch, status);
            match completion {
                QuitCompletion::Exit => {
                    (controller.quit)();
                }
                QuitCompletion::ContinueFlush => controller.request_quit(),
                QuitCompletion::RetainAndShow => {
                    if let Some(window) = controller.window.upgrade() {
                        window.set_settings_open(true);
                        let _ = window.show();
                    }
                }
            }
        })
        .expect("slint event loop not running");
    }
}

fn save_field(
    handle: &AppHandle,
    cache: &SettingsCache,
    mutate: impl FnOnce(&mut AppSettings),
) -> SettingsSaveOutcome {
    crate::settings_values::save_field(
        cache,
        || souffle_lib::commands::get_settings(handle.clone()),
        |candidate| souffle_lib::commands::save_settings_observed(handle.clone(), candidate),
        mutate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use souffle_lib::commands::SettingsEffectFailure;
    use std::cell::Cell;

    fn edit_every_family(state: &mut DraftState) {
        state.edit_polish_prompt("polish-default".into(), "polish draft".into());
        state.edit_summary_prompt("summary-default".into(), "summary draft".into());
        state.edit_summary_name("summary-default".into(), "summary name draft".into());
    }

    #[test]
    fn injected_prompt_database_failure_keeps_every_draft_recoverable() {
        let mut state = DraftState::default();
        edit_every_family(&mut state);
        let batch = state.begin_explicit_flush().expect("dirty batch");

        state.finish(&batch, SettingsCommitStatus::NotCommitted);

        assert_eq!(state.phase, DraftSavePhase::RetainedAfterFailure);
        assert_eq!(state.polish_prompts["polish-default"].text, "polish draft");
        assert_eq!(
            state.summary_prompts["summary-default"].text,
            "summary draft"
        );
        assert_eq!(
            state.summary_names["summary-default"].text,
            "summary name draft"
        );
    }

    #[test]
    fn close_before_four_hundred_milliseconds_flushes_the_latest_batch() {
        let mut state = DraftState::default();
        state.edit_summary_prompt("summary-default".into(), "latest text".into());

        let batch = state
            .begin_explicit_flush()
            .expect("close must flush scheduled work");

        assert_eq!(batch.summary_prompts["summary-default"].text, "latest text");
    }

    #[test]
    fn consumed_timer_then_close_does_not_save_the_same_batch_twice() {
        let mut state = DraftState::default();
        let save_count = Cell::new(0);
        state.edit_polish_prompt("polish-default".into(), "saved once".into());
        if let Some(timer_batch) = state.begin_timer_flush() {
            save_count.set(save_count.get() + 1);
            state.finish(&timer_batch, SettingsCommitStatus::Committed);
        }
        if let Some(close_batch) = state.begin_explicit_flush() {
            save_count.set(save_count.get() + 1);
            state.finish(&close_batch, SettingsCommitStatus::Committed);
        }

        assert_eq!(save_count.get(), 1);
        assert_eq!(state.phase, DraftSavePhase::Clean);
    }

    #[test]
    fn injected_quit_database_failure_cancels_quit_and_retains_the_batch() {
        let mut state = DraftState::default();
        state.edit_summary_prompt("summary-default".into(), "do not lose".into());
        let batch = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("quit must start the pending batch")
            }
        };
        assert_eq!(state.quit_phase, QuitSavePhase::Saving);

        let completion = state.finish_quit(&batch, SettingsCommitStatus::NotCommitted);

        assert_eq!(state.phase, DraftSavePhase::RetainedAfterFailure);
        assert_eq!(state.quit_phase, QuitSavePhase::Idle);
        assert_eq!(completion, QuitCompletion::RetainAndShow);
        assert_eq!(state.summary_prompts["summary-default"].text, "do not lose");
    }

    #[test]
    fn late_committed_batch_never_clears_a_newer_edit() {
        let mut state = DraftState::default();
        state.edit_summary_prompt("summary-default".into(), "first".into());
        let first = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("quit must start the first batch")
            }
        };
        state.edit_summary_prompt("summary-default".into(), "newer".into());

        let completion = state.finish_quit(&first, SettingsCommitStatus::Committed);

        assert_eq!(state.summary_prompts["summary-default"].text, "newer");
        assert_eq!(state.phase, DraftSavePhase::PendingDebounce);
        assert!(state.begin_timer_flush().is_some());
        assert_eq!(completion, QuitCompletion::ContinueFlush);
    }

    #[test]
    fn repeated_quit_while_the_worker_is_running_never_skips_the_pending_batch() {
        let mut state = DraftState::default();
        state.edit_polish_prompt("polish-default".into(), "pending quit".into());
        assert!(matches!(
            state.begin_quit_flush(),
            QuitFlushDecision::Start(_)
        ));

        assert!(matches!(
            state.begin_quit_flush(),
            QuitFlushDecision::AlreadySaving
        ));
        assert_eq!(state.quit_phase, QuitSavePhase::Saving);
        assert_eq!(state.polish_prompts["polish-default"].text, "pending quit");
    }

    #[test]
    fn quit_request_releases_the_state_borrow_before_cancelling_the_timer() {
        let state = RefCell::new(DraftState::default());
        state
            .borrow_mut()
            .edit_polish_prompt("polish-default".into(), "pending quit".into());

        let decision = begin_quit_request(&state);

        match decision {
            QuitFlushDecision::Start(batch) => {
                assert_eq!(batch.polish_prompts["polish-default"].text, "pending quit");
                assert_eq!(state.borrow().quit_phase, QuitSavePhase::Saving);
            }
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("quit must start without a nested RefCell borrow")
            }
        }
    }

    #[test]
    fn timer_during_quit_is_suspended_then_the_newer_edit_gets_a_second_batch() {
        let mut state = DraftState::default();
        state.edit_polish_prompt("polish-default".into(), "first batch".into());
        let first = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("first quit must start its batch")
            }
        };
        state.edit_polish_prompt("polish-default".into(), "newer edit".into());

        assert!(matches!(
            state.begin_quit_flush(),
            QuitFlushDecision::AlreadySaving
        ));
        assert_eq!(state.phase, DraftSavePhase::PendingDebounce);
        assert!(
            state.begin_timer_flush().is_none(),
            "timer must not race the quit worker"
        );

        let completion = state.finish_quit(&first, SettingsCommitStatus::Committed);
        assert_eq!(completion, QuitCompletion::ContinueFlush);
        let follow_up = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("newer edit must start a second quit batch")
            }
        };
        assert_eq!(
            follow_up.polish_prompts["polish-default"].text,
            "newer edit"
        );
    }

    #[test]
    fn immediate_name_flush_during_quit_is_suspended_then_chained() {
        let mut state = DraftState::default();
        state.edit_polish_prompt("polish-default".into(), "first batch".into());
        let first = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("first quit must start its batch")
            }
        };
        state.edit_summary_name("summary-default".into(), "newer name".into());

        assert!(
            state.begin_explicit_flush().is_none(),
            "explicit save must not race the quit worker"
        );
        assert_eq!(state.phase, DraftSavePhase::PendingImmediate);

        assert_eq!(
            state.finish_quit(&first, SettingsCommitStatus::Committed),
            QuitCompletion::ContinueFlush
        );
        let follow_up = match state.begin_quit_flush() {
            QuitFlushDecision::Start(batch) => batch,
            QuitFlushDecision::NothingPending | QuitFlushDecision::AlreadySaving => {
                panic!("newer name must start a second quit batch")
            }
        };
        assert_eq!(
            follow_up.summary_names["summary-default"].text,
            "newer name"
        );
    }

    #[test]
    fn summary_name_is_pending_immediate_before_its_first_attempt() {
        let mut state = DraftState::default();

        state.edit_summary_name("summary-default".into(), "Exact draft".into());

        assert_eq!(state.phase, DraftSavePhase::PendingImmediate);
        assert!(state.begin_explicit_flush().is_some());
    }

    #[test]
    fn first_open_edit_target_exists_before_provider_refresh_and_close_flushes_it() {
        let settings = AppSettings::default();
        let editing_id = summary_editing_id("", &settings);
        assert!(!editing_id.is_empty());
        assert_eq!(
            summary_editing_id("deleted-template", &settings),
            settings.default_summary_template_id
        );

        let mut state = DraftState::default();
        state.edit_summary_name(editing_id.clone(), "Edited before refresh".into());
        let batch = state
            .begin_explicit_flush()
            .expect("close flushes the first-open edit");
        let mut candidate = settings;
        batch.apply_to(&mut candidate);

        assert_eq!(
            candidate
                .summary_templates
                .iter()
                .find(|template| template.id == editing_id)
                .expect("default summary template")
                .name,
            "Edited before refresh"
        );
    }

    #[test]
    fn injected_template_add_failure_retains_the_name_but_after_commit_effect_failure_does_not() {
        let observed = AppSettings::default();
        let rejected = SettingsSaveOutcome::Observed {
            settings: Box::new(observed.clone()),
            result: Err(SettingsSaveError::NotCommitted {
                message: "injected template database failure".into(),
            }),
        };
        assert_eq!(
            settle_settings_submission("  Weekly sync  ".to_string(), &rejected),
            SettingsSubmission::Retained("  Weekly sync  ".to_string())
        );

        let committed = SettingsSaveOutcome::Observed {
            settings: Box::new(observed),
            result: Err(SettingsSaveError::EffectFailedAfterCommit {
                cause: SettingsEffectFailure::Logging {
                    message: "injected post-commit effect failure".into(),
                },
            }),
        };
        assert_eq!(
            settle_settings_submission("Weekly sync".to_string(), &committed),
            SettingsSubmission::Committed
        );
    }

    #[test]
    fn draft_error_is_reserved_for_non_committed_outcomes() {
        let observed = AppSettings::default();
        let rejected = SettingsSaveOutcome::Observed {
            settings: Box::new(observed.clone()),
            result: Err(SettingsSaveError::NotCommitted {
                message: "injected database failure".into(),
            }),
        };
        assert_eq!(
            SettingsDraftController::draft_retention_error(&rejected),
            "injected database failure"
        );

        let effect_failed = SettingsSaveOutcome::Observed {
            settings: Box::new(observed),
            result: Err(SettingsSaveError::EffectFailedAfterCommit {
                cause: SettingsEffectFailure::Logging {
                    message: "injected post-commit effect failure".into(),
                },
            }),
        };
        assert!(SettingsDraftController::draft_retention_error(&effect_failed).is_empty());

        let observation_failed = SettingsSaveOutcome::Unavailable {
            result: Ok(()),
            read_error: "injected observation failure".into(),
        };
        assert!(SettingsDraftController::draft_retention_error(&observation_failed).is_empty());
    }

    #[test]
    fn successful_template_delete_clears_the_last_retained_draft_state() {
        let mut state = DraftState::default();
        state.edit_summary_prompt("custom".into(), "retained prompt".into());
        let batch = state
            .begin_explicit_flush()
            .expect("pending template draft");
        state.finish(&batch, SettingsCommitStatus::NotCommitted);
        assert_eq!(state.phase, DraftSavePhase::RetainedAfterFailure);

        assert_eq!(state.discard_summary("custom"), DraftRemainder::Empty);
        assert_eq!(state.phase, DraftSavePhase::Clean);
    }
}
