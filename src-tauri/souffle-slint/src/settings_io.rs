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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use souffle_lib::commands::{SettingsSaveError, SettingsSaveLane, SettingsSaveOutcome};
use souffle_lib::settings::AppSettings;

use crate::AppHandle;
use crate::settings_values::SettingsCache;

static NEXT_RESPONSE_TARGET: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static RESPONSE_TARGETS: RefCell<HashMap<u64, std::rc::Weak<SettingsIoCoordinator>>> =
        RefCell::new(HashMap::new());
}

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
type SettingsSnapshotCompletion = Box<dyn FnOnce(SettingsSnapshotResult)>;
type SettingsSaveProjection = Rc<dyn Fn(SettingsResponseOrder, &SettingsSaveOutcome)>;
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

pub(crate) enum SettingsSnapshotResult {
    Observed(Box<AppSettings>),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSnapshotAudience {
    Startup,
    Open(SettingsLoadToken),
}

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
    #[cfg(test)]
    Seed(Box<AppSettings>),
    Refresh {
        revision: u64,
        lane: SettingsSaveLane,
    },
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

struct ResponsePublisher {
    sender: crossbeam_channel::Sender<WorkerResponse>,
    wake_pending: Arc<AtomicBool>,
    target_id: u64,
}

fn reserve_response_wake(wake_pending: &AtomicBool) -> bool {
    !wake_pending.swap(true, Ordering::AcqRel)
}

impl ResponsePublisher {
    fn send(&self, response: WorkerResponse) -> bool {
        if self.sender.send(response).is_err() {
            return false;
        }
        if !reserve_response_wake(&self.wake_pending) {
            return true;
        }
        let wake_pending = Arc::clone(&self.wake_pending);
        let target_id = self.target_id;
        if slint::invoke_from_event_loop(move || {
            wake_pending.store(false, Ordering::Release);
            RESPONSE_TARGETS.with(|targets| {
                let controller = targets
                    .borrow()
                    .get(&target_id)
                    .and_then(std::rc::Weak::upgrade);
                if let Some(controller) = controller {
                    controller.drain_responses();
                }
            });
        })
        .is_err()
        {
            self.wake_pending.store(false, Ordering::Release);
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerResponseOrigin {
    Save,
    Refresh,
    Barrier,
}

pub(crate) struct SettingsIoCoordinator {
    requests: tokio::sync::mpsc::UnboundedSender<WorkerRequest>,
    responses: crossbeam_channel::Receiver<WorkerResponse>,
    response_target_id: u64,
    response_wake_pending: Arc<AtomicBool>,
    sequence: RefCell<ResponseSequence>,
    callbacks: RefCell<HashMap<u64, SettingsCompletion>>,
    save_projection: RefCell<Option<SettingsSaveProjection>>,
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
        let response_target_id = NEXT_RESPONSE_TARGET.fetch_add(1, Ordering::Relaxed);
        let response_wake_pending = Arc::new(AtomicBool::new(false));
        let initial_snapshot = cache.known_snapshot();
        souffle_lib::async_runtime::spawn(run_worker(
            receiver,
            ResponsePublisher {
                sender: response_sender,
                wake_pending: Arc::clone(&response_wake_pending),
                target_id: response_target_id,
            },
            io,
            initial_snapshot,
        ));

        let controller = Rc::new(Self {
            requests,
            responses,
            response_target_id,
            response_wake_pending,
            sequence: RefCell::new(ResponseSequence::default()),
            callbacks: RefCell::new(HashMap::new()),
            save_projection: RefCell::new(None),
            cache,
            session: Cell::new(SettingsSessionState::Closed { generation: 0 }),
        });
        RESPONSE_TARGETS.with(|targets| {
            targets
                .borrow_mut()
                .insert(response_target_id, Rc::downgrade(&controller));
        });
        controller
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

    #[cfg(test)]
    pub(crate) fn current_revision(&self) -> u64 {
        self.sequence.borrow().latest_submitted
    }

    pub(crate) fn set_save_projection(
        &self,
        projection: impl Fn(SettingsResponseOrder, &SettingsSaveOutcome) + 'static,
    ) {
        *self.save_projection.borrow_mut() = Some(Rc::new(projection));
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
    #[cfg(test)]
    pub(crate) fn seed_if_current(
        &self,
        token: SettingsLoadToken,
        revision_at_start: u64,
        settings: AppSettings,
    ) -> bool {
        let sequence = self.sequence.borrow();
        let fully_settled = sequence.latest_submitted == revision_at_start
            && sequence.latest_observed == revision_at_start;
        drop(sequence);
        if !fully_settled || !self.accepts_load(token) {
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
        revision
    }

    /// Serialize the Settings-open effective refresh behind any writes that
    /// were already queued before the sheet opened. This is the only refresh
    /// that consults ServiceManagement, through the explicit Autostart lane.
    pub(crate) fn load_effective_snapshot(
        self: &Rc<Self>,
        token: SettingsLoadToken,
        completion: impl FnOnce(AppSettings) + 'static,
    ) {
        let completion = Rc::new(RefCell::new(Some(Box::new(move |result| {
            if let SettingsSnapshotResult::Observed(settings) = result {
                completion(*settings);
            }
        }) as SettingsSnapshotCompletion)));
        self.queue_snapshot_refresh(
            SettingsSaveLane::Autostart,
            SettingsSnapshotAudience::Open(token),
            completion,
        );
    }

    /// Load the initial durable snapshot on the serialized worker. This is
    /// queued before Slint starts processing user input, so startup theme and
    /// onboarding state never perform database I/O on the UI thread.
    pub(crate) fn load_startup_snapshot(
        self: &Rc<Self>,
        completion: impl FnOnce(SettingsSnapshotResult) + 'static,
    ) {
        let completion = Rc::new(RefCell::new(Some(
            Box::new(completion) as SettingsSnapshotCompletion
        )));
        self.queue_snapshot_refresh(
            SettingsSaveLane::General,
            SettingsSnapshotAudience::Startup,
            completion,
        );
    }

    fn accepts_snapshot_audience(&self, audience: SettingsSnapshotAudience) -> bool {
        match audience {
            SettingsSnapshotAudience::Startup => true,
            SettingsSnapshotAudience::Open(token) => self.accepts_load(token),
        }
    }

    fn order_publishes_snapshot(
        audience: SettingsSnapshotAudience,
        order: SettingsResponseOrder,
    ) -> bool {
        match (audience, order) {
            (
                SettingsSnapshotAudience::Startup,
                SettingsResponseOrder::LatestVisible | SettingsResponseOrder::LatestHidden,
            )
            | (SettingsSnapshotAudience::Open(_), SettingsResponseOrder::LatestVisible) => true,
            (
                SettingsSnapshotAudience::Startup,
                SettingsResponseOrder::Intermediate | SettingsResponseOrder::Stale,
            )
            | (
                SettingsSnapshotAudience::Open(_),
                SettingsResponseOrder::LatestHidden
                | SettingsResponseOrder::Intermediate
                | SettingsResponseOrder::Stale,
            ) => false,
        }
    }

    fn queue_snapshot_refresh(
        self: &Rc<Self>,
        lane: SettingsSaveLane,
        audience: SettingsSnapshotAudience,
        completion: Rc<RefCell<Option<SettingsSnapshotCompletion>>>,
    ) {
        let revision = self.sequence.borrow_mut().submit();
        let coordinator = Rc::clone(self);
        let completion_for_response = completion.clone();
        self.callbacks.borrow_mut().insert(
            revision,
            Box::new(move |order, outcome| {
                if !coordinator.accepts_snapshot_audience(audience) {
                    return;
                }
                if Self::order_publishes_snapshot(audience, order) {
                    match outcome {
                        SettingsSaveOutcome::Observed { settings, .. } => {
                            if let Some(completion) = completion_for_response.borrow_mut().take() {
                                completion(SettingsSnapshotResult::Observed(settings.clone()));
                            }
                        }
                        SettingsSaveOutcome::Unavailable { .. } => {
                            if let Some(completion) = completion_for_response.borrow_mut().take() {
                                completion(SettingsSnapshotResult::Unavailable);
                            }
                        }
                    }
                } else {
                    coordinator.queue_snapshot_barrier(audience, completion_for_response);
                }
            }),
        );
        if self
            .requests
            .send(WorkerRequest::Refresh { revision, lane })
            .is_err()
        {
            self.settle(WorkerResponse {
                revision,
                origin: WorkerResponseOrigin::Refresh,
                outcome: worker_failure("Settings worker is unavailable"),
            });
        }
    }

    fn queue_snapshot_barrier(
        self: &Rc<Self>,
        audience: SettingsSnapshotAudience,
        completion: Rc<RefCell<Option<SettingsSnapshotCompletion>>>,
    ) {
        let coordinator = Rc::clone(self);
        self.barrier(move |order, outcome| {
            if !coordinator.accepts_snapshot_audience(audience) {
                return;
            }
            if Self::order_publishes_snapshot(audience, order) {
                match outcome {
                    SettingsSaveOutcome::Observed { settings, .. } => {
                        if let Some(completion) = completion.borrow_mut().take() {
                            completion(SettingsSnapshotResult::Observed(settings.clone()));
                        }
                    }
                    SettingsSaveOutcome::Unavailable { .. } => {
                        if let Some(completion) = completion.borrow_mut().take() {
                            completion(SettingsSnapshotResult::Unavailable);
                        }
                    }
                }
            } else {
                coordinator.queue_snapshot_barrier(audience, completion);
            }
        });
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
                WorkerResponseOrigin::Refresh | WorkerResponseOrigin::Barrier => {
                    match &response.outcome {
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
                    }
                }
            },
            SequenceOrder::Stale => {}
        }
        if response.origin == WorkerResponseOrigin::Save
            && let Some(projection) = self.save_projection.borrow().as_ref()
        {
            projection(order, &response.outcome);
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
    responses: ResponsePublisher,
    io: SettingsWorkerIo,
    initial_snapshot: Option<AppSettings>,
) {
    let io = Arc::new(io);
    let mut snapshot = initial_snapshot;
    while let Some(request) = requests.recv().await {
        match request {
            #[cfg(test)]
            WorkerRequest::Seed(settings) => snapshot = Some(*settings),
            WorkerRequest::Refresh { revision, lane } => {
                let current = snapshot.take();
                let io = Arc::clone(&io);
                let result =
                    souffle_lib::async_runtime::spawn_blocking(move || io.load(lane)).await;
                let outcome = match result {
                    Ok(Ok(settings)) => {
                        snapshot = Some(settings.clone());
                        SettingsSaveOutcome::Observed {
                            settings: Box::new(settings),
                            result: Ok(()),
                        }
                    }
                    Ok(Err(read_error)) => {
                        snapshot = current.clone();
                        match current {
                            Some(settings) => SettingsSaveOutcome::Observed {
                                settings: Box::new(settings),
                                result: Err(SettingsSaveError::NotCommitted {
                                    message: format!(
                                        "Effective Settings refresh failed: {read_error}"
                                    ),
                                }),
                            },
                            None => SettingsSaveOutcome::Unavailable {
                                result: Err(SettingsSaveError::NotCommitted {
                                    message: "Settings could not be refreshed".into(),
                                }),
                                read_error,
                            },
                        }
                    }
                    Err(error) => {
                        snapshot = current.clone();
                        match current {
                            Some(settings) => SettingsSaveOutcome::Observed {
                                settings: Box::new(settings),
                                result: Err(SettingsSaveError::NotCommitted {
                                    message: format!("Settings refresh failed: {error}"),
                                }),
                            },
                            None => worker_failure(&format!("Settings refresh failed: {error}")),
                        }
                    }
                };
                if !responses.send(WorkerResponse {
                    revision,
                    origin: WorkerResponseOrigin::Refresh,
                    outcome,
                }) {
                    break;
                }
            }
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
                if !responses.send(WorkerResponse {
                    revision,
                    origin: WorkerResponseOrigin::Barrier,
                    outcome,
                }) {
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
                if !responses.send(WorkerResponse {
                    revision,
                    origin: WorkerResponseOrigin::Save,
                    outcome,
                }) {
                    break;
                }
            }
        }
    }
}

impl Drop for SettingsIoCoordinator {
    fn drop(&mut self) {
        RESPONSE_TARGETS.with(|targets| {
            targets.borrow_mut().remove(&self.response_target_id);
        });
        self.response_wake_pending.store(false, Ordering::Release);
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
    fn response_wake_reservation_coalesces_until_dispatch_and_drop_is_safe() {
        let pending = AtomicBool::new(false);
        assert!(reserve_response_wake(&pending));
        assert!(!reserve_response_wake(&pending));
        pending.store(false, Ordering::Release);
        assert!(reserve_response_wake(&pending));

        let window = test_window();
        let cache = SettingsCache::with_observed(&window, AppSettings::default());
        let completed = Arc::new(AtomicUsize::new(0));
        let completed_for_save = Arc::clone(&completed);
        let coordinator = SettingsIoCoordinator::with_functions(
            cache,
            || Ok(AppSettings::default()),
            || Ok(AppSettings::default()),
            move |settings| {
                completed_for_save.fetch_add(1, Ordering::SeqCst);
                SettingsSaveOutcome::Observed {
                    settings: Box::new(settings),
                    result: Ok(()),
                }
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let target_id = coordinator.response_target_id;
        coordinator.submit(SettingsSaveLane::General, |_| {}, |_, _| {});
        coordinator.submit(SettingsSaveLane::General, |_| {}, |_, _| {});

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while completed.load(Ordering::SeqCst) < 2 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(RESPONSE_TARGETS.with(|targets| targets.borrow().contains_key(&target_id)));

        drop(coordinator);
        assert!(!RESPONSE_TARGETS.with(|targets| targets.borrow().contains_key(&target_id)));
    }

    #[test]
    fn startup_refresh_populates_cache_and_reports_failure_without_blocking() {
        let window = test_window();
        let expected = AppSettings {
            locale: "fr".into(),
            ..AppSettings::default()
        };
        let cache = SettingsCache::new(&window);
        let coordinator = SettingsIoCoordinator::with_functions(
            cache.clone(),
            {
                let expected = expected.clone();
                move || Ok(expected.clone())
            },
            || unreachable!("startup refresh must not consult ServiceManagement"),
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let observed = Rc::new(RefCell::new(None));
        let observed_for_callback = observed.clone();
        coordinator.load_startup_snapshot(move |result| {
            *observed_for_callback.borrow_mut() = Some(result);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while observed.borrow().is_none() {
            coordinator.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let SettingsSnapshotResult::Observed(settings) =
            observed.borrow_mut().take().expect("startup result")
        else {
            panic!("startup snapshot should be observed")
        };
        assert_eq!(settings.locale, "fr");
        assert_eq!(cache.known_snapshot().unwrap().locale, "fr");

        let failed_cache = SettingsCache::new(&window);
        let failed = SettingsIoCoordinator::with_functions(
            failed_cache.clone(),
            || Err("injected startup read failure".into()),
            || unreachable!("startup refresh must not consult ServiceManagement"),
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        let unavailable = Rc::new(Cell::new(false));
        let unavailable_for_callback = unavailable.clone();
        failed.load_startup_snapshot(move |result| match result {
            SettingsSnapshotResult::Observed(_) => panic!("unexpected startup snapshot"),
            SettingsSnapshotResult::Unavailable => unavailable_for_callback.set(true),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !unavailable.get() {
            failed.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(failed_cache.known_snapshot().is_none());
        assert!(
            window
                .get_settings_save_error()
                .contains("injected startup read failure")
        );
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
    fn open_refresh_waits_for_a_preexisting_save_and_preserves_it_next_time() {
        let window = test_window();
        let durable = Arc::new(Mutex::new(AppSettings::default()));
        let cache = SettingsCache::with_observed(&window, durable.lock().unwrap().clone());
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let general_load_state = Arc::clone(&durable);
        let effective_load_state = Arc::clone(&durable);
        let general_save_state = Arc::clone(&durable);
        let autostart_save_state = Arc::clone(&durable);
        let save_count = Arc::new(AtomicUsize::new(0));
        let general_save_count = Arc::clone(&save_count);
        let coordinator = SettingsIoCoordinator::with_io(
            cache,
            SettingsWorkerIo {
                load_general: Arc::new(move || Ok(general_load_state.lock().unwrap().clone())),
                load_autostart: Arc::new(move || Ok(effective_load_state.lock().unwrap().clone())),
                save_general: Arc::new(move |settings| {
                    if general_save_count.fetch_add(1, Ordering::SeqCst) == 0 {
                        started_tx.send(()).unwrap();
                        release_rx.lock().unwrap().recv().unwrap();
                    }
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
        started_rx.recv().unwrap();
        let token = coordinator.begin_open();

        let dependent_started = Rc::new(Cell::new(false));
        let dependent_settings = Rc::new(RefCell::new(None));
        let started = dependent_started.clone();
        let observed = dependent_settings.clone();
        coordinator.load_effective_snapshot(token, move |settings| {
            started.set(true);
            *observed.borrow_mut() = Some(settings);
        });
        coordinator.drain_for_test();
        assert!(!dependent_started.get());
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !dependent_started.get() {
            coordinator.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            dependent_settings
                .borrow()
                .as_ref()
                .expect("latest settings snapshot")
                .locale,
            "fr"
        );

        coordinator.submit(
            SettingsSaveLane::General,
            |settings| settings.paste_delay_ms = 225,
            |_, _| {},
        );
        while !coordinator.is_idle_for_test() {
            coordinator.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let durable = durable.lock().unwrap();
        assert_eq!(durable.locale, "fr");
        assert_eq!(durable.paste_delay_ms, 225);
    }

    #[test]
    fn failed_effective_refresh_keeps_the_post_save_snapshot_and_opens() {
        let window = test_window();
        let initial = AppSettings::default();
        let cache = SettingsCache::with_observed(&window, initial.clone());
        let coordinator = SettingsIoCoordinator::with_functions(
            cache,
            move || Ok(initial.clone()),
            || Err("injected ServiceManagement read failure".into()),
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
            |settings| SettingsSaveOutcome::Observed {
                settings: Box::new(settings),
                result: Ok(()),
            },
        );
        coordinator.submit(
            SettingsSaveLane::General,
            |settings| settings.locale = "fr".into(),
            |_, _| {},
        );
        let token = coordinator.begin_open();
        let opened = Rc::new(RefCell::new(None));
        let published = opened.clone();
        coordinator.load_effective_snapshot(token, move |settings| {
            *published.borrow_mut() = Some(settings);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while opened.borrow().is_none() {
            coordinator.drain_for_test();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(opened.borrow().as_ref().unwrap().locale, "fr");
        assert!(
            window
                .get_settings_save_error()
                .contains("injected ServiceManagement read failure")
        );
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
