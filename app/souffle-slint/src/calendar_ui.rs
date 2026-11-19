//! Meetings settings: the calendar catalogue, the selection by calendar identity
//! and the permission flow. EventKit, the permission and the stored preferences
//! stay in Rust; the page only shows the projection and reports intentions.
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use slint::{ComponentHandle, Model};
use souffle_lib::calendar::CalendarInfo;
use souffle_lib::commands::{SettingsSaveLane, SettingsSaveOutcome};
use souffle_lib::permissions::{PermState, PermissionKind as DomainPermissionKind};

use crate::calendar_selection::{Refusal, SelectionChange, change_selection, publish_rows, rows};
use crate::settings_io::{SettingsIoCoordinator, SettingsResponseOrder};
use crate::settings_ui::perm_state_to_slint;
use crate::settings_values::SettingsCache;
use crate::{AccessState, MainWindow};

pub(crate) type LocalFuture<T> = Pin<Box<dyn Future<Output = T>>>;

#[cfg(test)]
mod tests;

struct CatalogState {
    permission: PermState,
    calendars: Vec<CalendarInfo>,
    request: u64,
}

/// What Rust last learned from EventKit. It is shared with the settings worker,
/// which checks a gesture against it at the moment the write happens.
#[derive(Clone)]
pub(crate) struct CalendarCatalog(Arc<Mutex<CatalogState>>);

impl CalendarCatalog {
    pub(crate) fn new(permission: PermState) -> Self {
        Self(Arc::new(Mutex::new(CatalogState {
            permission,
            calendars: Vec::new(),
            request: 0,
        })))
    }

    fn state(&self) -> MutexGuard<'_, CatalogState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn snapshot(&self) -> (PermState, Vec<CalendarInfo>) {
        let state = self.state();
        (state.permission, state.calendars.clone())
    }

    fn replace(&self, permission: PermState, calendars: Vec<CalendarInfo>) {
        let mut state = self.state();
        state.permission = permission;
        state.calendars = calendars;
    }

    /// Starts a read and invalidates the ones still in flight.
    fn begin_request(&self) -> u64 {
        let mut state = self.state();
        state.request += 1;
        state.request
    }

    fn is_latest(&self, request: u64) -> bool {
        self.state().request == request
    }
}

/// The system and application effects the page depends on. Tests replace them.
pub(crate) struct CalendarBackend {
    pub(crate) authorization: Arc<dyn Fn() -> PermState + Send + Sync>,
    pub(crate) list: Arc<dyn Fn() -> Result<Vec<CalendarInfo>, String> + Send + Sync>,
    pub(crate) request_access: Rc<dyn Fn() -> LocalFuture<PermState>>,
    pub(crate) open_system_settings: Rc<dyn Fn()>,
    pub(crate) spawn: Rc<dyn Fn(LocalFuture<()>)>,
    pub(crate) clear_upcoming: Rc<dyn Fn(&MainWindow)>,
    pub(crate) refresh_upcoming: Rc<dyn Fn(&MainWindow)>,
}

impl CalendarBackend {
    pub(crate) fn live(
        clear_upcoming: impl Fn(&MainWindow) + 'static,
        refresh_upcoming: impl Fn(&MainWindow) + 'static,
    ) -> Self {
        Self {
            authorization: Arc::new(souffle_lib::calendar::authorization_state),
            list: Arc::new(souffle_lib::calendar::list_calendars),
            request_access: Rc::new(|| {
                Box::pin(async {
                    souffle_lib::commands::request_permission(DomainPermissionKind::Calendar)
                        .await
                        .unwrap_or(PermState::Denied)
                })
            }),
            open_system_settings: Rc::new(souffle_lib::commands::open_calendar_settings),
            spawn: Rc::new(|future| {
                slint::spawn_local(future).expect("slint event loop not running");
            }),
            clear_upcoming: Rc::new(clear_upcoming),
            refresh_upcoming: Rc::new(refresh_upcoming),
        }
    }
}

pub(crate) struct CalendarHost {
    io: Rc<SettingsIoCoordinator>,
    settings: SettingsCache,
    catalog: CalendarCatalog,
    backend: CalendarBackend,
    enabling: Cell<bool>,
}

impl CalendarHost {
    pub(crate) fn new(
        io: Rc<SettingsIoCoordinator>,
        settings: SettingsCache,
        backend: CalendarBackend,
    ) -> Rc<Self> {
        let permission = (backend.authorization)();
        Rc::new(Self {
            io,
            settings,
            catalog: CalendarCatalog::new(permission),
            backend,
            enabling: Cell::new(false),
        })
    }

    fn integration_enabled(&self) -> Option<bool> {
        self.settings
            .borrow()
            .as_ref()
            .map(|settings| settings.calendar_integration_enabled)
    }

    fn stored_selection(&self) -> Vec<String> {
        self.settings
            .borrow()
            .as_ref()
            .map(|settings| settings.calendar_selected_ids.clone())
            .unwrap_or_default()
    }
}

fn log_save_failure(outcome: &SettingsSaveOutcome) {
    match outcome {
        SettingsSaveOutcome::Observed { result, .. } => {
            if let Err(error) = result {
                eprintln!("Failed to save settings: {error}");
            }
        }
        SettingsSaveOutcome::Unavailable { result, read_error } => {
            eprintln!("Settings state unavailable after save {result:?}: {read_error}");
        }
    }
}

fn show_unavailable(window: &MainWindow, host: &CalendarHost, permission: PermState) {
    host.catalog.begin_request();
    host.catalog.replace(permission, Vec::new());
    window.set_settings_calendar_permission(perm_state_to_slint(permission));
    publish_rows(window, Vec::new());
}

/// Reads the catalogue off the UI thread and publishes it. Without a distinct
/// loading state in the page contract, the last projection stays visible until
/// the answer arrives.
pub(crate) fn reload(window: &MainWindow, host: &Rc<CalendarHost>) {
    let Some(token) = host.io.current_load_token() else {
        return;
    };
    match host.integration_enabled() {
        None => return,
        Some(false) => {
            show_unavailable(window, host, (host.backend.authorization)());
            return;
        }
        Some(true) => {}
    }
    let request = host.catalog.begin_request();
    let list = host.backend.list.clone();
    let worker = souffle_lib::async_runtime::spawn_blocking(move || list());
    let weak = window.as_weak();
    let host = host.clone();
    (host.backend.spawn.clone())(Box::pin(async move {
        let result = worker.await;
        if !host.io.accepts_load(token) {
            return;
        }
        let Some(window) = weak.upgrade() else {
            return;
        };
        finish_reload(&window, &host, request, result.ok());
    }));
}

/// Publishes a finished read. `None` is a worker that failed to run.
fn finish_reload(
    window: &MainWindow,
    host: &CalendarHost,
    request: u64,
    result: Option<Result<Vec<CalendarInfo>, String>>,
) {
    if !host.catalog.is_latest(request) || host.integration_enabled() != Some(true) {
        return;
    }
    match result {
        Some(Ok(calendars)) => {
            host.catalog.replace(PermState::Granted, calendars.clone());
            window.set_settings_calendar_permission(AccessState::Granted);
            publish_rows(window, rows(&calendars, &host.stored_selection()));
        }
        Some(Err(_)) => {
            let permission = (host.backend.authorization)();
            if permission != PermState::Granted {
                show_unavailable(window, host, permission);
            }
        }
        None => {}
    }
}

fn disable(window: &MainWindow, host: &Rc<CalendarHost>) {
    let weak = window.as_weak();
    let host_for_completion = host.clone();
    host.io.submit(
        SettingsSaveLane::General,
        |settings| settings.calendar_integration_enabled = false,
        move |order, outcome| {
            log_save_failure(outcome);
            if !order.publishes_to_open_window() {
                return;
            }
            let SettingsSaveOutcome::Observed { settings, .. } = outcome else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            window.set_settings_calendar_enabled(settings.calendar_integration_enabled);
            if !settings.calendar_integration_enabled {
                let permission = (host_for_completion.backend.authorization)();
                show_unavailable(&window, &host_for_completion, permission);
                (host_for_completion.backend.clear_upcoming)(&window);
            }
        },
    );
}

fn begin_enable(window: &MainWindow, host: &Rc<CalendarHost>) {
    if host.enabling.replace(true) {
        return;
    }
    let weak = window.as_weak();
    let host = host.clone();
    let request = (host.backend.request_access)();
    (host.backend.spawn.clone())(Box::pin(async move {
        let permission = request.await;
        host.enabling.set(false);
        let Some(window) = weak.upgrade() else {
            return;
        };
        finish_enable(&window, &host, permission);
    }));
}

/// A refused or missing permission keeps the integration off and shows why. A
/// granted one is persisted before the page reports the integration on.
fn finish_enable(window: &MainWindow, host: &Rc<CalendarHost>, permission: PermState) {
    if permission != PermState::Granted {
        window.set_settings_calendar_enabled(false);
        show_unavailable(window, host, permission);
        return;
    }
    let weak = window.as_weak();
    let host_for_completion = host.clone();
    host.io.submit(
        SettingsSaveLane::General,
        |settings| settings.calendar_integration_enabled = true,
        move |order, outcome| {
            log_save_failure(outcome);
            if !order.publishes_to_open_window() {
                return;
            }
            let SettingsSaveOutcome::Observed { settings, .. } = outcome else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            window.set_settings_calendar_enabled(settings.calendar_integration_enabled);
            if !settings.calendar_integration_enabled {
                let permission = (host_for_completion.backend.authorization)();
                show_unavailable(&window, &host_for_completion, permission);
                (host_for_completion.backend.clear_upcoming)(&window);
                return;
            }
            reload(&window, &host_for_completion);
            (host_for_completion.backend.refresh_upcoming)(&window);
        },
    );
}

/// Writes the state the checkbox asked for. The intention is the projection the
/// user saw (`selected` flipped), applied by identity: the worker verifies the
/// integration, the permission and the calendar's membership against the latest
/// catalogue when the write really happens, so a calendar removed or a
/// permission revoked in the meantime changes nothing.
fn toggle(window: &MainWindow, host: &Rc<CalendarHost>, id: &str) {
    if !window.get_settings_calendar_enabled()
        || window.get_settings_calendar_permission() != AccessState::Granted
    {
        return;
    }
    let model = window.get_settings_calendars();
    let Some(shown) = model.iter().find(|row| row.id == id) else {
        return;
    };
    let selected = !shown.selected;
    let id = id.to_string();
    let catalog = host.catalog.clone();
    let weak = window.as_weak();
    let host_for_completion = host.clone();
    host.io.submit(
        SettingsSaveLane::General,
        move |settings| {
            if !settings.calendar_integration_enabled {
                return;
            }
            let (permission, calendars) = catalog.snapshot();
            if permission != PermState::Granted {
                return;
            }
            match change_selection(&settings.calendar_selected_ids, &calendars, &id, selected) {
                SelectionChange::Write(ids) => settings.calendar_selected_ids = ids,
                SelectionChange::Unchanged(Refusal::AlreadyApplied) => {}
                SelectionChange::Unchanged(
                    refusal @ (Refusal::UnknownCalendar | Refusal::LastCalendar),
                ) => {
                    eprintln!("Calendar selection kept ({refusal:?}) for {id}");
                }
            }
        },
        move |order: SettingsResponseOrder, outcome| {
            log_save_failure(outcome);
            if !order.publishes_to_open_window() {
                return;
            }
            let SettingsSaveOutcome::Observed { settings, .. } = outcome else {
                return;
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            let (_, calendars) = host_for_completion.catalog.snapshot();
            publish_rows(&window, rows(&calendars, &settings.calendar_selected_ids));
            reload(&window, &host_for_completion);
        },
    );
}

pub(crate) fn wire(window: &MainWindow, host: &Rc<CalendarHost>) {
    let weak = window.as_weak();
    let for_enabled = host.clone();
    window.on_settings_calendar_enabled_changed(move |enabled| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if enabled {
            begin_enable(&window, &for_enabled);
        } else {
            disable(&window, &for_enabled);
        }
    });

    let weak = window.as_weak();
    let for_toggle = host.clone();
    window.on_settings_calendar_toggled(move |id| {
        if let Some(window) = weak.upgrade() {
            toggle(&window, &for_toggle, id.as_str());
        }
    });

    let weak = window.as_weak();
    let for_autostart = host.clone();
    window.on_settings_calendar_autostart_enabled_changed(move |enabled| {
        for_autostart.io.submit(
            SettingsSaveLane::General,
            move |settings| settings.calendar_autostart_enabled = enabled,
            |_, outcome| log_save_failure(outcome),
        );
        if let Some(window) = weak.upgrade() {
            window.set_settings_calendar_autostart_enabled(enabled);
        }
    });

    let for_open = host.clone();
    window.on_settings_calendar_open_system_settings_requested(move || {
        (for_open.backend.open_system_settings)();
    });
}
