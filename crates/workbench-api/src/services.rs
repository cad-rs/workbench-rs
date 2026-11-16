//! The platform service aggregate and the capability facade visible to workbenches
//! (design.md §4.1 Platform).

use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

use crate::command::{CommandArgs, CommandResult, CtxSnapshot};
use crate::documents::DocumentService;
use crate::ids::DocumentId;
use crate::log::LogStore;
use crate::settings::Settings;
use crate::storage::{Location, Storage};
use crate::tasks::{TaskEventRx, TaskManager};
use crate::workspace::WorkspaceState;

/// Runtime statistics (for the diagnostics panel/automated tests).
#[derive(Clone, Debug, Default)]
pub struct RuntimeStats {
    /// Rendered frames (reported every frame by the GUI frontend; always 0 when headless).
    pub frames: u64,
    /// Exponential moving average of frame time (milliseconds).
    pub avg_frame_ms: f32,
    /// Time of the last frame (milliseconds).
    pub last_frame_ms: f32,
    /// GUI backend label (written when the product is assembled, e.g. "egui"/"gpui").
    pub backend: String,
    /// Number of autosaves.
    pub autosaves: u64,
}

/// A service's per-tick callback (the platform's own tick, driven by the
/// frontend's timer — see `AppRuntime::frame_tick`).
///
/// This is the extension point for background pumps: a service whose work
/// arrives on a channel (a collaboration room's socket, a feed) drains it
/// here and returns whether the UI needs a redraw. The frontend runs the
/// tick on its own timer precisely so such a pump cannot stall an idle UI,
/// and so an idle pump costs no frames at all.
pub type TickHook = Box<dyn FnMut(&mut ServiceHost) -> bool + Send>;

/// Identifies a registered [`TickHook`] (for `ServiceHost::remove_tick_hook`).
pub type TickHookId = u64;

/// A document-command sink: a collaboration session's apply path.
///
/// The domain layer's edit choke point hands the command it built to the
/// installed sink; a session applies it (minting the op) instead of the
/// document's plain apply. The command is the domain's own type, boxed — the
/// same type erasure the platform already uses for `DocumentContent`, so the
/// platform keeps no domain schema. The sink downcasts it.
pub trait CommandSink: Send {
    /// Apply one domain command. `Ok(false)` = this document is not the
    /// session's, so the caller applies it the plain way.
    fn apply(
        &mut self,
        host: &mut ServiceHost,
        id: DocumentId,
        label: &str,
        command: Box<dyn Any + Send>,
    ) -> Result<bool, String>;

    /// Undo one step for `id`. `Ok(None)` = not handled, the caller falls back
    /// to the platform's own history.
    fn undo(&mut self, host: &mut ServiceHost, id: DocumentId)
        -> Result<Option<String>, String>;

    /// Redo one step for `id`; the same `Ok(None)` fallback contract.
    fn redo(&mut self, host: &mut ServiceHost, id: DocumentId)
        -> Result<Option<String>, String>;

    /// Whether the session would take `id`'s undo step (the ribbon's and the
    /// hotkey's enabled state).
    fn can_undo(&self, host: &ServiceHost, id: DocumentId) -> bool;

    /// Whether the session would take `id`'s redo step.
    fn can_redo(&self, host: &ServiceHost, id: DocumentId) -> bool;
}

/// A plugin registry entry (for the diagnostics/management panels).
#[derive(Clone, Debug)]
pub struct PluginEntry {
    pub id: String,
    /// Loaded / Rejected / Disabled
    pub state: &'static str,
    /// Why it was Rejected/Disabled.
    pub detail: Option<String>,
    /// The feature the plugin requires (manifest `requires`); None = no requirement.
    pub requires: Option<String>,
}

/// The platform service aggregate. Owned by the UI thread.
pub struct ServiceHost {
    pub docs: DocumentService,
    /// Where user files live; reach it through [`crate::CommandCtx::read_file`]
    /// and its siblings, which run the I/O off the UI thread.
    pub storage: Storage,
    pub tasks: TaskManager,
    pub ws: WorkspaceState,
    pub log: LogStore,
    pub settings: Settings,
    /// Registered setting specs (platform built-ins + plugin settings),
    /// filled at build time; the settings dialog renders from these and
    /// `app.settings.set` validates against them.
    pub setting_specs: Vec<crate::settings::SettingSpec>,
    /// Merged locale tables (platform built-ins + plugin contributions),
    /// filled at build time; display sites go through [`ServiceHost::tr`].
    pub localizer: crate::i18n::Localizer,
    /// Injected account capability (Phase 3); None = no login surface.
    /// What decides gated features; None = everything is allowed.
    pub entitlements: Option<std::sync::Arc<dyn crate::entitlements::Entitlements>>,
    pub account: Option<std::sync::Arc<dyn crate::account::AccountService>>,
    /// In-flight device-login poll thread: phase updates (drained each frame
    /// into `ws.account` by the runtime) and the cancel flag.
    pub account_login_rx: Option<std::sync::mpsc::Receiver<crate::workspace::AccountPhase>>,
    pub account_login_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub stats: RuntimeStats,
    /// Mirror of the plugin registry (filled at build time, updated when a plugin is disabled at
    /// runtime).
    pub plugins: Vec<PluginEntry>,
    /// When the platform started.
    pub started: web_time::Instant,
    extras: std::collections::HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    task_rx: TaskEventRx,
    /// Per-tick callbacks ([`TickHook`]) with the id they were registered
    /// under; run by the platform's tick.
    tick_hooks: Vec<(TickHookId, TickHook)>,
    next_tick_hook_id: TickHookId,
    /// The installed collaboration session's command sink (see [`CommandSink`]).
    command_sink: Option<Box<dyn CommandSink>>,
}

impl ServiceHost {
    pub fn new(
        tx: std::sync::mpsc::Sender<crate::tasks::TaskEvent>,
        task_rx: TaskEventRx,
        default_mode: impl Into<String>,
    ) -> Self {
        Self {
            docs: DocumentService::new(),
            storage: Storage::default(),
            tasks: TaskManager::new(tx),
            ws: WorkspaceState::new(default_mode),
            log: LogStore::new(1000),
            settings: Settings::in_memory(),
            setting_specs: Vec::new(),
            localizer: crate::i18n::Localizer::new(),
            entitlements: None,
            account: None,
            account_login_rx: None,
            account_login_cancel: None,
            stats: RuntimeStats::default(),
            plugins: Vec::new(),
            started: web_time::Instant::now(),
            extras: std::collections::HashMap::new(),
            task_rx,
            tick_hooks: Vec::new(),
            next_tick_hook_id: 0,
            command_sink: None,
        }
    }

    /// Report a rendered frame (called every frame by the GUI frontend).
    pub fn record_frame(&mut self, dt_ms: f32) {
        let s = &mut self.stats;
        s.frames += 1;
        s.last_frame_ms = dt_ms;
        s.avg_frame_ms = if s.avg_frame_ms <= 0.0 {
            dt_ms
        } else {
            s.avg_frame_ms * 0.9 + dt_ms * 0.1
        };
    }

    /// Platform uptime (seconds).
    /// Whether the user may use `feature` (see [`crate::Entitlements`]).
    pub fn allows(&self, feature: &str) -> bool {
        self.entitlements.as_ref().map_or(true, |entitlements| entitlements.allows(feature))
    }

    /// Why the user may not use `feature`; None when they may.
    pub fn refusal(&self, feature: &str) -> Option<String> {
        crate::entitlements::refusal(self.entitlements.as_ref(), feature)
    }

    pub fn uptime_secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// The current locale (the `ui.locale` setting, `en` when unset).
    pub fn locale(&self) -> String {
        self.settings.get_str("ui.locale", "en")
    }

    /// Translate a UI string: look it up for the current locale, fall back
    /// to `en`, then to the key itself, so an untranslated string shows as
    /// written instead of leaving a hole in the UI.
    pub fn tr(&self, key: &str) -> String {
        let locale = self.locale();
        self.localizer
            .translate(&locale, key)
            .unwrap_or(key)
            .to_string()
    }

    // ---- Account (Phase 3: device flow, polled on a background thread) ----

    /// Start signing in. Phase updates arrive via the channel and land in `ws.account` each
    /// tick (drained by the runtime). The desktop's network calls block, so they get a thread;
    /// on the web they are written not to.
    pub fn begin_account_login(&mut self) -> Result<(), String> {
        let Some(service) = self.account.clone() else {
            return Err("The product has no account service configured".to_string());
        };
        if self.account_login_rx.is_some() {
            return Err("A sign-in is already in progress".to_string());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.account_login_rx = Some(rx);
        self.account_login_cancel = Some(cancel.clone());
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || async_io::block_on(sign_in(service, tx, cancel)));
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(sign_in(service, tx, cancel));
        Ok(())
    }

    /// Abort an in-flight device login (the poll thread exits on the flag;
    /// the phase returns to LoggedOut).
    pub fn cancel_account_login(&mut self) {
        if let Some(flag) = self.account_login_cancel.take() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.account_login_rx = None;
        self.ws.account = crate::workspace::AccountPhase::LoggedOut;
    }

    /// End the session (Phase 3).
    pub fn account_logout(&mut self) {
        self.cancel_account_login();
        if let Some(service) = &self.account {
            service.logout();
        }
        self.ws.account = crate::workspace::AccountPhase::LoggedOut;
        self.log.info("Signed out");
    }

    /// The locked task event receiver (the core's frame_tick pumps events from it).
    pub fn task_events(&self) -> Arc<Mutex<std::sync::mpsc::Receiver<crate::tasks::TaskEvent>>> {
        self.task_rx.clone()
    }

    // ---- Workbench/plugin extension services (design.md §8.1 "service" contributions) ----

    pub fn set_service<T: Any + Send + Sync>(&mut self, value: T) {
        self.extras.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn service<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extras.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    pub fn service_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.extras.get_mut(&TypeId::of::<T>()).and_then(|b| b.downcast_mut::<T>())
    }

    /// Moved in as-is during assembly (from Registry.take_services).
    pub fn insert_raw(&mut self, ty: TypeId, value: Box<dyn Any + Send + Sync>) {
        self.extras.insert(ty, value);
    }

    // ---- Per-tick hooks (the platform's own tick, not a view's render) ----

    /// Registers a [`TickHook`], returning its id.
    ///
    /// Hooks run once per platform tick (`AppRuntime::frame_tick`, driven by
    /// the frontend's timer). A hook that keeps no state of its own can be
    /// registered once and left there; one bound to a session removes itself
    /// with [`Self::remove_tick_hook`] when that session ends.
    pub fn on_tick(&mut self, hook: TickHook) -> TickHookId {
        let id = self.next_tick_hook_id;
        self.next_tick_hook_id += 1;
        self.tick_hooks.push((id, hook));
        id
    }

    /// Removes a tick hook by id (a no-op when it is already gone).
    pub fn remove_tick_hook(&mut self, id: TickHookId) {
        self.tick_hooks.retain(|(hook_id, _)| *hook_id != id);
    }

    /// Runs every registered hook; returns whether any reported a change.
    pub fn run_tick_hooks(&mut self) -> bool {
        let mut hooks = std::mem::take(&mut self.tick_hooks);
        let mut changed = false;
        for (_, hook) in hooks.iter_mut() {
            changed |= hook(self);
        }
        self.tick_hooks = hooks;
        changed
    }

    // ---- Document-command sink (a collaboration session's apply path) ----

    /// Installs / clears the command sink. A session installs it when it
    /// begins and clears it when it ends (a stale sink would keep minting
    /// ops for a room nobody is in).
    pub fn set_command_sink(&mut self, sink: Option<Box<dyn CommandSink>>) {
        self.command_sink = sink;
    }

    /// Whether a session currently takes documents' commands.
    pub fn has_command_sink(&self) -> bool {
        self.command_sink.is_some()
    }

    /// Runs `f` with the installed sink and puts it back afterwards — even
    /// when `f` fails, so one refused edit cannot end the session.
    ///
    /// The take/put-back is forced by the sink's contract: it is handed
    /// `&mut ServiceHost`, which it cannot be borrowing at the same time.
    /// `None` = no sink installed.
    fn with_command_sink<T>(
        &mut self,
        f: impl FnOnce(&mut dyn CommandSink, &mut ServiceHost) -> T,
    ) -> Option<T> {
        let mut sink = self.command_sink.take()?;
        let outcome = f(&mut *sink, self);
        self.command_sink = Some(sink);
        Some(outcome)
    }

    /// The installed sink's answer for one undo/redo step, mapped onto the
    /// platform's own history when the sink has no step.
    fn settle_step(
        &mut self,
        outcome: Result<Option<String>, String>,
        plain: impl FnOnce(&mut Self) -> Result<String, String>,
    ) -> Result<String, String> {
        match outcome {
            Ok(Some(label)) => Ok(label),
            Ok(None) => plain(self),
            Err(error) => Err(error),
        }
    }

    /// Hands one domain command to the installed sink. `Ok(false)` = no sink
    /// (or the sink says the document is not its session's), so the caller
    /// applies the command the plain way.
    pub fn apply_document_command(
        &mut self,
        id: DocumentId,
        label: &str,
        command: Box<dyn Any + Send>,
    ) -> Result<bool, String> {
        self.with_command_sink(|sink, host| sink.apply(host, id, label, command))
            .unwrap_or(Ok(false))
    }

    /// Undo one step for `id`: the session's step when it handles it,
    /// otherwise the platform's own history.
    pub fn undo_document(&mut self, id: DocumentId) -> Result<String, String> {
        let Some(outcome) = self.with_command_sink(|sink, host| sink.undo(host, id)) else {
            return self.docs.undo(id);
        };
        self.settle_step(outcome, |host| host.docs.undo(id))
    }

    /// Redo one step for `id`; the same contract as [`Self::undo_document`].
    pub fn redo_document(&mut self, id: DocumentId) -> Result<String, String> {
        let Some(outcome) = self.with_command_sink(|sink, host| sink.redo(host, id)) else {
            return self.docs.redo(id);
        };
        self.settle_step(outcome, |host| host.docs.redo(id))
    }

    /// Whether a session would take `id`'s undo (the ribbon's enabled state).
    pub fn can_undo_document(&self, id: DocumentId) -> bool {
        match self.command_sink.as_ref() {
            Some(sink) => sink.can_undo(self, id),
            None => self.docs.can_undo(id),
        }
    }

    /// Whether a session would take `id`'s redo.
    pub fn can_redo_document(&self, id: DocumentId) -> bool {
        match self.command_sink.as_ref() {
            Some(sink) => sink.can_redo(self, id),
            None => self.docs.can_redo(id),
        }
    }
}

/// The platform capabilities visible to workbenches (Workbench modules, script plugin hosts,
/// views).
///
/// Implemented by the core's AppRuntime; CommandCtx / ViewCtx reach the platform through this
/// trait, so command and view code only ever faces a stable API (design.md §15).
pub trait AppServices {
    /// The platform service aggregate.
    fn host(&mut self) -> &mut ServiceHost;
    /// A snapshot of the current context.
    fn snapshot(&self) -> CtxSnapshot;
    /// Run a command by stable ID (the single entry point).
    fn dispatch(&mut self, command: &str, args: &CommandArgs) -> CommandResult;
    /// Open a central view; with `bind_document` true it binds the active document.
    fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String>;
    /// Close the active central tab.
    fn close_active_view(&mut self) -> Result<(), String>;
    /// Switch the work mode.
    fn set_work_mode(&mut self, mode: &str) -> Result<(), String>;
    /// Show/hide a panel.
    fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String>;
    /// Disable a plugin at runtime: removes its commands and Ribbon contributions (phase 5,
    /// §16.2). Returns the IDs of the removed commands.
    fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String>;
    /// Run an autosave right now and return the number of documents saved (phase 5).
    fn autosave_now(&mut self) -> Result<usize, String>;
    /// What support needs to see, as plain text the user can read before sending it.
    fn diagnostics_report(&self) -> String;
    /// Ask the user for a file (non-blocking): an open dialog, or a save
    /// dialog suggesting `default_name`. Storage the in-app picker browses
    /// (the browser's, cloud stores) gets that picker; the local file system
    /// alone gets the OS's own dialog, run off the UI thread (its modal loop
    /// would wedge gpui's render loop). Either way `action` runs on the UI
    /// thread with the pick (`None`: cancelled), through
    /// [`Self::finish_file_dialog`]. One dialog at a time.
    fn request_file_dialog(
        &mut self,
        extensions: Vec<String>,
        default_name: Option<String>,
        action: Box<dyn FnOnce(&mut dyn AppServices, Option<Location>)>,
    );
    /// Close the file dialog in flight with the user's pick (`None`:
    /// cancelled) and run its action.
    fn finish_file_dialog(&mut self, picked: Option<Location>);
    /// Run `io` off the UI thread (file and network I/O, waiting on the
    /// browser); what it resolves to runs on the UI thread on a later frame
    /// tick. [`crate::CommandCtx::read_file`] and its siblings build on this.
    fn spawn_io(&mut self, io: IoTask);
    /// Fetch the latest entitlements now and tell the user how it went (see
    /// [`crate::Entitlements::refresh`]).
    fn refresh_entitlements(&mut self);
    /// A fetch of the latest entitlements ended.
    fn finish_entitlements_refresh(&mut self, result: Result<(), String>);
    /// Open the page where the user buys or changes a plan in the browser.
    /// When they come back to the app the platform keeps checking for a while,
    /// since the payment can take a moment to reach the product's server.
    fn open_purchase_page(&mut self, url: &str);
}

/// One sign-in from start to finish, reporting each phase.
async fn sign_in(
    service: Arc<dyn crate::account::AccountService>,
    tx: std::sync::mpsc::Sender<crate::workspace::AccountPhase>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) {
    use crate::account::{PollResult, SLOW_DOWN_INTERVAL_SECS};
    use crate::workspace::AccountPhase;

    let request = match service.begin_login() {
        Ok(request) => request,
        Err(e) => {
            let _ = tx.send(AccountPhase::Failed(format!("Could not start signing in: {e}")));
            return;
        }
    };
    let mut interval = request.interval.max(1);
    let (started, expires_in) = (web_time::Instant::now(), request.expires_in);
    let _ = tx.send(AccountPhase::Pending(request));
    loop {
        crate::asyncrt::sleep(std::time::Duration::from_secs(interval)).await;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let answer = if started.elapsed().as_secs() >= expires_in {
            Ok(PollResult::Expired)
        } else {
            service.poll()
        };
        let end = match answer {
            Ok(PollResult::Pending) => continue,
            Ok(PollResult::SlowDown) => {
                interval += SLOW_DOWN_INTERVAL_SECS;
                continue;
            }
            Ok(PollResult::Success(user)) => AccountPhase::LoggedIn(user),
            Ok(PollResult::Denied) => AccountPhase::Failed("Sign-in was denied".to_string()),
            Ok(PollResult::Expired) => {
                AccountPhase::Failed("The sign-in request has expired".to_string())
            }
            Err(e) => AccountPhase::Failed(format!("Sign-in failed: {e}")),
        };
        let _ = tx.send(end);
        return;
    }
}

/// Where crash reports go once the user has agreed to send them
/// (`WorkbenchAppBuilder::with_crash_reporter`). The product injects it; the
/// framework binds to no service.
pub trait CrashReporter: Send + Sync {
    /// Send the report called `name`. Runs off the UI thread and may block on
    /// the network. Errors are user-readable.
    fn send(&self, name: &str, report: &str) -> Result<(), String>;
}

/// What to do on the UI thread once some I/O has finished.
pub type UiThen = Box<dyn FnOnce(&mut dyn AppServices) + Send>;

/// I/O for [`AppServices::spawn_io`].
pub type IoTask = crate::asyncrt::BoxFuture<UiThen>;

/// What to do on the UI thread with a file operation's result.
pub type FileThen<T> = Box<dyn FnOnce(&mut dyn AppServices, Result<T, String>) + Send>;
