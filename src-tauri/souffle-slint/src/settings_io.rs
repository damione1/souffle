//! Ordered Settings persistence outside Slint's event loop.
//!
//! The UI submits typed mutations. A single worker applies them to its latest
//! observed durable snapshot, so whole-object saves cannot overtake each
//! other. Responses carry monotonic revisions; the UI may consume an
//! intermediate response for draft bookkeeping, but only the newest response
//! may publish canonical values.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use souffle_lib::commands::{SettingsSaveError, SettingsSaveLane, SettingsSaveOutcome};
use souffle_lib::settings::AppSettings;

use crate::AppHandle;
use crate::settings_values::SettingsCache;

const RESPONSE_POLL_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsResponseOrder {
    LatestVisible,
    LatestHidden,
    Intermediate,
    Stale,
}

impl SettingsResponseOrder {
    pub(crate) fn publishes_to_open_window(self) -> bool {
        match self {
            Self::LatestVisible => true,
            Self::LatestHidden | Self::Intermediate | Self::Stale => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceOrder {
    Latest,
    Intermediate,
    Stale,
}

#[derive(Default)]
struct ResponseSequence {
    latest_submitted: u64,
    latest_observed: u64,
}

impl ResponseSequence {
    fn submit(&mut self) -> u64 {
        self.latest_submitted += 1;
        self.latest_submitted
    }

    fn settle(&mut self, revision: u64) -> SequenceOrder {
        if revision <= self.latest_observed {
            return SequenceOrder::Stale;
        }
        self.latest_observed = revision;
        if revision == self.latest_submitted {
            SequenceOrder::Latest
        } else {
            SequenceOrder::Intermediate
        }
    }
}

type SettingsMutation = Box<dyn FnOnce(&mut AppSettings) + Send>;
type SettingsCompletion = Box<dyn FnOnce(SettingsResponseOrder, &SettingsSaveOutcome)>;
type LoadSettings = Arc<dyn Fn() -> Result<AppSettings, String> + Send + Sync>;
type SaveSettings = Arc<dyn Fn(AppSettings) -> SettingsSaveOutcome + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSessionState {
    Closed { generation: u64 },
    Open { generation: u64 },
}

impl SettingsSessionState {
    fn generation(self) -> u64 {
        match self {
            Self::Closed { generation } | Self::Open { generation } => generation,
        }
    }

    fn is_open(self) -> bool {
        match self {
            Self::Closed { .. } => false,
            Self::Open { .. } => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettingsLoadToken(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SettingsCloseToken(u64);

struct SettingsWorkerIo {
    load_general: LoadSettings,
    load_autostart: LoadSettings,
    save_general: SaveSettings,
    save_autostart: SaveSettings,
}

impl SettingsWorkerIo {
    fn load(&self, lane: SettingsSaveLane) -> Result<AppSettings, String> {
        match lane {
            SettingsSaveLane::General => (self.load_general)(),
            SettingsSaveLane::Autostart => (self.load_autostart)(),
        }
    }

    fn save(&self, lane: SettingsSaveLane, settings: AppSettings) -> SettingsSaveOutcome {
        match lane {
            SettingsSaveLane::General => (self.save_general)(settings),
            SettingsSaveLane::Autostart => (self.save_autostart)(settings),
        }
    }
}

enum WorkerRequest {
    Seed(Box<AppSettings>),
    Barrier {
        revision: u64,
    },
    Save {
        revision: u64,
        lane: SettingsSaveLane,
        mutation: SettingsMutation,
    },
}

struct WorkerResponse {
    revision: u64,
    origin: WorkerResponseOrigin,
    outcome: SettingsSaveOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerResponseOrigin {
    Save,
    Barrier,
}

pub(crate) struct SettingsIoCoordinator {
    requests: tokio::sync::mpsc::UnboundedSender<WorkerRequest>,
    responses: crossbeam_channel::Receiver<WorkerResponse>,
    response_timer: slint::Timer,
    sequence: RefCell<ResponseSequence>,
    callbacks: RefCell<HashMap<u64, SettingsCompletion>>,
    cache: SettingsCache,
    session: Cell<SettingsSessionState>,
}

impl SettingsIoCoordinator {
    pub(crate) fn new(handle: AppHandle, cache: SettingsCache) -> Rc<Self> {
        let stored_handle = handle.clone();
        let effective_handle = handle.clone();
        let general_handle = handle.clone();
        Self::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(move || {
                    souffle_lib::commands::get_stored_settings(stored_handle.clone())
                }),
                load_autostart: Arc::new(move || {
                    souffle_lib::commands::get_settings(effective_handle.clone())
                }),
                save_general: Arc::new(move |settings| {
                    souffle_lib::commands::save_settings_observed_in_lane(
                        general_handle.clone(),
                        settings,
                        SettingsSaveLane::General,
                    )
                }),
                save_autostart: Arc::new(move |settings| {
                    souffle_lib::commands::save_settings_observed_in_lane(
                        handle.clone(),
                        settings,
                        SettingsSaveLane::Autostart,
                    )
                }),
            },
        )
    }

    fn with_io(cache: SettingsCache, io: SettingsWorkerIo) -> Rc<Self> {
        let (requests, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (response_sender, responses) = crossbeam_channel::unbounded();
        souffle_lib::async_runtime::spawn(run_worker(receiver, response_sender, io));

        Rc::new(Self {
            requests,
            responses,
            response_timer: slint::Timer::default(),
            sequence: RefCell::new(ResponseSequence::default()),
            callbacks: RefCell::new(HashMap::new()),
            cache,
            session: Cell::new(SettingsSessionState::Closed { generation: 0 }),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_functions(
        cache: SettingsCache,
        load_general: impl Fn() -> Result<AppSettings, String> + Send + Sync + 'static,
        load_autostart: impl Fn() -> Result<AppSettings, String> + Send + Sync + 'static,
        save_general: impl Fn(AppSettings) -> SettingsSaveOutcome + Send + Sync + 'static,
        save_autostart: impl Fn(AppSettings) -> SettingsSaveOutcome + Send + Sync + 'static,
    ) -> Rc<Self> {
        Self::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(load_general),
                load_autostart: Arc::new(load_autostart),
                save_general: Arc::new(save_general),
                save_autostart: Arc::new(save_autostart),
            },
        )
    }

    pub(crate) fn current_revision(&self) -> u64 {
        self.sequence.borrow().latest_submitted
    }

    pub(crate) fn begin_open(&self) -> SettingsLoadToken {
        let generation = self.session.get().generation().wrapping_add(1);
        self.session.set(SettingsSessionState::Open { generation });
        SettingsLoadToken(generation)
    }

    pub(crate) fn close(&self) -> SettingsCloseToken {
        let generation = self.session.get().generation().wrapping_add(1);
        self.session
            .set(SettingsSessionState::Closed { generation });
        SettingsCloseToken(generation)
    }

    pub(crate) fn accepts_load(&self, token: SettingsLoadToken) -> bool {
        self.session.get()
            == (SettingsSessionState::Open {
                generation: token.0,
            })
    }

    pub(crate) fn current_load_token(&self) -> Option<SettingsLoadToken> {
        match self.session.get() {
            SettingsSessionState::Open { generation } => Some(SettingsLoadToken(generation)),
            SettingsSessionState::Closed { .. } => None,
        }
    }

    pub(crate) fn accepts_close(&self, token: SettingsCloseToken) -> bool {
        self.session.get()
            == (SettingsSessionState::Closed {
                generation: token.0,
            })
    }

    /// Seed the worker only if no write was submitted while the load was in
    /// flight. A late open response must never replace a newer candidate.
    pub(crate) fn seed_if_current(
        &self,
        token: SettingsLoadToken,
        revision_at_start: u64,
        settings: AppSettings,
    ) -> bool {
        if self.current_revision() != revision_at_start || !self.accepts_load(token) {
            return false;
        }
        self.cache.replace_observed(settings.clone());
        self.requests
            .send(WorkerRequest::Seed(Box::new(settings)))
            .is_ok()
    }

    pub(crate) fn submit(
        self: &Rc<Self>,
        lane: SettingsSaveLane,
        mutation: impl FnOnce(&mut AppSettings) + Send + 'static,
        completion: impl FnOnce(SettingsResponseOrder, &SettingsSaveOutcome) + 'static,
    ) -> u64 {
        let revision = self.sequence.borrow_mut().submit();
        self.callbacks
            .borrow_mut()
            .insert(revision, Box::new(completion));
        if self
            .requests
            .send(WorkerRequest::Save {
                revision,
                lane,
                mutation: Box::new(mutation),
            })
            .is_err()
        {
            self.settle(WorkerResponse {
                revision,
                origin: WorkerResponseOrigin::Save,
                outcome: worker_failure("Settings worker is unavailable"),
            });
        }
        self.arm_response_pump();
        revision
    }

    pub(crate) fn barrier(
        self: &Rc<Self>,
        completion: impl FnOnce(SettingsResponseOrder, &SettingsSaveOutcome) + 'static,
    ) -> u64 {
        let revision = self.sequence.borrow_mut().submit();
        self.callbacks
            .borrow_mut()
            .insert(revision, Box::new(completion));
        if self
            .requests
            .send(WorkerRequest::Barrier { revision })
            .is_err()
        {
            self.settle(WorkerResponse {
                revision,
                origin: WorkerResponseOrigin::Barrier,
                outcome: worker_failure("Settings worker is unavailable"),
            });
        }
        self.arm_response_pump();
        revision
    }

    fn arm_response_pump(self: &Rc<Self>) {
        if self.callbacks.borrow().is_empty() || self.response_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.response_timer.start(
            slint::TimerMode::SingleShot,
            RESPONSE_POLL_INTERVAL,
            move || {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                controller.drain_responses();
                controller.arm_response_pump();
            },
        );
    }

    fn drain_responses(&self) {
        while let Ok(response) = self.responses.try_recv() {
            self.settle(response);
        }
    }

    fn settle(&self, response: WorkerResponse) {
        let sequence_order = self.sequence.borrow_mut().settle(response.revision);
        let callback = self.callbacks.borrow_mut().remove(&response.revision);
        let order = match sequence_order {
            SequenceOrder::Latest if self.session.get().is_open() => {
                SettingsResponseOrder::LatestVisible
            }
            SequenceOrder::Latest => SettingsResponseOrder::LatestHidden,
            SequenceOrder::Intermediate => SettingsResponseOrder::Intermediate,
            SequenceOrder::Stale => SettingsResponseOrder::Stale,
        };
        match sequence_order {
            SequenceOrder::Latest | SequenceOrder::Intermediate => match response.origin {
                WorkerResponseOrigin::Save => self.cache.observe_save_outcome(&response.outcome),
                WorkerResponseOrigin::Barrier => match &response.outcome {
                    SettingsSaveOutcome::Observed { result: Ok(()), .. }
                    | SettingsSaveOutcome::Unavailable { result: Ok(()), .. } => {
                        // A barrier proves ordering, not a new successful
                        // write. Refresh the snapshot without erasing the
                        // preceding save error.
                        self.cache.observe_save_outcome_silent(&response.outcome);
                    }
                    SettingsSaveOutcome::Observed { result: Err(_), .. }
                    | SettingsSaveOutcome::Unavailable { result: Err(_), .. } => {
                        self.cache.observe_save_outcome(&response.outcome);
                    }
                },
            },
            SequenceOrder::Stale => {}
        }
        if let Some(callback) = callback {
            callback(order, &response.outcome);
        }
    }

    #[cfg(test)]
    pub(crate) fn drain_for_test(&self) {
        self.drain_responses();
    }

    #[cfg(test)]
    pub(crate) fn is_idle_for_test(&self) -> bool {
        self.callbacks.borrow().is_empty()
    }

    #[cfg(test)]
    fn register_response_for_test(
        &self,
        completion: impl FnOnce(SettingsResponseOrder, &SettingsSaveOutcome) + 'static,
    ) -> u64 {
        let revision = self.sequence.borrow_mut().submit();
        self.callbacks
            .borrow_mut()
            .insert(revision, Box::new(completion));
        revision
    }

    #[cfg(test)]
    fn settle_for_test(&self, revision: u64, outcome: SettingsSaveOutcome) {
        self.settle(WorkerResponse {
            revision,
            origin: WorkerResponseOrigin::Save,
            outcome,
        });
    }
}

async fn run_worker(
    mut requests: tokio::sync::mpsc::UnboundedReceiver<WorkerRequest>,
    responses: crossbeam_channel::Sender<WorkerResponse>,
    io: SettingsWorkerIo,
) {
    let io = Arc::new(io);
    let mut snapshot: Option<AppSettings> = None;
    while let Some(request) = requests.recv().await {
        match request {
            WorkerRequest::Seed(settings) => snapshot = Some(*settings),
            WorkerRequest::Barrier { revision } => {
                let current = snapshot.take();
                let io = Arc::clone(&io);
                let result = souffle_lib::async_runtime::spawn_blocking(move || match current {
                    Some(settings) => {
                        let outcome = SettingsSaveOutcome::Observed {
                            settings: Box::new(settings.clone()),
                            result: Ok(()),
                        };
                        (Some(settings), outcome)
                    }
                    None => match io.load(SettingsSaveLane::General) {
                        Ok(settings) => {
                            let outcome = SettingsSaveOutcome::Observed {
                                settings: Box::new(settings.clone()),
                                result: Ok(()),
                            };
                            (Some(settings), outcome)
                        }
                        Err(read_error) => (
                            None,
                            SettingsSaveOutcome::Unavailable {
                                result: Err(SettingsSaveError::NotCommitted {
                                    message: "Settings could not be read before the barrier".into(),
                                }),
                                read_error,
                            },
                        ),
                    },
                })
                .await;
                let outcome = match result {
                    Ok((next, outcome)) => {
                        snapshot = next;
                        outcome
                    }
                    Err(error) => {
                        snapshot = None;
                        worker_failure(&format!("Settings barrier failed: {error}"))
                    }
                };
                if responses
                    .send(WorkerResponse {
                        revision,
                        origin: WorkerResponseOrigin::Barrier,
                        outcome,
                    })
                    .is_err()
                {
                    break;
                }
            }
            WorkerRequest::Save {
                revision,
                lane,
                mutation,
            } => {
                let current = snapshot.take();
                let io = Arc::clone(&io);
                let result = souffle_lib::async_runtime::spawn_blocking(move || {
                    let fallback = current.clone();
                    let loaded = match lane {
                        SettingsSaveLane::General => match current {
                            Some(settings) => Ok(settings),
                            None => io.load(SettingsSaveLane::General),
                        },
                        // ServiceManagement is an effective native value, not
                        // part of a generic stored snapshot. Refresh it at the
                        // exact typed boundary even if earlier General saves
                        // already populated the worker cache.
                        SettingsSaveLane::Autostart => io.load(SettingsSaveLane::Autostart),
                    };
                    let mut candidate = match loaded {
                        Ok(settings) => settings,
                        Err(read_error) => {
                            let outcome = SettingsSaveOutcome::Unavailable {
                                result: Err(SettingsSaveError::NotCommitted {
                                    message: "Settings could not be loaded before saving".into(),
                                }),
                                read_error,
                            };
                            return (fallback, outcome);
                        }
                    };
                    mutation(&mut candidate);
                    let outcome = io.save(lane, candidate);
                    let next = match &outcome {
                        SettingsSaveOutcome::Observed { settings, .. } => {
                            Some(settings.as_ref().clone())
                        }
                        SettingsSaveOutcome::Unavailable { .. } => None,
                    };
                    (next, outcome)
                })
                .await;

                let outcome = match result {
                    Ok((next, outcome)) => {
                        snapshot = next;
                        outcome
                    }
                    Err(error) => {
                        snapshot = None;
                        worker_failure(&format!("Settings worker failed: {error}"))
                    }
                };
                if responses
                    .send(WorkerResponse {
                        revision,
                        origin: WorkerResponseOrigin::Save,
                        outcome,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

fn worker_failure(message: &str) -> SettingsSaveOutcome {
    SettingsSaveOutcome::Unavailable {
        result: Err(SettingsSaveError::NotCommitted {
            message: message.into(),
        }),
        read_error: "The durable Settings snapshot is unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MainWindow, SettingsTab};
    use slint::ComponentHandle;
    use slint::PhysicalSize;
    use slint::platform::software_renderer::{
        MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
    };
    use slint::platform::{Platform, WindowAdapter};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestPlatform;

    #[derive(Clone, Copy, Default)]
    struct BenchmarkPixel {
        red: u8,
        green: u8,
        blue: u8,
    }

    impl TargetPixel for BenchmarkPixel {
        fn blend(&mut self, color: PremultipliedRgbaColor) {
            let inverse_alpha = 255_u32 - u32::from(color.alpha);
            self.red =
                (u32::from(color.red) + u32::from(self.red) * inverse_alpha / 255).min(255) as u8;
            self.green = (u32::from(color.green) + u32::from(self.green) * inverse_alpha / 255)
                .min(255) as u8;
            self.blue =
                (u32::from(color.blue) + u32::from(self.blue) * inverse_alpha / 255).min(255) as u8;
        }

        fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
            Self { red, green, blue }
        }
    }

    thread_local! {
        static TEST_ADAPTER: Rc<MinimalSoftwareWindow> =
            MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        static TEST_FRAMEBUFFER: RefCell<Vec<BenchmarkPixel>> =
            RefCell::new(vec![BenchmarkPixel::default(); 1_024 * 800]);
    }

    impl Platform for TestPlatform {
        fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
            Ok(TEST_ADAPTER.with(Rc::clone))
        }
    }

    fn test_window() -> MainWindow {
        let _ = slint::platform::set_platform(Box::new(TestPlatform));
        TEST_ADAPTER.with(|adapter| adapter.set_size(PhysicalSize::new(1_024, 800)));
        MainWindow::new().unwrap()
    }

    fn render_test_window() {
        const WIDTH: usize = 1_024;
        TEST_ADAPTER.with(|adapter| {
            adapter.request_redraw();
            TEST_FRAMEBUFFER.with(|buffer| {
                adapter.draw_if_needed(|renderer| {
                    renderer.render(&mut buffer.borrow_mut(), WIDTH);
                });
            });
        });
    }

    fn immediate_io() -> SettingsWorkerIo {
        SettingsWorkerIo {
            load_general: Arc::new(|| Ok(AppSettings::default())),
            load_autostart: Arc::new(|| Ok(AppSettings::default())),
            save_general: Arc::new(|settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            }),
            save_autostart: Arc::new(|settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            }),
        }
    }

    #[test]
    fn blocked_save_leaves_the_ui_free_to_switch_tabs() {
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let coordinator = SettingsIoCoordinator::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(|| Ok(AppSettings::default())),
                load_autostart: Arc::new(|| Ok(AppSettings::default())),
                save_general: Arc::new(move |settings| {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(settings),
                        result: Ok(()),
                    }
                }),
                save_autostart: Arc::new(|settings| SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }),
            },
        );

        coordinator.submit(
            SettingsSaveLane::General,
            |s| s.auto_paste = false,
            |_, _| {},
        );
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("controlled save started");

        let processed = Rc::new(Cell::new(false));
        let processed_in_timer = Rc::clone(&processed);
        let weak = window.as_weak();
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
            if let Some(window) = weak.upgrade() {
                window.set_settings_tab(SettingsTab::Audio);
                processed_in_timer.set(true);
            }
        });
        slint::platform::update_timers_and_animations();
        assert!(processed.get(), "Slint event-loop callback must run");
        assert_eq!(window.get_settings_tab(), SettingsTab::Audio);

        release_tx.send(()).unwrap();
    }

    #[test]
    fn barrier_settles_only_after_the_blocked_save_response() {
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let coordinator = SettingsIoCoordinator::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(|| Ok(AppSettings::default())),
                load_autostart: Arc::new(|| Ok(AppSettings::default())),
                save_general: Arc::new(move |settings| {
                    started_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(settings),
                        result: Err(SettingsSaveError::NotCommitted {
                            message: "injected failure".into(),
                        }),
                    }
                }),
                save_autostart: Arc::new(|settings| SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }),
            },
        );
        coordinator.submit(SettingsSaveLane::General, |_| {}, |_, _| {});
        let barrier_settled = Rc::new(Cell::new(false));
        let settled = Rc::clone(&barrier_settled);
        coordinator.barrier(move |_, _| settled.set(true));
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("controlled save started");
        coordinator.drain_for_test();
        assert!(!barrier_settled.get());

        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !barrier_settled.get() {
            coordinator.drain_for_test();
            assert!(
                std::time::Instant::now() < deadline,
                "barrier did not settle"
            );
            std::thread::yield_now();
        }
        assert!(
            window
                .get_settings_save_error()
                .contains("injected failure"),
            "a successful ordering barrier must not erase the preceding save error"
        );
    }

    #[test]
    fn inverse_responses_never_publish_an_older_revision() {
        let mut sequence = ResponseSequence::default();
        let first = sequence.submit();
        let second = sequence.submit();

        assert_eq!(sequence.settle(second), SequenceOrder::Latest);
        assert_eq!(sequence.settle(first), SequenceOrder::Stale);
    }

    #[test]
    fn inverse_completions_keep_the_latest_value_visible_and_cached() {
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let coordinator = SettingsIoCoordinator::with_io(cache.clone(), immediate_io());
        coordinator.begin_open();

        let weak = window.as_weak();
        let publish = move |order: SettingsResponseOrder, outcome: &SettingsSaveOutcome| match order
        {
            SettingsResponseOrder::LatestVisible => {
                let SettingsSaveOutcome::Observed { settings, .. } = outcome else {
                    return;
                };
                if let Some(window) = weak.upgrade() {
                    window.set_settings_paste_delay_ms(settings.paste_delay_ms as i32);
                }
            }
            SettingsResponseOrder::LatestHidden
            | SettingsResponseOrder::Intermediate
            | SettingsResponseOrder::Stale => {}
        };
        let first = coordinator.register_response_for_test(|_, _| {});
        let second = coordinator.register_response_for_test(publish);

        let latest = AppSettings {
            paste_delay_ms: 250,
            ..AppSettings::default()
        };
        coordinator.settle_for_test(
            second,
            SettingsSaveOutcome::Observed {
                settings: Box::new(latest),
                result: Ok(()),
            },
        );
        let older = AppSettings {
            paste_delay_ms: 200,
            ..AppSettings::default()
        };
        coordinator.settle_for_test(
            first,
            SettingsSaveOutcome::Observed {
                settings: Box::new(older),
                result: Ok(()),
            },
        );

        assert_eq!(window.get_settings_paste_delay_ms(), 250);
        assert_eq!(cache.known_snapshot().unwrap().paste_delay_ms, 250);
    }

    #[test]
    fn serialized_mutations_make_the_last_input_durable() {
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let durable = Arc::new(Mutex::new(AppSettings::default()));
        let durable_for_load = Arc::clone(&durable);
        let durable_for_save = Arc::clone(&durable);
        let coordinator = SettingsIoCoordinator::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(move || Ok(durable_for_load.lock().unwrap().clone())),
                load_autostart: Arc::new(|| Ok(AppSettings::default())),
                save_general: Arc::new(move |settings| {
                    *durable_for_save.lock().unwrap() = settings.clone();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(settings),
                        result: Ok(()),
                    }
                }),
                save_autostart: Arc::new(|settings| SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }),
            },
        );

        coordinator.submit(
            SettingsSaveLane::General,
            |s| s.paste_delay_ms = 200,
            |_, _| {},
        );
        coordinator.submit(
            SettingsSaveLane::General,
            |s| s.paste_delay_ms = 250,
            |_, _| {},
        );

        for _ in 0..100 {
            coordinator.drain_for_test();
            if durable.lock().unwrap().paste_delay_ms == 250 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(durable.lock().unwrap().paste_delay_ms, 250);
    }

    #[test]
    fn autostart_lane_refreshes_effective_state_after_a_general_save() {
        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let durable = Arc::new(Mutex::new(AppSettings::default()));
        let general_load_state = Arc::clone(&durable);
        let autostart_load_state = Arc::clone(&durable);
        let general_save_state = Arc::clone(&durable);
        let autostart_save_state = Arc::clone(&durable);
        let autostart_loads = Arc::new(AtomicUsize::new(0));
        let autostart_load_count = Arc::clone(&autostart_loads);
        let coordinator = SettingsIoCoordinator::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(move || Ok(general_load_state.lock().unwrap().clone())),
                load_autostart: Arc::new(move || {
                    autostart_load_count.fetch_add(1, Ordering::SeqCst);
                    let mut settings = autostart_load_state.lock().unwrap().clone();
                    settings.autostart_enabled = true;
                    Ok(settings)
                }),
                save_general: Arc::new(move |settings| {
                    *general_save_state.lock().unwrap() = settings.clone();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(settings),
                        result: Ok(()),
                    }
                }),
                save_autostart: Arc::new(move |settings| {
                    *autostart_save_state.lock().unwrap() = settings.clone();
                    SettingsSaveOutcome::Observed {
                        settings: Box::new(settings),
                        result: Ok(()),
                    }
                }),
            },
        );

        coordinator.submit(
            SettingsSaveLane::General,
            |settings| settings.locale = "fr".into(),
            |_, _| {},
        );
        coordinator.submit(SettingsSaveLane::Autostart, |_| {}, |_, _| {});

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !coordinator.is_idle_for_test() {
            coordinator.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let durable = durable.lock().unwrap();
        assert_eq!(durable.locale, "fr");
        assert!(durable.autostart_enabled);
        assert_eq!(autostart_loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn close_and_reopen_cancel_every_older_load_token() {
        let window = test_window();
        let weak = window.as_weak();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let coordinator = SettingsIoCoordinator::with_io(cache, immediate_io());
        let first = coordinator.begin_open();
        let revision = coordinator.current_revision();
        coordinator.close();
        assert!(!coordinator.accepts_load(first));

        let second = coordinator.begin_open();
        window.set_settings_new_summary_template_draft("newer draft".into());
        assert!(!coordinator.accepts_load(first));
        assert!(coordinator.accepts_load(second));
        assert!(!coordinator.seed_if_current(first, revision, AppSettings::default()));
        assert_eq!(
            window.get_settings_new_summary_template_draft().as_str(),
            "newer draft"
        );

        drop(window);
        assert!(
            weak.upgrade().is_none(),
            "load/session state must not retain MainWindow"
        );
    }

    /// AC5 manual harness. The window/framebuffer are already allocated and
    /// the closed home frame is rendered before timing. Opening Settings is
    /// then rendered with the software backend solely to force item-tree and
    /// layout materialization; no callback/OS/DB command is wired. Absolute
    /// raster time is not a Skia/Metal claim. Run this exact test in debug
    /// and `--release`; both paths use 30 repetitions and identical data.
    #[test]
    #[ignore = "manual AC5 cold/warm mount measurement"]
    fn measure_settings_mount_cold_and_warm_thirty_repetitions() {
        const REPETITIONS: usize = 30;

        fn measure_open(window: &MainWindow) -> u128 {
            let started = std::time::Instant::now();
            window.set_settings_open(true);
            slint::platform::update_timers_and_animations();
            render_test_window();
            started.elapsed().as_nanos()
        }

        fn median(samples: &mut [u128]) -> u128 {
            samples.sort_unstable();
            samples[samples.len() / 2]
        }

        let fixture = AppSettings::default();
        let mut cold = Vec::with_capacity(REPETITIONS);
        for _ in 0..REPETITIONS {
            let window = test_window();
            crate::settings_ui::populate(&window, &fixture);
            render_test_window();
            cold.push(measure_open(&window));
        }

        let window = test_window();
        crate::settings_ui::populate(&window, &fixture);
        window.set_settings_open(true);
        slint::platform::update_timers_and_animations();
        let mut warm = Vec::with_capacity(REPETITIONS);
        for _ in 0..REPETITIONS {
            window.set_settings_open(false);
            slint::platform::update_timers_and_animations();
            render_test_window();
            warm.push(measure_open(&window));
        }

        println!(
            "SOU-202 forced materialization: repetitions={REPETITIONS} cold_median_ns={} warm_median_ns={} cold={cold:?} warm={warm:?}",
            median(&mut cold),
            median(&mut warm),
        );
    }
}
