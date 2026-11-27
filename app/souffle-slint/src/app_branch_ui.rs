//! App rules use the shared polish catalogue and the pipeline's resolver.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::settings_drafts::SettingsDraftController;
use crate::settings_io::{SettingsIoCoordinator, SettingsLoadToken};
use crate::settings_values::SettingsCache;
use crate::{AppBranchRow, MainWindow, ia_ui};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use souffle_lib::commands::SettingsSaveLane;
use souffle_lib::settings::{
    AppBranchId, AppBranchRule, AppBranchTarget, AppSettings, PolishTemplateId,
};

fn sync_model<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>) -> ModelRc<T> {
    if let Some(model) = current.as_any().downcast_ref::<VecModel<T>>() {
        for (index, row) in rows.iter().enumerate() {
            match model.row_data(index) {
                Some(existing) if existing == *row => {}
                Some(_) => model.set_row_data(index, row.clone()),
                None => model.push(row.clone()),
            }
        }
        while model.row_count() > rows.len() {
            model.remove(model.row_count() - 1);
        }
        current
    } else {
        Rc::new(VecModel::from(rows)).into()
    }
}

fn targets(settings: &AppSettings) -> Vec<AppBranchTarget> {
    std::iter::once(AppBranchTarget::Global)
        .chain(
            settings
                .dictation_polish_templates
                .iter()
                .map(|template| AppBranchTarget::Template(PolishTemplateId(template.id.clone()))),
        )
        .collect()
}

fn target_at(settings: &AppSettings, index: i32) -> Option<AppBranchTarget> {
    usize::try_from(index)
        .ok()
        .and_then(|index| targets(settings).get(index).cloned())
}

fn target_label(settings: &AppSettings, target: &AppBranchTarget) -> String {
    match target {
        AppBranchTarget::Global => "__DEFAULT__".into(),
        AppBranchTarget::Template(id) => settings
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == id.0)
            .map(ia_ui::dictation_polish_label)
            .unwrap_or_else(|| "__MISSING_TEMPLATE__".into()),
    }
}

pub(crate) fn populate(window: &MainWindow, settings: &AppSettings) {
    let labels: Vec<slint::SharedString> = targets(settings)
        .iter()
        .map(|target| target_label(settings, target).into())
        .collect();
    window.set_settings_app_branch_target_labels(sync_model(
        window.get_settings_app_branch_target_labels(),
        labels,
    ));
    let rollback_revisions: HashMap<_, _> = window
        .get_settings_app_branch_rules()
        .iter()
        .map(|row| (row.id.to_string(), row.pattern_rollback_revision))
        .collect();
    let rows: Vec<_> = settings
        .dictation_app_branches
        .iter()
        .map(|rule| AppBranchRow {
            id: rule.id.0.as_str().into(),
            app_pattern: rule.app_pattern.as_str().into(),
            enabled: rule.enabled,
            target_label: target_label(settings, &rule.target).into(),
            pattern_rollback_revision: rollback_revisions
                .get(&rule.id.0)
                .copied()
                .unwrap_or_default(),
        })
        .collect();
    window.set_settings_app_branch_rules(sync_model(window.get_settings_app_branch_rules(), rows));
}

fn restore_rendered_pattern(window: &MainWindow, id: &AppBranchId) {
    let rows = window.get_settings_app_branch_rules();
    for index in 0..rows.row_count() {
        if let Some(mut row) = rows.row_data(index)
            && row.id.as_str() == id.0
        {
            // A local edit replaces Slint's text binding. Reapplying equal
            // model data cannot restore it; only this explicit row reset does.
            row.pattern_rollback_revision = row.pattern_rollback_revision.wrapping_add(1);
            rows.set_row_data(index, row);
            break;
        }
    }
}

fn move_rule(settings: &mut AppSettings, id: &AppBranchId, destination: i32) {
    let Some(destination) = usize::try_from(destination)
        .ok()
        .filter(|index| *index < settings.dictation_app_branches.len())
    else {
        return;
    };
    let Some(source) = settings
        .dictation_app_branches
        .iter()
        .position(|rule| &rule.id == id)
    else {
        return;
    };
    let rule = settings.dictation_app_branches.remove(source);
    settings.dictation_app_branches.insert(destination, rule);
}

struct PatternRequest {
    session: Option<SettingsLoadToken>,
    revision: u64,
    value: String,
}

pub(crate) struct Editor {
    window: slint::Weak<MainWindow>,
    io: Rc<SettingsIoCoordinator>,
    cache: SettingsCache,
    drafts: Rc<SettingsDraftController>,
    patterns: RefCell<HashMap<AppBranchId, PatternRequest>>,
    pattern_rollbacks: RefCell<HashMap<AppBranchId, (SettingsLoadToken, u64)>>,
}

impl Editor {
    fn project_routing(&self, window: &MainWindow, settings: &AppSettings) {
        let provider_available = !window.get_settings_summary_provider_unavailable();
        ia_ui::populate_dictation_polish(window, settings, provider_available);
        let displayed_id = window.get_settings_active_dictation_polish_id();
        self.drafts
            .reapply_polish_prompt(window, displayed_id.as_str());
        let rollbacks = std::mem::take(&mut *self.pattern_rollbacks.borrow_mut());
        for (id, (token, revision)) in rollbacks {
            // An observed request may already be pruned. A newer valid edit
            // explicitly removes this ID's queued rollback before submission.
            let still_requested = self
                .patterns
                .borrow()
                .get(&id)
                .is_none_or(|request| request.revision == revision);
            if self.io.accepts_load(token) && still_requested {
                restore_rendered_pattern(window, &id);
            }
        }
    }

    fn save(self: &Rc<Self>, mutation: impl FnOnce(&mut AppSettings) + Send + 'static) -> u64 {
        self.save_then(mutation, |_, _, _| {})
    }

    fn save_then(
        self: &Rc<Self>,
        mutation: impl FnOnce(&mut AppSettings) + Send + 'static,
        completion: impl FnOnce(&Editor, u64, &souffle_lib::commands::SettingsSaveOutcome) + 'static,
    ) -> u64 {
        let token = self.io.current_load_token();
        let callback_revision = Rc::new(Cell::new(0));
        let submitted_revision = callback_revision.clone();
        let weak = Rc::downgrade(self);
        let revision = self
            .io
            .submit(SettingsSaveLane::General, mutation, move |_, outcome| {
                crate::log_settings_save_outcome("app branch", outcome);
                if let Some(editor) = weak.upgrade()
                    && let Some(token) = token
                    && editor.io.accepts_load(token)
                {
                    completion(&editor, submitted_revision.get(), outcome);
                }
            });
        callback_revision.set(revision);
        if let Some(token) = token {
            let weak = Rc::downgrade(self);
            self.io
                .observe_current_open_snapshot(token, move |settings| {
                    if let Some(editor) = weak.upgrade()
                        && let Some(window) = editor.window.upgrade()
                    {
                        editor.project_routing(&window, &settings);
                    }
                });
        }
        revision
    }

    fn queue_pattern_rollback(&self, id: AppBranchId, revision: u64) {
        if let Some(token) = self.io.current_load_token() {
            self.pattern_rollbacks
                .borrow_mut()
                .insert(id, (token, revision));
        }
    }

    fn pattern_is_current(&self, id: &AppBranchId, pattern: &str) -> bool {
        let session = self.io.current_load_token();
        self.patterns.borrow_mut().retain(|_, request| {
            request.session == session && !self.io.revision_is_observed(request.revision)
        });
        if let Some(request) = self.patterns.borrow().get(id) {
            return request.value == pattern;
        }
        // A reopened session can still have writes from its predecessor in
        // flight. Its observed cache is not a safe deduplication baseline.
        self.io.snapshot_revision_is_current()
            && self.cache.borrow().as_ref().is_some_and(|settings| {
                settings
                    .dictation_app_branches
                    .iter()
                    .any(|rule| &rule.id == id && rule.app_pattern == pattern)
            })
    }
}

/// Wire the real global routing controls alongside the rule editor. Their
/// changes settle all canonical routing controls/rows through the same
/// terminal observation as rule mutations.
pub(crate) fn wire_polish_routing_callbacks(window: &MainWindow, editor: Rc<Editor>) {
    let current = editor.clone();
    window.on_settings_dictation_polish_enabled_changed(move |enabled| {
        current.save(move |settings| settings.dictation_polish_enabled = enabled);
    });
    let current = editor.clone();
    window.on_settings_dictation_polish_prompt_changed(move |text| {
        let active_id = current
            .window
            .upgrade()
            .map(|window| window.get_settings_active_dictation_polish_id().to_string());
        if let Some(active_id) = active_id.filter(|id| !id.is_empty()) {
            current
                .drafts
                .edit_polish_prompt(active_id, text.to_string());
        }
    });
    window.on_settings_dictation_polish_template_changed(move |template_id| {
        let template_id = template_id.to_string();
        if !editor.cache.borrow().as_ref().is_some_and(|settings| {
            ia_ui::contains_dictation_polish_id(&settings.dictation_polish_templates, &template_id)
        }) {
            return;
        }
        editor.save(move |settings| settings.dictation_polish_template_id = template_id);
    });
}

pub(crate) fn register(
    window: &MainWindow,
    io: Rc<SettingsIoCoordinator>,
    cache: SettingsCache,
    drafts: Rc<SettingsDraftController>,
) -> Rc<Editor> {
    let editor = Rc::new(Editor {
        window: window.as_weak(),
        io,
        cache,
        drafts,
        patterns: RefCell::new(HashMap::new()),
        pattern_rollbacks: RefCell::new(HashMap::new()),
    });
    let current = editor.clone();
    window.on_settings_app_branch_added(move |pattern, target_index| {
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() {
            return;
        }
        current.save(move |settings| {
            if let Some(target) = target_at(settings, target_index) {
                settings
                    .dictation_app_branches
                    .push(AppBranchRule::new(pattern, target));
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_deleted(move |id| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| settings.dictation_app_branches.retain(|rule| rule.id != id));
    });
    let current = editor.clone();
    window.on_settings_app_branch_enabled(move |id, enabled| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| {
            if let Some(rule) = settings
                .dictation_app_branches
                .iter_mut()
                .find(|rule| rule.id == id)
            {
                rule.enabled = enabled;
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_pattern_changed(move |id, pattern| {
        let id = AppBranchId(id.to_string());
        let pattern = pattern.trim().to_string();
        // Empty edits are rejected and the canonical row is restored.
        if pattern.is_empty() {
            if let Some(window) = current.window.upgrade() {
                restore_rendered_pattern(&window, &id);
            }
            if !current.io.snapshot_revision_is_current() {
                let revision = current
                    .patterns
                    .borrow()
                    .get(&id)
                    .filter(|request| request.session == current.io.current_load_token())
                    .map(|request| request.revision);
                if let Some(revision) = revision {
                    current.queue_pattern_rollback(id, revision);
                }
            }
            return;
        }
        if current.pattern_is_current(&id, &pattern) {
            return;
        }
        // A new valid edit supersedes this ID's queued rollback, even if an
        // Intermediate response already pruned the preceding request.
        current.pattern_rollbacks.borrow_mut().remove(&id);
        let worker_id = id.clone();
        let worker_pattern = pattern.clone();
        let rollback_id = id.clone();
        let revision = current.save_then(
            move |settings| {
                if let Some(rule) = settings
                    .dictation_app_branches
                    .iter_mut()
                    .find(|rule| rule.id == worker_id)
                {
                    rule.app_pattern = worker_pattern;
                }
            },
            move |editor, revision, outcome| {
                use crate::settings_values::{SettingsCommitStatus, save_outcome_commit_status};
                match save_outcome_commit_status(outcome) {
                    SettingsCommitStatus::Committed => {}
                    SettingsCommitStatus::NotCommitted => {
                        let latest = editor
                            .patterns
                            .borrow()
                            .get(&rollback_id)
                            .is_some_and(|request| request.revision == revision);
                        if latest {
                            if let Some(window) = editor.window.upgrade() {
                                // A failed re-read may never publish a terminal
                                // snapshot. Restore the last rendered truth now.
                                restore_rendered_pattern(&window, &rollback_id);
                            }
                            editor.queue_pattern_rollback(rollback_id, revision);
                        }
                    }
                }
            },
        );
        current.patterns.borrow_mut().insert(
            id,
            PatternRequest {
                session: current.io.current_load_token(),
                revision,
                value: pattern,
            },
        );
    });
    let current = editor.clone();
    window.on_settings_app_branch_target_changed(move |id, index| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| {
            if let Some(target) = target_at(settings, index)
                && let Some(rule) = settings
                    .dictation_app_branches
                    .iter_mut()
                    .find(|rule| rule.id == id)
            {
                rule.target = target;
            }
        });
    });
    let current = editor.clone();
    window.on_settings_app_branch_moved(move |id, destination| {
        let id = AppBranchId(id.to_string());
        current.save(move |settings| move_rule(settings, &id, destination));
    });
    editor
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use slint::platform::{
        EventLoopProxy, Platform, WindowAdapter, software_renderer::MinimalSoftwareWindow,
    };
    use souffle_lib::summary::{AppBranchFallback, AppBranchResolution, resolve_app_branch};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender};
    use std::time::Duration;

    type UiEvent = Box<dyn FnOnce() + Send>;
    thread_local! {
        static EVENTS: (Sender<UiEvent>, Receiver<UiEvent>) = std::sync::mpsc::channel();
        static SOFTWARE_WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    }

    struct TestProxy(Sender<UiEvent>);
    impl EventLoopProxy for TestProxy {
        fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
            Ok(())
        }
        fn invoke_from_event_loop(&self, event: UiEvent) -> Result<(), slint::EventLoopError> {
            self.0
                .send(event)
                .map_err(|_| slint::EventLoopError::EventLoopTerminated)
        }
    }

    struct TestPlatform {
        registering: Cell<bool>,
    }
    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(Default::default());
            SOFTWARE_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
            Ok(window)
        }
        fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
            // Unit fixtures run on independent libtest threads. Keep the
            // process-global proxy unset during platform registration; local
            // futures then obtain the proxy belonging to their own context.
            if self.registering.replace(false) {
                return None;
            }
            Some(Box::new(TestProxy(EVENTS.with(|events| events.0.clone()))))
        }
        // A frozen clock keeps draft debounce timers pending.
        fn duration_since_start(&self) -> Duration {
            Duration::ZERO
        }
    }

    fn test_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform {
            registering: Cell::new(true),
        }));
        crate::settings_kit::new_window().unwrap()
    }

    fn pump() {
        slint::platform::update_timers_and_animations();
        for _ in 0..64 {
            let event = EVENTS.with(|events| events.1.try_recv().ok());
            let Some(event) = event else { break };
            event();
        }
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "app rule operation timed out"
            );
            pump();
            std::thread::yield_now();
        }
    }

    fn settle(io: &SettingsIoCoordinator) {
        let revision_limit = io.current_revision() + 16;
        wait_until(|| {
            assert!(
                io.current_revision() <= revision_limit,
                "app rule observations must be bounded"
            );
            io.drain_one_for_test();
            io.is_idle_for_test()
        });
    }

    fn isolated_editor() -> (
        MainWindow,
        Rc<SettingsIoCoordinator>,
        std::sync::Arc<souffle_lib::db::Database>,
        tempfile::TempDir,
        Rc<Editor>,
    ) {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_autostart = Arc::clone(&db);
        let save = Arc::clone(&db);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&load_autostart),
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        let values = crate::settings_values::SettingsValueController::new(
            &window,
            cache.clone(),
            io.clone(),
            Rc::new(std::cell::RefCell::new(Vec::new())),
            Rc::new(std::cell::RefCell::new(None)),
        );
        crate::settings_values::wire(&window, values);
        let editor = register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        wire_polish_routing_callbacks(&window, editor.clone());
        populate(&window, &initial);
        (window, io, db, dir, editor)
    }

    struct BlockedEditor {
        window: MainWindow,
        io: Rc<SettingsIoCoordinator>,
        db: Arc<souffle_lib::db::Database>,
        editor: Rc<Editor>,
        _dir: tempfile::TempDir,
        release: Sender<()>,
    }

    impl BlockedEditor {
        fn new() -> Self {
            use souffle_lib::commands::SettingsSaveOutcome;
            use std::sync::{
                Mutex,
                atomic::{AtomicUsize, Ordering},
            };
            let dir = tempfile::tempdir().unwrap();
            let db =
                Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
            let initial = AppSettings::default();
            initial.save(&db).unwrap();
            let window = test_window();
            let cache = SettingsCache::with_observed(&window, initial.clone());
            let load = db.clone();
            let effective = db.clone();
            let save = db.clone();
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let release_rx = Mutex::new(release_rx);
            let saves = AtomicUsize::new(0);
            let io = SettingsIoCoordinator::with_functions(
                cache.clone(),
                move || AppSettings::load(&load),
                move || AppSettings::load(&effective),
                move |settings| {
                    if saves.fetch_add(1, Ordering::SeqCst) == 0 {
                        started_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(2))
                            .unwrap();
                    }
                    settings.save(&save).unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(AppSettings::load(&save).unwrap()),
                        result: Ok(()),
                    }
                },
                |_| panic!("app rules must not use the autostart lane"),
            );
            io.begin_open();
            let editor = register(
                &window,
                io.clone(),
                cache,
                SettingsDraftController::new(&window, io.clone()),
            );
            wire_polish_routing_callbacks(&window, editor.clone());
            populate(&window, &initial);
            let rule = window.get_settings_app_branch_rules().row_data(0).unwrap();
            window.invoke_settings_app_branch_enabled(rule.id, true);
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            Self {
                window,
                io,
                db,
                editor,
                _dir: dir,
                release,
            }
        }

        fn finish(&self) {
            self.release.send(()).unwrap();
            settle(&self.io);
            assert!(
                self.window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
            assert!(AppSettings::load(&self.db).unwrap().dictation_app_branches[0].enabled);
        }
    }

    fn assert_polish_controls_match(window: &MainWindow, canonical: &AppSettings) {
        let active = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == canonical.dictation_polish_template_id)
            .unwrap();
        assert_eq!(
            window.get_settings_active_dictation_polish_id().as_str(),
            active.id
        );
        assert_eq!(
            window
                .get_settings_active_dictation_polish_prompt()
                .as_str(),
            active.prompt
        );
        assert_eq!(
            window.get_settings_dictation_polish_enabled(),
            canonical.dictation_polish_enabled
        );
        assert_eq!(
            window.get_settings_app_branch_rules().row_count(),
            canonical.dictation_app_branches.len()
        );
        for (index, rule) in canonical.dictation_app_branches.iter().enumerate() {
            let row = window
                .get_settings_app_branch_rules()
                .row_data(index)
                .unwrap();
            assert_eq!(row.id.as_str(), rule.id.0);
            assert_eq!(row.enabled, rule.enabled);
            assert_eq!(row.app_pattern.as_str(), rule.app_pattern);
        }
    }

    #[test]
    fn global_template_then_rule_publishes_canonical_polish_controls() {
        let test = BlockedEditor::new();
        ia_ui::populate_dictation_polish(&test.window, &AppSettings::load(&test.db).unwrap(), true);
        assert_eq!(
            test.window.get_settings_active_dictation_polish_id(),
            "clean"
        );
        test.window
            .invoke_settings_dictation_polish_template_changed(
                souffle_lib::summary::TEMPLATE_EMAIL.into(),
            );
        let row = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window.invoke_settings_app_branch_enabled(row.id, true);
        test.finish();
        let canonical = AppSettings::load(&test.db).unwrap();
        assert_eq!(
            canonical.dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_polish_controls_match(&test.window, &canonical);
    }

    #[test]
    fn polish_enabled_then_rule_publishes_canonical_polish_controls() {
        let test = BlockedEditor::new();
        ia_ui::populate_dictation_polish(&test.window, &AppSettings::load(&test.db).unwrap(), true);
        assert!(test.window.get_settings_dictation_polish_enabled());
        test.window
            .invoke_settings_dictation_polish_enabled_changed(false);
        let row = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window.invoke_settings_app_branch_enabled(row.id, true);
        test.finish();
        let canonical = AppSettings::load(&test.db).unwrap();
        assert!(!canonical.dictation_polish_enabled);
        assert_polish_controls_match(&test.window, &canonical);
    }

    #[test]
    fn coalesced_routing_preserves_shared_prompt_drafts_and_routes_visible_edits() {
        let (window, io, db, _dir, editor) = isolated_editor();
        let initial = AppSettings::load(&db).unwrap();
        ia_ui::populate_dictation_polish(&window, &initial, true);
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        settle(&io);
        window.set_settings_active_dictation_polish_prompt("Email draft".into());
        window.invoke_settings_dictation_polish_prompt_changed("Email draft".into());
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_CLEAN.into(),
        );
        settle(&io);
        assert_polish_controls_match(&window, &AppSettings::load(&db).unwrap());

        // No mock-clock advance: the shared prompt is still a pending draft
        // when the rule observer supersedes the global-template observer.
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        let row = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(row.id.clone(), true);
        settle(&io);
        assert_eq!(
            window.get_settings_active_dictation_polish_id(),
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_eq!(
            window.get_settings_active_dictation_polish_prompt(),
            "Email draft"
        );

        window.set_settings_active_dictation_polish_prompt("New email draft".into());
        window.invoke_settings_dictation_polish_prompt_changed("New email draft".into());
        editor.drafts.flush_explicit(|result| {
            assert_eq!(result, crate::settings_drafts::DraftFlushResult::Committed)
        });
        settle(&io);
        window.invoke_settings_app_branch_enabled(row.id, false);
        settle(&io);
        let canonical = AppSettings::load(&db).unwrap();
        let email = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap();
        assert_eq!(email.prompt, "New email draft");
        let cleanup = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
            .unwrap();
        assert_eq!(
            cleanup.prompt,
            initial
                .dictation_polish_templates
                .iter()
                .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
                .unwrap()
                .prompt
        );
        assert_polish_controls_match(&window, &canonical);
    }

    #[test]
    fn prompt_edit_during_intermediate_global_response_targets_displayed_template() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let original_email_prompt = initial
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap()
            .prompt
            .clone();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = db.clone();
        let effective = db.clone();
        let save = db.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let blocked = AtomicBool::new(false);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                if settings.dictation_app_branches[0].enabled
                    && !blocked.swap(true, Ordering::SeqCst)
                {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("unexpected autostart save"),
        );
        io.begin_open();
        let editor = register(
            &window,
            io.clone(),
            cache.clone(),
            SettingsDraftController::new(&window, io.clone()),
        );
        wire_polish_routing_callbacks(&window, editor.clone());
        ia_ui::populate_dictation_polish(&window, &initial, true);
        window.invoke_settings_dictation_polish_template_changed(
            souffle_lib::summary::TEMPLATE_EMAIL.into(),
        );
        let rule = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(rule.id, true);
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // The global save response is Intermediate because the rule save
        // remains blocked. Durable/cache ID advances, but its prompt is not
        // the one the user is editing until terminal publication happens.
        io.drain_one_for_test();
        assert_eq!(
            cache.known_snapshot().unwrap().dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        assert_eq!(window.get_settings_active_dictation_polish_id(), "clean");
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        window.set_settings_active_dictation_polish_prompt("Edited displayed Cleanup".into());
        window.invoke_settings_dictation_polish_prompt_changed("Edited displayed Cleanup".into());
        editor.drafts.flush_explicit(|result| {
            assert_eq!(result, crate::settings_drafts::DraftFlushResult::Committed)
        });
        release_tx.send(()).unwrap();
        settle(&io);
        let canonical = AppSettings::load(&db).unwrap();
        assert!(canonical.dictation_app_branches[0].enabled);
        assert_eq!(
            canonical.dictation_polish_template_id,
            souffle_lib::summary::TEMPLATE_EMAIL
        );
        let cleanup = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_CLEAN)
            .unwrap();
        let email = canonical
            .dictation_polish_templates
            .iter()
            .find(|template| template.id == souffle_lib::summary::TEMPLATE_EMAIL)
            .unwrap();
        assert_eq!(cleanup.prompt, "Edited displayed Cleanup");
        assert_eq!(email.prompt, original_email_prompt);
        assert_polish_controls_match(&window, &canonical);
    }

    #[test]
    fn reopened_pattern_editor_cannot_deduplicate_against_unsettled_old_session() {
        let test = BlockedEditor::new();
        let original = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        test.io.close();
        test.io.begin_open();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id, original.app_pattern.clone());
        test.finish();
        assert_eq!(
            AppSettings::load(&test.db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
        assert_eq!(
            test.window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
    }

    #[test]
    fn pattern_a_b_a_while_save_is_blocked_preserves_latest_requested_value() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let effective = Arc::clone(&db);
        let save = Arc::clone(&db);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                if settings.dictation_app_branches[0].app_pattern == "Messages" {
                    started_tx.send(()).unwrap();
                    release_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                }
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        window.invoke_settings_app_branch_pattern_changed(
            original.id.clone(),
            original.app_pattern.clone(),
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !io.is_idle_for_test() {
            assert!(io.current_revision() <= 16 && Instant::now() < deadline);
            io.drain_one_for_test();
            std::thread::yield_now();
        }
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
        // Re-publication/focus-lost echoes of the canonical value must not
        // create another write, even after a rapid round trip.
        let settled_revision = io.current_revision();
        window.invoke_settings_app_branch_pattern_changed(original.id, original.app_pattern);
        assert_eq!(io.current_revision(), settled_revision);
    }

    #[test]
    fn delayed_open_and_immediate_rule_edit_publish_one_terminal_snapshot() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::cell::Cell;
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_effective = Arc::clone(&db);
        let save = Arc::clone(&db);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || {
                started_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                AppSettings::load(&load_effective)
            },
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        let token = io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        let opened = Rc::new(Cell::new(0));
        let open_count = opened.clone();
        let weak = window.as_weak();
        io.load_effective_snapshot(token, move |settings| {
            open_count.set(open_count.get() + 1);
            if let Some(window) = weak.upgrade() {
                populate(&window, &settings);
            }
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Invoke the actual editor callback while Open remains blocked;
        // neither audience may restart the other.
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(original.id, true);
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while !io.is_idle_for_test() {
            assert!(
                io.current_revision() <= 32 && Instant::now() < deadline,
                "Open and rule observations must settle: revision={}, open publications={}",
                io.current_revision(),
                opened.get(),
            );
            pump();
            io.drain_one_for_test();
            std::thread::yield_now();
        }
        assert_eq!(opened.get(), 1);
        assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        // Holding the coordinator must not hold a strong Window handle.
        let weak = window.as_weak();
        drop(window);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn rule_mutations_publish_terminal_rule_rows() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        window.invoke_settings_app_branch_enabled(original.id.clone(), true);
        settle(&io);
        assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled,
            "terminal snapshot must refresh the rule switch"
        );

        window.invoke_settings_app_branch_enabled(original.id.clone(), false);
        settle(&io);
        assert!(
            !window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        window.invoke_settings_app_branch_target_changed(original.id.clone(), 2);
        settle(&io);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .target_label,
            "__PROFESSIONAL_EMAIL__"
        );
        let old_count = window.get_settings_app_branch_rules().row_count();
        window.invoke_settings_app_branch_added("Immediate addition".into(), 0);
        settle(&io);
        assert_eq!(
            window.get_settings_app_branch_rules().row_count(),
            old_count + 1
        );
    }

    #[test]
    fn unrelated_settings_save_cannot_hide_rule_publication() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let paste_delay = AppSettings::load(&db).unwrap().paste_delay_ms as i32;
        window.invoke_settings_app_branch_enabled(original.id.clone(), true);
        window.invoke_settings_paste_delay_changed(600);
        settle(&io);
        let durable = AppSettings::load(&db).unwrap();
        assert!(durable.dictation_app_branches[0].enabled);
        assert_eq!(durable.paste_delay_ms, 600);
        assert!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .enabled
        );
        assert_eq!(window.get_settings_paste_delay_ms(), 600);
        window.invoke_settings_app_branch_target_changed(original.id.clone(), 2);
        window.invoke_settings_paste_delay_changed(paste_delay);
        settle(&io);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .target_label,
            "__PROFESSIONAL_EMAIL__"
        );
        assert_eq!(
            AppSettings::load(&db).unwrap().paste_delay_ms as i32,
            paste_delay
        );
    }

    #[test]
    fn rule_snapshots_do_not_publish_after_close_or_into_reopened_session() {
        for reopen in [false, true] {
            let (window, io, db, _dir, _editor) = isolated_editor();
            let initial = AppSettings::load(&db).unwrap();
            let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
            window.invoke_settings_app_branch_enabled(original.id.clone(), true);
            io.close();
            if reopen {
                io.begin_open();
            }
            populate(&window, &initial);
            settle(&io);
            assert!(AppSettings::load(&db).unwrap().dictation_app_branches[0].enabled);
            assert!(
                !window
                    .get_settings_app_branch_rules()
                    .row_data(0)
                    .unwrap()
                    .enabled
            );
        }
    }

    #[test]
    fn rule_controls_persist_to_isolated_database_and_drive_the_pipeline_resolver() {
        use souffle_lib::commands::SettingsSaveOutcome;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rules.db");
        let db = Arc::new(souffle_lib::db::Database::open(&db_path).unwrap());
        let mut initial = AppSettings::default();
        initial.dictation_app_branches.clear();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = Arc::clone(&db);
        let load_autostart = Arc::clone(&db);
        let save = Arc::clone(&db);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&load_autostart),
            move |settings| {
                settings.save(&save).unwrap();
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result: Ok(()),
                }
            },
            |_| panic!("app branches must not use the autostart lane"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        window.invoke_settings_app_branch_added("Mail".into(), 0);
        window.invoke_settings_app_branch_added("mail".into(), 2);
        settle(&io);
        let first = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let second = window.get_settings_app_branch_rules().row_data(1).unwrap();
        assert!(!first.enabled && !second.enabled);
        window.invoke_settings_app_branch_enabled(first.id.clone(), true);
        window.invoke_settings_app_branch_enabled(second.id.clone(), true);
        window.invoke_settings_app_branch_pattern_changed(second.id.clone(), "Apple Mail".into());
        window.invoke_settings_app_branch_moved(second.id.clone(), 0);
        settle(&io);
        let durable = AppSettings::load(&db).unwrap();
        match resolve_app_branch(&durable, Some("Apple Mail")) {
            AppBranchResolution::Rule { rule, template } => {
                assert_eq!(rule.app_pattern, "Apple Mail");
                assert_eq!(template.id, "email");
            }
            AppBranchResolution::Global { reason, .. } => {
                panic!("the moved rule must win, got {reason:?}")
            }
        }
        window.invoke_settings_app_branch_target_changed(second.id.clone(), 0);
        settle(&io);
        let durable = AppSettings::load(&db).unwrap();
        assert_eq!(
            resolve_app_branch(&durable, Some("Apple Mail"))
                .template()
                .unwrap()
                .id,
            "clean"
        );
        window.invoke_settings_app_branch_pattern_changed(first.id.clone(), "   ".into());
        window.invoke_settings_app_branch_added("   ".into(), 0);
        window.invoke_settings_app_branch_deleted(first.id.clone());
        settle(&io);
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches.len(),
            1
        );
        drop(io);
        drop(db);
        let restarted = souffle_lib::db::Database::open(&db_path).unwrap();
        let restored = AppSettings::load(&restarted).unwrap();
        assert_eq!(restored.dictation_app_branches[0].id.0, second.id.as_str());
        assert!(restored.dictation_app_branches[0].enabled);
        assert_eq!(
            restored.dictation_app_branches[0].target,
            AppBranchTarget::Global
        );
        assert!(matches!(
            resolve_app_branch(&restored, None),
            AppBranchResolution::Global {
                reason: AppBranchFallback::AppUnavailable,
                ..
            }
        ));
    }

    #[test]
    fn canonical_repopulation_preserves_models_and_does_not_echo_writes() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        let rules = window.get_settings_app_branch_rules();
        let labels = window.get_settings_app_branch_target_labels();
        let canonical = AppSettings::load(&db).unwrap();
        let revision = io.current_revision();
        populate(&window, &canonical);
        assert!(std::ptr::eq(
            rules.as_any(),
            window.get_settings_app_branch_rules().as_any()
        ));
        assert!(std::ptr::eq(
            labels.as_any(),
            window.get_settings_app_branch_target_labels().as_any()
        ));
        let row = rules.row_data(0).unwrap();
        window.invoke_settings_app_branch_pattern_changed(row.id, row.app_pattern);
        assert_eq!(io.current_revision(), revision);
    }

    fn pattern_inputs(window: &MainWindow) -> Vec<i_slint_backend_testing::ElementHandle> {
        use i_slint_backend_testing::{AccessibleRole, ElementRoot};
        window
            .root_element()
            .query_descendants()
            .match_predicate(|element| {
                element.accessible_role() == Some(AccessibleRole::TextInput)
                    && element.accessible_label().is_some_and(|label| {
                        label == "Rule pattern" || label == "Motif de la règle"
                    })
            })
            .find_all()
    }

    fn render_rules(window: &MainWindow) {
        crate::typography::initialize(window).unwrap();
        // The rules section is mounted only while polish is on.
        window.set_settings_dictation_polish_enabled(true);
        window
            .window()
            .set_size(slint::PhysicalSize::new(1280, 1400));
        let adapter = SOFTWARE_WINDOW.with(|slot| slot.borrow().clone()).unwrap();
        let mut pixels =
            vec![slint::platform::software_renderer::Rgb565Pixel::default(); 1280 * 1400];
        for _ in 0..3 {
            pump();
            adapter.request_redraw();
            adapter.draw_if_needed(|renderer| {
                renderer.render(&mut pixels, 1280);
            });
        }
    }

    fn click_pattern_input(window: &MainWindow, input: &i_slint_backend_testing::ElementHandle) {
        let position = slint::LogicalPosition::new(
            input.absolute_position().x + input.size().width / 2.0,
            input.absolute_position().y + input.size().height / 2.0,
        );
        let button = slint::platform::PointerEventButton::Left;
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        pump();
    }

    fn pattern_key(window: &MainWindow, text: slint::SharedString) {
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        pump();
    }

    fn clear_pattern_input(window: &MainWindow) {
        for _ in 0..12 {
            pattern_key(window, slint::platform::Key::RightArrow.into());
        }
        for _ in 0..12 {
            pattern_key(window, slint::platform::Key::Backspace.into());
        }
    }

    #[test]
    fn pending_pattern_rollback_survives_intermediate_observation_and_other_rule_edit() {
        let test = BlockedEditor::new();
        test.window.set_settings_open(true);
        test.window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&test.window);
        let inputs = pattern_inputs(&test.window);
        assert!(inputs.len() >= 2);
        let original = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        click_pattern_input(&test.window, &inputs[0]);
        clear_pattern_input(&test.window);
        pattern_key(&test.window, "Messages".into());
        pattern_key(&test.window, slint::platform::Key::Return.into());
        let requested_revision = test.io.current_revision();
        clear_pattern_input(&test.window);
        click_pattern_input(&test.window, &inputs[1]);
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        test.release.send(()).unwrap();
        wait_until(|| {
            test.io.drain_one_for_test();
            test.io.revision_is_observed(requested_revision)
        });
        assert!(!test.io.snapshot_revision_is_current());
        assert_eq!(
            test.editor
                .cache
                .known_snapshot()
                .unwrap()
                .dictation_app_branches[0]
                .app_pattern,
            "Messages"
        );
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        clear_pattern_input(&test.window);
        pattern_key(&test.window, "Updated Mail".into());
        pattern_key(&test.window, slint::platform::Key::Return.into());
        settle(&test.io);
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), "Messages");
        assert_eq!(inputs[1].accessible_value().unwrap(), "Updated Mail");
        let canonical = AppSettings::load(&test.db).unwrap();
        assert_eq!(canonical.dictation_app_branches[0].app_pattern, "Messages");
        assert_eq!(
            canonical.dictation_app_branches[1].app_pattern,
            "Updated Mail"
        );
    }

    #[test]
    fn moved_edited_pattern_field_tracks_rendered_rule_identity() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let next = window.get_settings_app_branch_rules().row_data(1).unwrap();
        click_pattern_input(&window, &inputs[0]);
        clear_pattern_input(&window);
        pattern_key(&window, "Messages".into());
        pattern_key(&window, slint::platform::Key::Return.into());
        settle(&io);
        pump();
        if inputs.len() >= 3 {
            inputs[2].set_accessible_value("Unmoved unsaved draft");
        }
        window.invoke_settings_app_branch_moved(original.id.clone(), 1);
        settle(&io);
        render_rules(&window);
        let moved_inputs = pattern_inputs(&window);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .id,
            next.id
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(1)
                .unwrap()
                .id,
            original.id
        );
        assert_eq!(
            moved_inputs[0].accessible_value().unwrap(),
            next.app_pattern
        );
        assert_eq!(moved_inputs[1].accessible_value().unwrap(), "Messages");
        if inputs.len() >= 3 {
            assert_eq!(
                moved_inputs[2].accessible_value().unwrap(),
                "Unmoved unsaved draft"
            );
        }
        click_pattern_input(&window, &moved_inputs[1]);
        settle(&io);
        pump();
        let canonical = AppSettings::load(&db).unwrap();
        assert_eq!(
            canonical.dictation_app_branches[0].app_pattern,
            next.app_pattern.as_str()
        );
        assert_eq!(canonical.dictation_app_branches[1].app_pattern, "Messages");
        if inputs.len() >= 3 {
            assert_eq!(
                canonical.dictation_app_branches[2].app_pattern, "Unmoved unsaved draft",
                "blur saves the draft to the rule that owns the field"
            );
        }
    }

    #[test]
    fn deletion_after_pattern_edit_resets_recycled_field_without_wrong_id_write() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let next = window.get_settings_app_branch_rules().row_data(1).unwrap();
        click_pattern_input(&window, &inputs[0]);
        clear_pattern_input(&window);
        pattern_key(&window, "Messages".into());
        pattern_key(&window, slint::platform::Key::Return.into());
        settle(&io);
        window.invoke_settings_app_branch_deleted(original.id);
        settle(&io);
        render_rules(&window);
        let remaining_inputs = pattern_inputs(&window);
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .id,
            next.id
        );
        assert_eq!(
            remaining_inputs[0].accessible_value().unwrap(),
            next.app_pattern
        );
        let revision = io.current_revision();
        click_pattern_input(&window, &remaining_inputs[1]);
        assert_eq!(
            io.current_revision(),
            revision,
            "recycled field blur must not corrupt its new ID"
        );
        let canonical = AppSettings::load(&db).unwrap();
        assert_eq!(canonical.dictation_app_branches[0].id.0, next.id.as_str());
        assert_eq!(
            canonical.dictation_app_branches[0].app_pattern,
            next.app_pattern.as_str()
        );
    }

    #[test]
    fn unavailable_rejected_pattern_restores_rendered_row_and_preserves_retry() {
        use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let read_failed = Arc::new(AtomicBool::new(false));
        let reads = read_failed.clone();
        let write_failure = read_failed.clone();
        let writes = Arc::new(AtomicUsize::new(0));
        let counted_writes = writes.clone();
        let load = db.clone();
        let effective = db.clone();
        let save = db.clone();
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || {
                if reads.load(Ordering::SeqCst) {
                    Err("injected unavailable barrier".into())
                } else {
                    AppSettings::load(&load)
                }
            },
            move || AppSettings::load(&effective),
            move |settings| {
                if counted_writes.fetch_add(1, Ordering::SeqCst) == 0 {
                    write_failure.store(true, Ordering::SeqCst);
                    SettingsSaveOutcome::Unavailable {
                        result: Err(SettingsSaveError::NotCommitted {
                            message: "injected rejected pattern".into(),
                        }),
                        read_error: "injected unavailable snapshot".into(),
                    }
                } else {
                    settings.save(&save).unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(AppSettings::load(&save).unwrap()),
                        result: Ok(()),
                    }
                }
            },
            |_| panic!("unexpected autostart save"),
        );
        io.begin_open();
        let _editor = register(
            &window,
            io.clone(),
            cache.clone(),
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2);
        click_pattern_input(&window, &inputs[0]);
        clear_pattern_input(&window);
        pattern_key(&window, "Messages".into());
        pattern_key(&window, slint::platform::Key::Return.into());
        inputs[1].set_accessible_value("Another unsaved draft");
        settle(&io);
        pump();
        assert!(cache.known_snapshot().is_none());
        assert!(window.get_settings_save_error_unavailable());
        assert_eq!(
            inputs[0].accessible_value().unwrap(),
            initial.dictation_app_branches[0].app_pattern
        );
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Another unsaved draft"
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            initial.dictation_app_branches[0].app_pattern
        );
        read_failed.store(false, Ordering::SeqCst);
        click_pattern_input(&window, &inputs[0]);
        settle(&io);
        render_rules(&window);
        click_pattern_input(&window, &inputs[0]);
        clear_pattern_input(&window);
        pattern_key(&window, "Messages".into());
        pattern_key(&window, slint::platform::Key::Return.into());
        settle(&io);
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), "Messages");
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Another unsaved draft"
        );
        assert_eq!(
            writes.load(Ordering::SeqCst),
            3,
            "leaving the draft field saves it once, then the retry saves the edit"
        );
        assert!(!window.get_settings_save_error_unavailable());
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            "Messages"
        );
    }

    #[test]
    fn empty_pattern_keyboard_blur_restores_canonical_field() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let revision = io.current_revision();
        click_pattern_input(&window, &inputs[0]);
        for _ in 0..12 {
            pattern_key(&window, slint::platform::Key::RightArrow.into());
        }
        for _ in 0..12 {
            pattern_key(&window, slint::platform::Key::Backspace.into());
        }
        assert_eq!(inputs[0].accessible_value().unwrap(), "");
        click_pattern_input(&window, &inputs[1]);
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        assert_eq!(io.current_revision(), revision);
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
    }

    #[test]
    fn empty_pattern_edit_restores_actual_field_without_touching_other_draft() {
        let (window, io, db, _dir, _editor) = isolated_editor();
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2, "the actual rule fields must be mounted");
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        let revision = io.current_revision();
        let rules = window.get_settings_app_branch_rules();
        let labels = window.get_settings_app_branch_target_labels();
        inputs[0].set_accessible_value("");
        assert_eq!(inputs[0].accessible_value().unwrap(), "");
        inputs[1].set_accessible_value("Other uncommitted draft");
        pump();
        window.invoke_settings_app_branch_pattern_changed(original.id.clone(), "".into());
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Other uncommitted draft"
        );
        assert_eq!(
            io.current_revision(),
            revision,
            "rollback must not write settings"
        );
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            original.app_pattern.as_str()
        );
        assert!(std::ptr::eq(
            rules.as_any(),
            window.get_settings_app_branch_rules().as_any()
        ));
        assert!(std::ptr::eq(
            labels.as_any(),
            window.get_settings_app_branch_target_labels().as_any()
        ));
        assert_eq!(rules.row_data(1).unwrap().pattern_rollback_revision, 0);
        ia_ui::populate_dictation_polish(&window, &AppSettings::load(&db).unwrap(), true);
        pump();
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Other uncommitted draft"
        );
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: "X".into() });
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: "X".into() });
        assert!(
            inputs[1].accessible_value().unwrap().contains('X'),
            "the other field must retain keyboard focus"
        );
        window.invoke_settings_app_branch_pattern_changed(original.id, original.app_pattern);
        assert_eq!(
            io.current_revision(),
            revision,
            "canonical focus-lost echoes must not write"
        );
    }

    #[test]
    fn empty_pattern_during_pending_save_restores_rendered_then_terminal_truth() {
        let test = BlockedEditor::new();
        test.window.set_settings_open(true);
        test.window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&test.window);
        let inputs = pattern_inputs(&test.window);
        assert!(inputs.len() >= 2);
        let original = test
            .window
            .get_settings_app_branch_rules()
            .row_data(0)
            .unwrap();
        inputs[1].set_accessible_value("Other pending draft");
        inputs[0].set_accessible_value("Messages");
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id.clone(), "Messages".into());
        inputs[0].set_accessible_value("");
        let revision = test.io.current_revision();
        test.window
            .invoke_settings_app_branch_pattern_changed(original.id, "".into());
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        assert_eq!(test.io.current_revision(), revision);
        test.finish();
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), "Messages");
        assert_eq!(inputs[1].accessible_value().unwrap(), "Other pending draft");
        assert_eq!(
            AppSettings::load(&test.db).unwrap().dictation_app_branches[0].app_pattern,
            "Messages"
        );
    }

    #[test]
    fn rejected_pattern_save_restores_canonical_value_and_allows_retry() {
        use souffle_lib::commands::{SettingsSaveError, SettingsSaveOutcome};
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(souffle_lib::db::Database::open(&dir.path().join("rules.db")).unwrap());
        let initial = AppSettings::default();
        initial.save(&db).unwrap();
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let load = db.clone();
        let effective = db.clone();
        let save = db.clone();
        let reject = AtomicBool::new(true);
        let io = SettingsIoCoordinator::with_functions(
            cache.clone(),
            move || AppSettings::load(&load),
            move || AppSettings::load(&effective),
            move |settings| {
                let result = if reject.swap(false, Ordering::SeqCst) {
                    Err(SettingsSaveError::NotCommitted {
                        message: "injected rollback".into(),
                    })
                } else {
                    settings.save(&save).unwrap();
                    Ok(())
                };
                SettingsSaveOutcome::Observed {
                    settings: Box::new(AppSettings::load(&save).unwrap()),
                    result,
                }
            },
            |_| panic!("unexpected autostart save"),
        );
        io.begin_open();
        register(
            &window,
            io.clone(),
            cache,
            SettingsDraftController::new(&window, io.clone()),
        );
        populate(&window, &initial);
        window.set_settings_open(true);
        window.set_settings_tab(crate::SettingsTab::Ai);
        render_rules(&window);
        let inputs = pattern_inputs(&window);
        assert!(inputs.len() >= 2);
        let original = window.get_settings_app_branch_rules().row_data(0).unwrap();
        inputs[0].set_accessible_value("Messages");
        inputs[1].set_accessible_value("Another unsaved draft");
        settle(&io);
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), original.app_pattern);
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Another unsaved draft"
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            original.app_pattern
        );
        let revision = io.current_revision();
        window.invoke_settings_app_branch_pattern_changed(
            original.id.clone(),
            original.app_pattern.clone(),
        );
        assert_eq!(io.current_revision(), revision);
        inputs[0].set_accessible_value("Messages");
        window.invoke_settings_app_branch_pattern_changed(original.id, "Messages".into());
        settle(&io);
        pump();
        assert_eq!(inputs[0].accessible_value().unwrap(), "Messages");
        assert_eq!(
            inputs[1].accessible_value().unwrap(),
            "Another unsaved draft"
        );
        assert_eq!(
            AppSettings::load(&db).unwrap().dictation_app_branches[0].app_pattern,
            "Messages"
        );
        assert_eq!(
            window
                .get_settings_app_branch_rules()
                .row_data(0)
                .unwrap()
                .app_pattern,
            "Messages"
        );
    }

    #[test]
    fn target_catalogue_tracks_shared_templates_and_global() {
        let mut settings = AppSettings::default();
        settings
            .dictation_polish_templates
            .push(souffle_lib::settings::DictationPolishTemplate {
                id: "user-template".into(),
                label: "My shared template".into(),
                prompt: "Custom instruction".into(),
            });
        assert_eq!(target_at(&settings, 0), Some(AppBranchTarget::Global));
        assert_eq!(
            targets(&settings).len(),
            settings.dictation_polish_templates.len() + 1
        );
        assert_eq!(target_at(&settings, -1), None);
        assert_eq!(target_at(&settings, 100), None);
        for (index, template) in settings.dictation_polish_templates.iter().enumerate() {
            assert_eq!(
                target_at(&settings, (index + 1) as i32),
                Some(AppBranchTarget::Template(PolishTemplateId(
                    template.id.clone()
                )))
            );
        }
        assert_eq!(
            target_label(
                &settings,
                &AppBranchTarget::Template(PolishTemplateId("user-template".into()))
            ),
            "My shared template"
        );
    }

    #[test]
    fn editor_reorders_by_stable_id_and_preserves_other_fields() {
        let mut settings = AppSettings::default();
        let first = settings.dictation_app_branches[0].clone();
        move_rule(&mut settings, &first.id, 2);
        assert_eq!(settings.dictation_app_branches[2], first);
        let before = settings.dictation_app_branches.clone();
        move_rule(&mut settings, &first.id, -1);
        move_rule(&mut settings, &AppBranchId("missing".into()), 0);
        assert_eq!(settings.dictation_app_branches, before);
    }
}
