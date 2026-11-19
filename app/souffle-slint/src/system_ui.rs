//! System settings page: the live log tail, the diagnostics copy, the data
//! export, the MCP server check, the model deletion and the update check. The
//! page shows the projection and reports intentions; every effect (log file,
//! clipboard, folder dialog, export, sidecar, model files, network) stays here
//! behind `SystemBackend`, so a test or a fixture replaces it without touching
//! the user's data.
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use slint_kit::{DirectoryHandle, DirectoryRequest, FileLocation, HostError, SelectionOutcome};
use souffle_lib::state::AppState;
use souffle_lib::update_check::UpdateCheckResult;

use crate::{ExportStatus, MainWindow, McpStatus, UpdateCheckStatus};

pub(crate) type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;
pub(crate) type FolderChoice = LocalFuture<SelectionOutcome<DirectoryHandle>>;

pub(crate) const LOG_TAIL_LINES: u32 = 80;
pub(crate) const LOG_POLL: Duration = Duration::from_millis(2000);
/// Lines kept for the reader. The log file's tail only ever shows the last
/// `LOG_TAIL_LINES`; keeping what scrolled past lets a refresh append without
/// moving the reader's place. Beyond this the oldest lines are dropped.
pub(crate) const MAX_LOG_LINES: usize = 2000;
const ARCHIVE_EXPORT_POLL: Duration = Duration::from_millis(150);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LogUpdate {
    Unchanged,
    /// The first `n` lines of the new tail are already shown; append the rest.
    Append(usize),
    Replace,
}

pub(crate) fn split_tail(tail: &str) -> Vec<SharedString> {
    tail.lines().map(SharedString::from).collect()
}

/// Compares the lines shown with a fresh tail by identity of content: the
/// longest suffix of what is shown that starts the new tail is the overlap.
pub(crate) fn merge_tail(shown: &[SharedString], fresh: &[SharedString]) -> LogUpdate {
    if fresh.is_empty() {
        return if shown.is_empty() {
            LogUpdate::Unchanged
        } else {
            LogUpdate::Replace
        };
    }
    let longest = shown.len().min(fresh.len());
    for overlap in (1..=longest).rev() {
        let start = shown.len() - overlap;
        if shown[start] == fresh[0] && shown[start..] == fresh[..overlap] {
            return if overlap == fresh.len() {
                LogUpdate::Unchanged
            } else {
                LogUpdate::Append(overlap)
            };
        }
    }
    LogUpdate::Replace
}

/// The whole tail the page lists. The model is updated in place so the rows
/// already built, their selection and the scroll position survive a refresh.
pub(crate) struct LogTail {
    shown: RefCell<Vec<SharedString>>,
    model: Rc<VecModel<SharedString>>,
}

impl LogTail {
    pub(crate) fn new() -> Self {
        Self {
            shown: RefCell::new(Vec::new()),
            model: Rc::new(VecModel::default()),
        }
    }

    pub(crate) fn model(&self) -> ModelRc<SharedString> {
        self.model.clone().into()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        use slint::Model;
        self.model.row_count()
    }

    pub(crate) fn apply(&self, tail: &str) {
        let fresh = split_tail(tail);
        let mut shown = self.shown.borrow_mut();
        match merge_tail(&shown, &fresh) {
            LogUpdate::Unchanged => {}
            LogUpdate::Append(overlap) => {
                for line in &fresh[overlap..] {
                    shown.push(line.clone());
                    self.model.push(line.clone());
                }
                let excess = shown.len().saturating_sub(MAX_LOG_LINES);
                for _ in 0..excess {
                    shown.remove(0);
                    self.model.remove(0);
                }
            }
            LogUpdate::Replace => {
                *shown = fresh.clone();
                self.model.set_vec(fresh);
            }
        }
    }
}

/// The system and application effects the page depends on. Tests and the
/// fixture replace them.
pub(crate) struct SystemBackend {
    pub(crate) fetch_log_tail: Rc<dyn Fn() -> LocalFuture<Result<String, String>>>,
    pub(crate) diagnostics_text: Rc<dyn Fn() -> LocalFuture<Result<String, String>>>,
    pub(crate) copy_text: Rc<dyn Fn(&str)>,
    pub(crate) choose_export_folder: Rc<dyn Fn(&MainWindow) -> FolderChoice>,
    pub(crate) export_archive: Rc<dyn Fn(String) -> LocalFuture<Result<(), String>>>,
    pub(crate) reveal_data_dir: Rc<dyn Fn() -> Result<(), String>>,
    pub(crate) test_mcp: Rc<dyn Fn() -> LocalFuture<Result<String, String>>>,
    pub(crate) check_updates: Rc<dyn Fn() -> LocalFuture<Result<UpdateCheckResult, String>>>,
    pub(crate) delete_model: Rc<dyn Fn() -> Result<(), String>>,
    pub(crate) model_deleted: Rc<dyn Fn()>,
    pub(crate) spawn: Rc<dyn Fn(LocalFuture<()>)>,
    pub(crate) poll_interval: Duration,
}

/// Shared by the diagnostics and MCP "Copy" buttons - no webview clipboard API
/// to fall back on in this shell (see the `arboard` dependency comment in
/// Cargo.toml).
pub(crate) fn copy_to_clipboard(text: &str) {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            if let Err(e) = clipboard.set_text(text) {
                eprintln!("Failed to write to clipboard: {e}");
            }
        }
        Err(e) => eprintln!("Failed to open clipboard: {e}"),
    }
}

fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    context: &'static str,
) -> LocalFuture<Result<T, String>> {
    let worker = souffle_lib::async_runtime::spawn_blocking(work);
    Box::pin(async move {
        worker
            .await
            .map_err(|error| format!("{context} : {error}"))
            .and_then(|result| result)
    })
}

impl SystemBackend {
    pub(crate) fn live(
        state: Arc<AppState>,
        delete_model: impl Fn() -> Result<(), String> + 'static,
        model_deleted: impl Fn() + 'static,
    ) -> Self {
        let diagnostics_state = Arc::clone(&state);
        let export_state = state;
        Self {
            fetch_log_tail: Rc::new(|| {
                blocking(
                    || souffle_lib::commands::get_log_tail(LOG_TAIL_LINES),
                    "Lecture du journal interrompue",
                )
            }),
            diagnostics_text: Rc::new(move || {
                let state = Arc::clone(&diagnostics_state);
                blocking(
                    move || souffle_lib::commands::get_diagnostics_text(state),
                    "Diagnostics interrompus",
                )
            }),
            copy_text: Rc::new(copy_to_clipboard),
            choose_export_folder: Rc::new(|window| {
                slint_kit::native::pick_directory_with_parent(
                    DirectoryRequest::default(),
                    &window.window().window_handle(),
                )
            }),
            export_archive: Rc::new(move |destination| {
                let state = Arc::clone(&export_state);
                Box::pin(async move {
                    souffle_lib::commands::export_archive(state, destination)?;
                    let progress = souffle_lib::async_runtime::spawn(async move {
                        loop {
                            if let Some(progress) =
                                souffle_lib::commands::get_archive_export_progress()
                                && progress.finished
                            {
                                return progress;
                            }
                            tokio::time::sleep(ARCHIVE_EXPORT_POLL).await;
                        }
                    })
                    .await
                    .map_err(|error| format!("Suivi de l'export interrompu : {error}"))?;
                    progress.error.map_or(Ok(()), Err)
                })
            }),
            reveal_data_dir: Rc::new(souffle_lib::commands::reveal_data_dir),
            test_mcp: Rc::new(|| {
                blocking(
                    souffle_lib::commands::test_mcp_connection,
                    "Test MCP interrompu",
                )
            }),
            check_updates: Rc::new(|| Box::pin(souffle_lib::commands::check_for_updates())),
            delete_model: Rc::new(delete_model),
            model_deleted: Rc::new(model_deleted),
            spawn: Rc::new(|future| {
                slint::spawn_local(future).expect("slint event loop not running");
            }),
            poll_interval: LOG_POLL,
        }
    }
}

pub(crate) struct SystemHost {
    window: slint::Weak<MainWindow>,
    backend: SystemBackend,
    log: LogTail,
    log_session: Cell<u64>,
    log_request: Cell<u64>,
    log_timer: slint::Timer,
    copying: Cell<bool>,
    exporting: Cell<bool>,
    testing_mcp: Cell<bool>,
    checking_updates: Cell<bool>,
    delete_armed: Cell<bool>,
}

fn export_failure_from_picker(error: &HostError) -> String {
    let detail = match error {
        HostError::Platform(detail) => detail.as_str(),
        HostError::Unsupported => "non pris en charge",
        HostError::PermissionDenied => "accès refusé",
        HostError::Unavailable => "indisponible",
        HostError::InvalidResource => "dossier invalide",
        HostError::InvalidFilter => "filtre invalide",
    };
    format!("Ouvrir le sélecteur de dossier : {detail}")
}

fn directory_text(handle: &DirectoryHandle) -> Option<String> {
    match &handle.location {
        FileLocation::Path(path) => Some(path.to_string_lossy().into_owned()),
        FileLocation::Uri(_) => None,
    }
}

impl SystemHost {
    pub(crate) fn new(window: &MainWindow, backend: SystemBackend) -> Rc<Self> {
        let host = Rc::new(Self {
            window: window.as_weak(),
            backend,
            log: LogTail::new(),
            log_session: Cell::new(0),
            log_request: Cell::new(0),
            log_timer: slint::Timer::default(),
            copying: Cell::new(false),
            exporting: Cell::new(false),
            testing_mcp: Cell::new(false),
            checking_updates: Cell::new(false),
            delete_armed: Cell::new(false),
        });
        window.set_settings_log_lines(host.log.model());
        host
    }

    #[cfg(test)]
    pub(crate) fn log_len(&self) -> usize {
        self.log.len()
    }

    fn spawn(&self, future: LocalFuture<()>) {
        (self.backend.spawn)(future);
    }

    /// The tail is read only while the System page is the one on screen. Each
    /// change of that condition starts a new session, so an answer that was
    /// still in flight when the page was left is dropped.
    fn set_log_active(self: &Rc<Self>, active: bool) {
        self.log_timer.stop();
        self.log_session.set(self.log_session.get() + 1);
        if !active {
            return;
        }
        self.refresh_log();
        let weak = Rc::downgrade(self);
        self.log_timer.start(
            slint::TimerMode::Repeated,
            self.backend.poll_interval,
            move || {
                if let Some(host) = weak.upgrade() {
                    host.refresh_log();
                }
            },
        );
    }

    fn refresh_log(self: &Rc<Self>) {
        let active = self
            .window
            .upgrade()
            .is_some_and(|window| window.get_settings_log_active());
        if !active {
            return;
        }
        let session = self.log_session.get();
        let request = self.log_request.get() + 1;
        self.log_request.set(request);
        let fetch = (self.backend.fetch_log_tail)();
        let host = self.clone();
        self.spawn(Box::pin(async move {
            let result = fetch.await;
            let current = host.log_session.get() == session && host.log_request.get() == request;
            let active = host
                .window
                .upgrade()
                .is_some_and(|window| window.get_settings_log_active());
            if !current || !active {
                return;
            }
            match result {
                Ok(tail) => host.log.apply(&tail),
                Err(error) => eprintln!("Failed to read log tail: {error}"),
            }
        }));
    }

    fn copy_diagnostics(self: &Rc<Self>) {
        if self.copying.replace(true) {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            window.set_settings_copying_diagnostics(true);
        }
        let text = (self.backend.diagnostics_text)();
        let host = self.clone();
        self.spawn(Box::pin(async move {
            let text = text.await;
            host.copying.set(false);
            let Some(window) = host.window.upgrade() else {
                return;
            };
            window.set_settings_copying_diagnostics(false);
            match text {
                Ok(text) => (host.backend.copy_text)(&text),
                Err(error) => eprintln!("Failed to build diagnostics text: {error}"),
            }
        }));
    }

    fn set_export(&self, status: ExportStatus, detail: &str) {
        if let Some(window) = self.window.upgrade() {
            window.set_settings_data_export_status(status);
            window.set_settings_data_export_detail(detail.into());
        }
    }

    fn finish_export(&self, status: ExportStatus, detail: &str) {
        self.exporting.set(false);
        self.set_export(status, detail);
    }

    fn export(self: &Rc<Self>) {
        if self.exporting.replace(true) {
            return;
        }
        let Some(window) = self.window.upgrade() else {
            self.exporting.set(false);
            return;
        };
        self.set_export(ExportStatus::Choosing, "");
        let chosen = (self.backend.choose_export_folder)(&window);
        drop(window);
        let host = self.clone();
        self.spawn(Box::pin(async move {
            let destination = match chosen.await {
                SelectionOutcome::Selected(handle) => match directory_text(&handle) {
                    Some(destination) => destination,
                    None => {
                        host.finish_export(
                            ExportStatus::Failed,
                            &export_failure_from_picker(&HostError::InvalidResource),
                        );
                        return;
                    }
                },
                SelectionOutcome::Cancelled => {
                    host.finish_export(ExportStatus::Idle, "");
                    return;
                }
                SelectionOutcome::Failed(error) => {
                    host.finish_export(ExportStatus::Failed, &export_failure_from_picker(&error));
                    return;
                }
            };
            host.set_export(ExportStatus::Running, &destination);
            match (host.backend.export_archive)(destination.clone()).await {
                Ok(()) => host.finish_export(ExportStatus::Succeeded, &destination),
                Err(error) => host.finish_export(ExportStatus::Failed, &error),
            }
        }));
    }

    fn copy_property(&self, read: impl Fn(&MainWindow) -> SharedString) {
        if let Some(window) = self.window.upgrade() {
            (self.backend.copy_text)(&read(&window));
        }
    }

    fn set_mcp(&self, status: McpStatus, detail: &str) {
        if let Some(window) = self.window.upgrade() {
            window.set_settings_mcp_status(status);
            window.set_settings_mcp_status_detail(detail.into());
        }
    }

    fn test_mcp(self: &Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if window.get_settings_mcp_status() == McpStatus::Absent || self.testing_mcp.replace(true) {
            return;
        }
        drop(window);
        self.set_mcp(McpStatus::Testing, "");
        let result = (self.backend.test_mcp)();
        let host = self.clone();
        self.spawn(Box::pin(async move {
            let result = result.await;
            host.testing_mcp.set(false);
            match result {
                Ok(tools) => host.set_mcp(McpStatus::Succeeded, &tools),
                Err(error) => host.set_mcp(McpStatus::Failed, &error),
            }
        }));
    }

    fn set_update(&self, status: UpdateCheckStatus, detail: &str) {
        if let Some(window) = self.window.upgrade() {
            window.set_settings_update_status(status);
            window.set_settings_update_detail(detail.into());
        }
    }

    fn check_updates(self: &Rc<Self>) {
        if self.checking_updates.replace(true) {
            return;
        }
        self.set_update(UpdateCheckStatus::Checking, "");
        let result = (self.backend.check_updates)();
        let host = self.clone();
        self.spawn(Box::pin(async move {
            let result = result.await;
            host.checking_updates.set(false);
            match result {
                Ok(UpdateCheckResult {
                    check_error: Some(error),
                    ..
                }) => host.set_update(UpdateCheckStatus::Failed, &error),
                Ok(update) if update.update_available => host.set_update(
                    UpdateCheckStatus::Available,
                    &update.latest_version.unwrap_or_default(),
                ),
                Ok(_) => host.set_update(UpdateCheckStatus::UpToDate, ""),
                Err(error) => host.set_update(UpdateCheckStatus::Failed, &error),
            }
        }));
    }

    /// The confirmation dialog opened: one deletion may follow.
    fn arm_delete(&self) {
        self.delete_armed.set(true);
    }

    /// Only a confirmed, armed request deletes, and it consumes the arming: a
    /// second confirmation of the same dialog does nothing. The command itself
    /// refuses a loaded or mid-transition model with its own text.
    fn delete_model(&self) {
        if !self.delete_armed.replace(false) {
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        match (self.backend.delete_model)() {
            Ok(()) => {
                window.set_settings_model_delete_error("".into());
                (self.backend.model_deleted)();
            }
            Err(error) => window.set_settings_model_delete_error(error.into()),
        }
    }
}

pub(crate) fn wire(window: &MainWindow, host: &Rc<SystemHost>) {
    let h = host.clone();
    window.on_settings_log_activity_changed(move |active| h.set_log_active(active));
    let h = host.clone();
    window.on_settings_copy_diagnostics_requested(move || h.copy_diagnostics());
    let h = host.clone();
    window.on_settings_data_export_requested(move || h.export());
    let h = host.clone();
    window.on_settings_data_reveal_requested(move || {
        if let Err(error) = (h.backend.reveal_data_dir)() {
            eprintln!("Failed to reveal data directory: {error}");
        }
    });
    let h = host.clone();
    window.on_settings_mcp_copy_desktop_snippet_requested(move || {
        h.copy_property(|window| window.get_settings_mcp_claude_desktop_snippet());
    });
    let h = host.clone();
    window.on_settings_mcp_copy_code_command_requested(move || {
        h.copy_property(|window| window.get_settings_mcp_claude_code_command());
    });
    let h = host.clone();
    window.on_settings_mcp_test_requested(move || h.test_mcp());
    let h = host.clone();
    window.on_settings_check_updates_requested(move || h.check_updates());
    let h = host.clone();
    window.on_settings_delete_model_armed(move || h.arm_delete());
    let h = host.clone();
    window.on_settings_delete_model_requested(move || h.delete_model());
    let weak = window.as_weak();
    window.on_settings_permissions_review_requested(move || {
        if let Some(window) = weak.upgrade() {
            window.set_settings_permissions_dialog_open(true);
        }
    });
}
