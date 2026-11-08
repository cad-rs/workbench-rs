//! The platform runtime: product assembly, command execution, the task event pump, context
//! snapshots and layout persistence.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use workbench_api as api;
use api::{
    ComboState, CommandArgs, CommandCtx, CommandExecution, CommandInvocation, CommandKind,
    CommandResult, CtxSnapshot, DocSummary, Hotkey, InvocationRecord, InvocationStatus, PanelId,
    Registry, ServiceHost, TaskEvent, ViewTypeId, Workbench,
};

use crate::executor::AsyncExecutor;
use crate::platform_cmds;

/// Capacity of the invocation history ring buffer.
const INVOCATION_HISTORY: usize = 100;

/// Autosave and crash recovery run on the desktop only: a web page has no
/// clean exit to tell a crash from a closed tab, and its state store
/// (localStorage) holds little, and only text.
const AUTOSAVE: bool = cfg!(not(target_arch = "wasm32"));
/// Where autosaves live in the state store.
pub(crate) const AUTOSAVE_DIR: &str = "autosave/";

/// The state-store key of a document's autosave without its extension
/// (`autosave/<title>_`): the content is `<stem>.<ext>`, its metadata
/// `<stem>.json`. The title is cut down to characters safe in a file name.
pub(crate) fn autosave_stem(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{AUTOSAVE_DIR}{safe}_")
}
/// The newest crash report already announced at a launch.
const CRASH_REPORTS_SEEN: &str = "crash_reports_seen";
/// Written on a clean exit; missing at startup means the last session crashed.
const CLEAN_EXIT: &str = "autosave/session.clean";
/// The settings' key in the state store.
pub(crate) const SETTINGS: &str = "settings.json";

/// The platform runtime. Owned by the UI thread; the frontend calls [`AppRuntime::frame_tick`]
/// on its own timer (decoupled from view rendering: gpui only redraws on invalidation, so a
/// render-driven pump stalls while the UI is idle).
pub struct AppRuntime {
    registry: Registry,
    pub host: ServiceHost,
    product_id: String,
    product_name: String,
    /// Product identity and branding (product.toml; the builder's defaults when none is given).
    profile: api::ProductProfile,
    snapshot: CtxSnapshot,
    /// The app's own state: settings, layout, autosaves, the clean-exit marker.
    state: Arc<dyn api::StateStore>,
    /// The layout's key in `state`.
    layout_key: String,
    /// The layout as last loaded or saved, so an unchanged one isn't rewritten.
    saved_layout: Option<String>,
    /// Where crash reports go if the user agrees.
    crash_reporter: Option<Arc<dyn api::CrashReporter>>,
    /// Seconds accumulated since the last autosave.
    autosave_accum: f32,
    dispatch_depth: u32,
    /// The shared async executor (phase 2: what Async commands run on).
    executor: AsyncExecutor,
    shutdown: Arc<AtomicBool>,
    /// Command invocation history (queryable, for logs/automated tests/diagnostics,
    /// design.md §16.4).
    invocations: VecDeque<InvocationRecord>,
    invocation_seq: u64,
    /// The file dialog in flight (the in-app picker or the OS's dialog); both
    /// end in [`api::AppServices::finish_file_dialog`].
    dialog_in_flight: Option<FileDialogRequest>,
    /// Finished I/O's continuations ([`api::AppServices::spawn_io`]), run by
    /// `frame_tick`.
    io_done_tx: std::sync::mpsc::Sender<api::UiThen>,
    io_done_rx: std::sync::mpsc::Receiver<api::UiThen>,
    /// I/O started and not yet drained.
    io_pending: usize,
}

/// A UI action waiting for the user to pick a file
/// ([`api::AppServices::request_file_dialog`]).
pub(crate) struct FileDialogRequest {
    /// File filter extensions (empty = no filter).
    pub extensions: Vec<String>,
    /// Default file name (save dialog); `None` for an open dialog.
    pub default_name: Option<String>,
    /// The follow-up run on the UI thread once the choice is in (`None` = cancelled).
    pub action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
}

impl AppRuntime {
    // ---- Queries (for frontend rendering) ----

    pub fn product_id(&self) -> &str {
        &self.product_id
    }

    pub fn product_name(&self) -> &str {
        &self.product_name
    }

    /// Product identity and branding (the one source for the window title, ribbon logo and
    /// About dialog).
    pub fn profile(&self) -> &api::ProductProfile {
        &self.profile
    }

    pub fn snapshot(&self) -> &CtxSnapshot {
        &self.snapshot
    }

    pub fn command_title(&self, id: &str) -> Option<String> {
        self.registry.command(id).map(|c| c.title.clone())
    }

    /// The command's icon asset path (None when unset; the UI falls back to plain text).
    pub fn command_icon(&self, id: &str) -> Option<String> {
        self.registry.command(id).and_then(|c| c.icon.clone())
    }

    pub fn command_hotkey_label(&self, id: &str) -> Option<String> {
        self.registry.command(id).and_then(|c| c.hotkey).map(|h| h.to_string())
    }

    /// Whether the command is currently enabled (per the snapshot the last frame_tick made).
    pub fn is_enabled(&self, id: &str) -> bool {
        match self.registry.command(id) {
            Some(def) => match &def.enabled {
                Some(f) => f(&self.snapshot),
                None => true,
            },
            None => false,
        }
    }

    /// Whether a toggle command is currently checked (ribbon-bar.md §4.2: the checked state is
    /// a query on the command system, the ribbon keeps no toggle state of its own; always false
    /// when unregistered or without a checked condition).
    pub fn is_checked(&self, id: &str) -> bool {
        match self.registry.command(id) {
            Some(def) => match &def.checked {
                Some(f) => f(&self.snapshot),
                None => false,
            },
            None => false,
        }
    }

    /// A combo command's current state (current value + options, ribbon-bar.md §4.2: queried
    /// every frame; always None when unregistered or without a combo query).
    pub fn combo_state(&self, id: &str) -> Option<ComboState> {
        match self.registry.command(id) {
            Some(def) => def.combo.as_ref().and_then(|f| f(&self.snapshot)),
            None => None,
        }
    }

    /// The last N command invocation records (design.md §16.4: state automated tests can
    /// query).
    pub fn recent_invocations(&self) -> impl Iterator<Item = &InvocationRecord> {
        self.invocations.iter().rev()
    }

    /// IDs of the plugins that loaded (phase 3).
    pub fn plugins_loaded(&self) -> &[String] {
        &self.registry.plugins_loaded
    }

    /// Rejected/failed plugins and why (phase 3, acceptance #19/#20 diagnostics).
    pub fn plugins_rejected(&self) -> &[(String, String)] {
        &self.registry.plugins_rejected
    }

    /// The unified command entry point (tracked, design.md §10.1): returns the immediate
    /// result + the async task handle.
    pub fn execute_command_tracked(&mut self, id: &str, args: &CommandArgs) -> CommandInvocation {
        let seq = self.invocation_seq + 1;
        let result = self.execute_command(id, args);
        let record = self
            .invocations
            .iter()
            .find(|r| r.seq == seq)
            .cloned()
            .expect("the invocation record is registered");
        let task_id = record.task_id;
        CommandInvocation {
            record,
            result,
            task_id,
        }
    }

    /// All registered hotkeys (the frontend dispatches key presses).
    pub fn hotkeys(&self) -> Vec<(Hotkey, String)> {
        self.registry
            .commands()
            .filter_map(|c| c.hotkey.map(|h| (h, c.id.0.clone())))
            .collect()
    }

    /// Where the app's own state lives (settings, layout, autosaves).
    pub fn state(&self) -> &Arc<dyn api::StateStore> {
        &self.state
    }

    // ---- Frame driving ----

    /// Ask the user for a file (see [`api::AppServices::request_file_dialog`]):
    /// the in-app picker over the storage it browses (`ws.file_picker`, drawn
    /// by the frontend), else the OS's dialog for the local file system, run
    /// as I/O off the UI thread.
    pub(crate) fn request_dialog(&mut self, request: FileDialogRequest) {
        use api::AppServices as _;
        if self.dialog_in_flight.is_some() {
            self.host.log.warn("A dialog is already open; ignoring the new open/save request");
            return;
        }
        let picker = api::FilePicker::new(
            request.default_name.clone(),
            request.extensions.clone(),
            self.host.storage.in_app_sources(),
        );
        if let Some(picker) = picker {
            self.dialog_in_flight = Some(request);
            self.host.ws.file_picker = Some(picker);
            crate::file_picker::list(&mut api::CommandCtx::new(self));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.host.storage.contains(api::storage::LOCAL) {
            let os_dialog =
                crate::dialog::pick(request.extensions.clone(), request.default_name.clone());
            self.dialog_in_flight = Some(request);
            self.spawn_io(Box::pin(async move {
                let picked = os_dialog.await.map(api::Location::local);
                Box::new(move |app: &mut dyn api::AppServices| app.finish_file_dialog(picked))
                    as api::UiThen
            }));
            return;
        }
        self.dialog_in_flight = Some(request);
        self.host.log.warn("No file storage is available");
        self.finish_file_dialog(None);
    }

    /// I/O started with [`api::AppServices::spawn_io`] whose continuation
    /// has not run yet.
    pub fn pending_io(&self) -> usize {
        self.io_pending
    }

    /// Run the continuations of finished I/O. Returns whether any ran (the
    /// UI needs a redraw).
    fn drain_io(&mut self) -> bool {
        let mut ran = false;
        while let Ok(then) = self.io_done_rx.try_recv() {
            self.io_pending -= 1;
            then(self);
            ran = true;
        }
        ran
    }

    // ---- Account sign-in (Phase 3: device flow) ----
    // The login state machine lives on the ServiceHost (commands reach it via
    // ctx.host()); the runtime only drains phase updates each frame.

    /// Drain pending login-phase updates into `ws.account` (once per tick).
    /// Returns whether any phase update was consumed (the UI needs a redraw).
    fn drain_account_login(&mut self) -> bool {
        let Some(rx) = self.host.account_login_rx.as_ref() else {
            return false;
        };
        let mut updates = Vec::new();
        while let Ok(phase) = rx.try_recv() {
            updates.push(phase);
        }
        if updates.is_empty() {
            return false;
        }
        // A terminal phase (LoggedIn / Failed) closes the flow: drop the
        // channel so the next login can start fresh.
        let closed = updates
            .last()
            .is_some_and(|p| !matches!(p, api::AccountPhase::Pending(_)));
        if closed {
            self.host.account_login_rx = None;
            self.host.account_login_cancel = None;
        }
        if let Some(phase) = updates.pop() {
            if let api::AccountPhase::LoggedIn(user) = &phase {
                self.host.log.info(format!("Signed in as {}", user.name));
            }
            self.host.ws.account = phase;
        }
        true
    }

    /// The platform tick: pumps task events, times autosave, syncs the active document/tab
    /// titles and refreshes the context snapshot.
    ///
    /// Returns `true` when this tick changed UI-visible state (task events, a dialog result,
    /// file I/O results, a sign-in phase, an autosave, the active document), so the frontend
    /// schedules a redraw. The frontend drives this on its own timer rather than in view
    /// rendering — gpui only redraws on invalidation, so a render-driven pump stalls while the
    /// UI is idle and task results sit in the channel unclaimed (the diagnostics panel's
    /// background task count then lags).
    pub fn frame_tick(&mut self, dt_secs: f32) -> bool {
        let mut changed = self.drain_io();
        changed |= self.drain_account_login();
        changed |= self.pump_task_events();
        // Autosave (phase 5: §18 production extensions)
        let interval = self
            .host
            .settings
            .get_f64("autosave.interval_secs", 30.0)
            .max(5.0) as f32;
        self.autosave_accum += dt_secs.max(0.0);
        if AUTOSAVE && self.autosave_accum >= interval {
            self.autosave_accum = 0.0;
            match self.autosave_now() {
                Ok(saved) => changed |= saved > 0,
                Err(e) => {
                    self.host.log.warn(format!("Autosave failed: {e}"));
                    changed = true;
                }
            }
        }
        // The active document follows the active tab's binding
        if let Some(tab) = self.host.ws.active_tab() {
            if tab.document.is_some() {
                let doc = tab.document;
                let before = self.host.docs.active_id();
                self.host.docs.set_active(doc);
                changed |= self.host.docs.active_id() != before;
            }
        }
        // Sync the titles of document-bound tabs (including the unsaved marker)
        let bindings: Vec<(api::TabInstanceId, Option<api::DocumentId>)> = self
            .host
            .ws
            .tabs
            .iter()
            .map(|t| (t.instance_id, t.document))
            .collect();
        for (instance_id, doc) in bindings {
            if let Some(did) = doc {
                if let Some(d) = self.host.docs.get(did) {
                    let title = format!("{}{}", d.title, if d.dirty { " •" } else { "" });
                    if let Some(slot) = self
                        .host
                        .ws
                        .tabs
                        .iter_mut()
                        .find(|t| t.instance_id == instance_id)
                    {
                        changed |= slot.title != title;
                        slot.title = title;
                    }
                } else {
                    // The bound document is gone: unbind (the close flow normally prevents
                    // this; defensive)
                    if let Some(slot) = self
                        .host
                        .ws
                        .tabs
                        .iter_mut()
                        .find(|t| t.instance_id == instance_id)
                    {
                        if slot.document.is_some() {
                            slot.document = None;
                            changed = true;
                        }
                    }
                }
            }
        }
        self.snapshot = self.make_snapshot();
        // Keep scheduling frames while tasks run (progress animation); `changed` is true on
        // the tick that takes a task's terminal event, so the panel drops the "running" count
        // back to 0.
        changed |= self.host.tasks.summary().0 > 0;
        changed
    }

    // ---- Autosave and crash recovery (phase 5: §18) ----
    //
    // Autosaves are pairs of entries under `autosave/` in the state store: the content
    // `<title>_.<ext>` and the metadata `<title>_.json`; a clean exit writes
    // `autosave/session.clean`.

    /// Serialize every dirty document into the autosave area now (`autosave/<title>_.<ext>`).
    /// Leaves the documents' dirty/location state alone — the user's save flow is unaffected.
    pub fn autosave_now(&mut self) -> Result<usize, String> {
        if !AUTOSAVE {
            return Err("Autosave is not supported on the web yet".to_string());
        }
        let docs = &self.host.docs;
        let dirty: Vec<_> = docs
            .docs()
            .filter(|d| d.dirty)
            .map(|d| {
                let ext = docs
                    .type_of(&d.type_id)
                    .and_then(|t| t.extensions.first().cloned())
                    .unwrap_or_else(|| "dat".to_string());
                (d.id, d.title.clone(), d.type_id.clone(), ext)
            })
            .collect();
        let mut saved = 0;
        for (id, title, type_id, ext) in dirty {
            let bytes = self.host.docs.encode(id)?;
            let stem = autosave_stem(&title);
            self.state.write(&format!("{stem}.{ext}"), &bytes)?;
            // Metadata sidecar: the original title for recovery (and the real type, for
            // diagnostics)
            let meta = serde_json::json!({ "doc_id": id.0, "type_id": type_id.0, "title": title });
            let _ = self.state.write(&format!("{stem}.json"), meta.to_string().as_bytes());
            saved += 1;
        }
        self.host.stats.autosaves += 1;
        if saved > 0 {
            self.host.log.info(format!("Autosave: {saved} documents"));
        }
        Ok(saved)
    }

    /// At startup: if the last session exited abnormally (no clean-exit marker), recover the
    /// autosaved documents. After a clean exit the autosave area is cleared — an old session's
    /// autosaves must not pop up after some later crash.
    fn restore_session(&mut self) {
        if self.state.read(CLEAN_EXIT).is_some() {
            for key in self.state.keys(AUTOSAVE_DIR) {
                self.state.remove(&key);
            }
            return;
        }
        self.host
            .log
            .warn("[Session] The last session did not exit cleanly; recovering autosaves…");
        self.recover_autosaves();
    }

    /// At startup: say so once if crash reports were written since the last launch, send them
    /// if the user agreed, and drop the oldest ones past the limit.
    fn handle_crash_reports(&mut self) {
        use crate::diagnostics::{crash_reports, KEPT_REPORTS, SEND_CRASH_REPORTS};

        let reports = crash_reports(self.state.as_ref());
        let excess = reports.len().saturating_sub(KEPT_REPORTS);
        for key in &reports[..excess] {
            self.state.remove(key);
        }
        let seen = self.state.read(CRASH_REPORTS_SEEN).unwrap_or_default();
        let seen = String::from_utf8_lossy(&seen);
        let new: Vec<&String> =
            reports[excess..].iter().filter(|key| key.as_str() > seen.as_ref()).collect();
        let Some(newest) = new.last() else {
            return;
        };
        let _ = self.state.write(CRASH_REPORTS_SEEN, newest.as_bytes());
        self.host.log.warn(format!(
            "A crash was recorded since the last launch ({} report(s)); Export Diagnostics \
             includes the details",
            new.len()
        ));

        let Some(reporter) = self.crash_reporter.clone() else {
            return;
        };
        if !self.host.settings.get_bool(SEND_CRASH_REPORTS, false) {
            return;
        }
        let reports: Vec<(String, String)> = new
            .iter()
            .filter_map(|key| {
                let text = String::from_utf8_lossy(&self.state.read(key)?).into_owned();
                let name = key.strip_prefix(crate::diagnostics::CRASH_DIR).unwrap_or(key);
                Some((name.to_string(), text))
            })
            .collect();
        let send = move || -> Vec<String> {
            reports
                .iter()
                .filter_map(|(name, text)| {
                    let error = reporter.send(name, text).err()?;
                    Some(format!("Failed to send crash report {name}: {error}"))
                })
                .collect()
        };
        api::AppServices::spawn_io(self, Box::pin(async move {
            #[cfg(not(target_arch = "wasm32"))]
            let failures = blocking::unblock(send).await;
            #[cfg(target_arch = "wasm32")]
            let failures = send();
            Box::new(move |app: &mut dyn api::AppServices| {
                for failure in failures {
                    app.host().log.warn(failure);
                }
            }) as api::UiThen
        }));
    }

    /// Reopen the documents in the autosave area (marked unsaved) and remove their entries.
    /// An entry that can't be recovered, such as one from a newer build, stays until the next
    /// clean exit, so a user going back to that build can still get it.
    fn recover_autosaves(&mut self) {
        let keys = self.state.keys(AUTOSAVE_DIR);
        let mut recovered = 0;
        for (meta_key, stem) in keys.iter().filter_map(|k| Some((k, k.strip_suffix(".json")?))) {
            // The content entry of the same name; its extension is the document type's
            let content = keys.iter().find(|key| {
                key.strip_prefix(stem)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .is_some_and(|ext| ext != "json")
            });
            let Some(content_key) = content else { continue };
            let Some(meta) = self
                .state
                .read(meta_key)
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                continue;
            };
            let name = stem.strip_prefix(AUTOSAVE_DIR).unwrap_or(stem);
            let title = format!(
                "[Recovered] {}",
                meta.get("title").and_then(|t| t.as_str()).unwrap_or(name)
            );
            // The recovered document gets no file: its first save asks where
            // to put it.
            let restored = self
                .host
                .docs
                .type_for(content_key)
                .and_then(|type_id| {
                    let bytes = self.state.read(content_key).ok_or("The content is gone")?;
                    self.host.docs.open_bytes(&type_id, &title, None, &bytes)
                });
            match restored {
                Ok(id) => {
                    if let Some(d) = self.host.docs.get_mut(id) {
                        d.dirty = true; // recovered content is saved only once the user confirms
                    }
                    self.host.docs.set_active(Some(id));
                    self.state.remove(meta_key);
                    self.state.remove(content_key);
                    recovered += 1;
                }
                Err(e) => self
                    .host
                    .log
                    .warn(format!("Failed to recover autosave {content_key}: {e}")),
            }
        }
        if recovered > 0 {
            self.host.log.warn(format!(
                "The last session exited abnormally; recovered {recovered} autosaved documents \
                 (marked unsaved, please review and save them)"
            ));
        }
    }

    // ---- Frame stats (phase 5: diagnostics panel) ----

    /// Called by the GUI frontend every frame.
    pub fn record_frame(&mut self, dt_ms: f32) {
        self.host.record_frame(dt_ms);
    }

    /// Pump the task event channel. Returns whether any event was taken (task state/log/
    /// invocation history changed, so the frontend needs a redraw).
    fn pump_task_events(&mut self) -> bool {
        let rx = self.host.task_events();
        let events: Vec<TaskEvent> = {
            let guard = rx.lock().unwrap_or_else(|p| p.into_inner());
            std::iter::from_fn(|| guard.try_recv().ok()).collect()
        };
        let any = !events.is_empty();
        for ev in events {
            self.host.tasks.apply_event(&ev);
            match ev {
                TaskEvent::Progress { .. } => {}
                TaskEvent::Log { message, .. } => {
                    self.host.log.info(message);
                }
                TaskEvent::Finished { outcome, id } => {
                    // Advance the matching record in the invocation history (unified invocation
                    // results, phase 2)
                    let (status, note) = match &outcome {
                        api::TaskOutcome::Succeeded { note, commit } => (
                            InvocationStatus::Succeeded,
                            commit.as_ref().map(|c| c.label.clone()).or_else(|| note.clone()),
                        ),
                        api::TaskOutcome::Failed { error } => {
                            (InvocationStatus::Failed, Some(error.clone()))
                        }
                        api::TaskOutcome::Cancelled => (InvocationStatus::Cancelled, None),
                    };
                    if let Some(r) = self
                        .invocations
                        .iter_mut()
                        .rev()
                        .find(|r| r.task_id == Some(id))
                    {
                        r.status = status;
                        r.note = note;
                    }
                    match outcome {
                        api::TaskOutcome::Succeeded { note, commit } => {
                            if let Some(commit) = commit {
                                // Background task results commit at a transaction boundary:
                                // document revision check (design.md §10.5)
                                let label = commit.label.clone();
                                match self.host.docs.apply_commit(commit) {
                                    Ok(_) => self
                                        .host
                                        .log
                                        .info(format!("Background task committed \"{label}\"")),
                                    Err(e) => self.host.log.warn(e),
                                }
                            }
                            self.host.log.info(format!(
                                "Task finished{}",
                                note.map(|n| format!(": {n}")).unwrap_or_default()
                            ));
                        }
                        api::TaskOutcome::Failed { error } => {
                            self.host.log.error(format!("Task failed: {error}"));
                        }
                        api::TaskOutcome::Cancelled => {
                            self.host.log.info("Task cancelled");
                        }
                    }
                }
            }
        }
        any
    }

    // ---- Command execution ----

    /// Register/update an invocation record.
    fn track(&mut self, record: InvocationRecord) {
        self.invocations.push_back(record);
        while self.invocations.len() > INVOCATION_HISTORY {
            self.invocations.pop_front();
        }
    }

    fn note_invocation(&mut self, seq: u64, status: InvocationStatus, note: Option<String>) {
        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
            r.status = status;
            r.note = note;
        }
    }

    /// The unified command entry point (design.md §10.6). Also records the queryable
    /// invocation history.
    pub fn execute_command(&mut self, id: &str, args: &CommandArgs) -> CommandResult {
        // Record the invocation (before running it; async/background stay Running until the
        // task's terminal event advances them)
        self.invocation_seq += 1;
        let seq = self.invocation_seq;
        let execution = self.registry.command(id).map(|d| d.execution());
        self.track(InvocationRecord {
            seq,
            command: id.to_string(),
            execution,
            status: InvocationStatus::Running,
            task_id: None,
            note: None,
        });

        if self.dispatch_depth > 16 {
            let msg = "Commands nested too deeply (possible self-recursion)".to_string();
            self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
            self.host.log.error(msg.clone());
            return CommandResult::failed(msg);
        }
        // Account commands run on the host directly (they manage the
        // background login thread; same interception pattern as dialogs).
        match id {
            crate::platform_cmds::ACCOUNT_LOGIN => {
                let result = match self.host.begin_account_login() {
                    Ok(()) => CommandResult::done(),
                    Err(e) => CommandResult::failed(e),
                };
                let status = if result.is_ok() {
                    InvocationStatus::Succeeded
                } else {
                    InvocationStatus::Failed
                };
                self.note_invocation(seq, status, None);
                return result;
            }
            crate::platform_cmds::ACCOUNT_CANCEL_LOGIN => {
                self.host.cancel_account_login();
                self.note_invocation(seq, InvocationStatus::Succeeded, None);
                return CommandResult::done();
            }
            crate::platform_cmds::ACCOUNT_LOGOUT => {
                self.host.account_logout();
                self.note_invocation(seq, InvocationStatus::Succeeded, None);
                return CommandResult::done();
            }
            _ => {}
        }
        let Some(mut def) = self.registry.take_command(id) else {
            let msg = format!("Unregistered command: {id}");
            self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
            self.host.log.error(msg.clone());
            return CommandResult::failed(msg);
        };
        let snapshot = self.make_snapshot();
        if let Some(enabled) = &def.enabled {
            if !enabled(&snapshot) {
                self.registry.put_command(def);
                let msg = format!("Command {id} is unavailable in the current context");
                self.note_invocation(seq, InvocationStatus::Failed, Some(msg.clone()));
                self.host.log.warn(msg.clone());
                return CommandResult::failed(msg);
            }
        }
        let result = match &mut def.kind {
            CommandKind::Sync(handler) => {
                self.dispatch_depth += 1;
                let r = handler(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices), args);
                self.dispatch_depth -= 1;
                r
            }
            CommandKind::Async(starter) => {
                self.dispatch_depth += 1;
                let started = starter(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices));
                self.dispatch_depth -= 1;
                match started {
                    Ok(spec) => {
                        // A task session (progress/cancellation) + the shared executor drives
                        // the future
                        let session =
                            self.host
                                .tasks
                                .begin(&spec.title, spec.cancellable, api::TaskKind::Async);
                        let job = (spec.job)(api::TaskCtx::new(
                            session.handle.clone(),
                            spec.input,
                        ));
                        let tx = self.host.tasks.event_sender();
                        let task_id = session.id;
                        self.executor.spawn_detached(async move {
                            let outcome = match job.await {
                                Ok(commit) => api::TaskOutcome::Succeeded {
                                    note: None,
                                    commit,
                                },
                                Err(api::TaskFailure::Cancelled) => api::TaskOutcome::Cancelled,
                                Err(api::TaskFailure::Error(e)) => {
                                    api::TaskOutcome::Failed { error: e }
                                }
                            };
                            let _ = tx.send(api::TaskEvent::Finished { id: task_id, outcome });
                        });
                        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
                            r.task_id = Some(task_id);
                        }
                        self.host.log.info(format!(
                            "Submitted async task #{} \"{}\"",
                            task_id.0, spec.title
                        ));
                        CommandResult::done_with(format!("Async task #{}", task_id.0))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            }
            CommandKind::Background(starter) => {
                self.dispatch_depth += 1;
                let started = starter(&mut CommandCtx::new(&mut *self as &mut dyn api::AppServices));
                self.dispatch_depth -= 1;
                match started {
                    Ok(spec) => {
                        let title = spec.title.clone();
                        let task_id = self.host.tasks.spawn(spec);
                        if let Some(r) = self.invocations.iter_mut().find(|r| r.seq == seq) {
                            r.task_id = Some(task_id);
                        }
                        self.host.log.info(format!(
                            "Submitted background task #{} \"{title}\"",
                            task_id.0
                        ));
                        CommandResult::done_with(format!("Background task #{}", task_id.0))
                    }
                    Err(e) => CommandResult::failed(e),
                }
            }
        };
        self.registry.put_command(def);
        // Sync commands reach their final state at once; async/background commands advance on
        // the task's finished event
        match (&result, &execution) {
            (CommandResult::Done { note }, Some(CommandExecution::Sync)) => {
                self.note_invocation(seq, InvocationStatus::Succeeded, note.clone());
            }
            (CommandResult::Failed { error }, Some(CommandExecution::Sync)) => {
                self.note_invocation(seq, InvocationStatus::Failed, Some(error.clone()));
            }
            (CommandResult::Failed { error }, _) => {
                // A failed async/background submission (the starter returned Err) is final
                // right away
                self.note_invocation(seq, InvocationStatus::Failed, Some(error.clone()));
            }
            _ => {}
        }
        if let CommandResult::Failed { error } = &result {
            self.host.log.error(format!("Command {id} failed: {error}"));
        }
        result
    }

    fn make_snapshot(&self) -> CtxSnapshot {
        let active_doc_id = self
            .host
            .ws
            .active_tab()
            .and_then(|t| t.document)
            .or(self.host.docs.active_id());
        let active_document = active_doc_id.and_then(|id| {
            self.host.docs.get(id).map(|d| DocSummary {
                id: d.id,
                title: d.title.clone(),
                dirty: d.dirty,
                revision: d.revision,
                can_undo: self.host.docs.can_undo(id),
                can_redo: self.host.docs.can_redo(id),
            })
        });
        let active_view_type = self.host.ws.active_tab().map(|t| t.type_id.0.clone());
        CtxSnapshot {
            active_document,
            active_view_type,
            work_mode: self.host.ws.work_mode.clone(),
        }
    }

    // ---- Layout persistence (design.md §6.5) ----

    /// Save the layout; `ui` is the GUI frontend's private physical layout data, `dock` the
    /// dock area's physical layout (the frontend's serialized `DockArea::dump`). Returns
    /// whether it was written: a layout equal to the last one loaded or saved is not rewritten
    /// (the web calls this periodically).
    pub fn save_layout(
        &mut self,
        ui: serde_json::Value,
        dock: Option<serde_json::Value>,
    ) -> Result<bool, String> {
        let mut file = self.host.ws.save_layout();
        file.ui = Some(ui);
        file.dock = dock;
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        if self.saved_layout.as_deref() == Some(json.as_str()) {
            return Ok(false);
        }
        let written = self.state.write(&self.layout_key, json.as_bytes());
        // Remembered even when the write failed: a failing store is retried
        // when the layout changes, not on every call.
        self.saved_layout = Some(json);
        written.map(|()| true)
    }

    /// Try to load the layout: apply the core part (panels/tabs/mode) and return the frontend's
    /// private part for the adapter to restore. Unregistered panels/views are skipped with a
    /// diagnostic (acceptance #9).
    pub fn load_layout(&mut self) -> Option<RestoredLayout> {
        let text = String::from_utf8(self.state.read(&self.layout_key)?).ok()?;
        let file: api::LayoutFile = match serde_json::from_str(&text) {
            Ok(f) => f,
            Err(e) => {
                self.host
                    .log
                    .warn(format!("Failed to parse the layout file; using the default: {e}"));
                return None;
            }
        };
        if file.version > api::LAYOUT_VERSION {
            self.host.log.warn(format!(
                "The layout was saved in a newer format ({}); using the default",
                file.version
            ));
            return None;
        }
        let skipped = self.host.ws.apply_layout(&file);
        for msg in skipped {
            self.host.log.warn(msg);
        }
        self.saved_layout = Some(text);
        self.snapshot = self.make_snapshot();
        self.host.log.info("Restored the last workspace layout");
        Some(RestoredLayout {
            ui: file.ui,
            dock: file.dock,
        })
    }
}

/// The frontend's private layout part restored by [`AppRuntime::load_layout`].
pub struct RestoredLayout {
    /// The GUI frontend's private physical layout (a legacy field; the frontend interprets it).
    pub ui: Option<serde_json::Value>,
    /// The dock area's physical layout (the gpui frontend hands it to `DockArea::load`).
    pub dock: Option<serde_json::Value>,
}

impl api::AppServices for AppRuntime {
    fn host(&mut self) -> &mut ServiceHost {
        &mut self.host
    }

    fn snapshot(&self) -> CtxSnapshot {
        self.snapshot.clone()
    }

    fn dispatch(&mut self, command: &str, args: &CommandArgs) -> CommandResult {
        self.execute_command(command, args)
    }

    fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String> {
        let doc = if bind_document {
            self.host.docs.active_id()
        } else {
            None
        };
        let type_id = ViewTypeId::new(view_type);
        self.host.ws.open_view(&type_id, doc, None)?;
        self.snapshot = self.make_snapshot();
        Ok(())
    }

    fn close_active_view(&mut self) -> Result<(), String> {
        let index = self.host.ws.active_tab;
        match self.host.ws.close_tab(index) {
            Some(_) => {
                self.host.log.info(format!("Closed center tab #{}", index + 1));
                self.snapshot = self.make_snapshot();
                Ok(())
            }
            None => Err("No center tab to close".to_string()),
        }
    }

    fn set_work_mode(&mut self, mode: &str) -> Result<(), String> {
        self.host.ws.set_work_mode(mode.to_string());
        self.host.log.info(format!("Switched work mode → {mode}"));
        self.snapshot = self.make_snapshot();
        Ok(())
    }

    fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String> {
        let now_visible = self.host.ws.toggle_panel(&PanelId::new(panel_id))?;
        self.host.log.info(format!(
            "Panel `{panel_id}` {}",
            if now_visible { "shown" } else { "hidden" }
        ));
        Ok(())
    }

    fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        self.disable_plugin(plugin_id)
    }

    fn autosave_now(&mut self) -> Result<usize, String> {
        self.autosave_now()
    }

    fn diagnostics_report(&self) -> String {
        crate::diagnostics::report(self)
    }

    fn request_file_dialog(
        &mut self,
        extensions: Vec<String>,
        default_name: Option<String>,
        action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
    ) {
        self.request_dialog(FileDialogRequest {
            extensions,
            default_name,
            action,
        });
    }

    fn finish_file_dialog(&mut self, picked: Option<api::Location>) {
        self.host.ws.file_picker = None;
        let Some(request) = self.dialog_in_flight.take() else {
            return;
        };
        if picked.is_none() {
            self.host.log.info("Cancelled");
        }
        (request.action)(self, picked);
    }

    fn spawn_io(&mut self, io: api::IoTask) {
        let done = self.io_done_tx.clone();
        self.io_pending += 1;
        self.executor.spawn_detached(async move {
            let _ = done.send(io.await);
        });
    }
}

impl AppRuntime {
    /// Disable a plugin at runtime (phase 5, §16.2/§18): removes all its contributions —
    /// commands, view types, panels, document types, ribbon groups and tabs — and updates the
    /// plugin registry mirror. Irreversible (restart the app to reload it).
    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        if !self.registry.plugins_loaded.iter().any(|x| x == plugin_id) {
            return Err(format!("Plugin not loaded: {plugin_id}"));
        }
        let removed = self.registry.remove_from_source(plugin_id);
        self.host.ws.remove_commands(&removed, plugin_id);
        self.host.ws.remove_from_source(plugin_id);
        self.registry.plugins_loaded.retain(|x| x != plugin_id);
        self.registry.plugins_disabled.push(plugin_id.to_string());
        if let Some(entry) = self
            .host
            .plugins
            .iter_mut()
            .find(|p| p.id == plugin_id && p.state == "Loaded")
        {
            entry.state = "Disabled";
            entry.detail = Some("Disabled at runtime".to_string());
        }
        self.host.log.warn(format!(
            "Plugin {plugin_id} disabled, {} commands removed",
            removed.len()
        ));
        Ok(removed)
    }
}

impl Drop for AppRuntime {
    fn drop(&mut self) {
        // Tell the async executor's worker threads to exit
        self.shutdown.store(true, Ordering::Relaxed);
        // The session's clean-exit marker (phase 5: crash recovery), checked by
        // `restore_session` at the next startup.
        if AUTOSAVE {
            let _ = self.state.write(CLEAN_EXIT, b"ok");
        }
        let _ = self.host.settings.save();
    }
}

// ---- Product assembly ----

/// A plugin loading stage: called by the builder during assembly, contributes commands/toolbars/
/// services to the registry. Failures are recorded through `Registry::error`/`plugins_rejected`
/// and do not abort assembly.
pub type PluginStage = Box<dyn FnOnce(&mut Registry) + Send>;

/// The product assembler (design.md §13): picks the Workbench modules, the default work mode
/// and the initial layout.
pub struct WorkbenchAppBuilder {
    product_id: String,
    product_name: String,
    /// Product identity and branding (product.toml). When given it overrides
    /// product_id/product_name and feeds the window title, ribbon logo and About dialog.
    profile: Option<api::ProductProfile>,
    workbenches: Vec<Box<dyn Workbench>>,
    plugin_stages: Vec<PluginStage>,
    default_mode: String,
    work_modes: Vec<String>,
    layout_path: Option<PathBuf>,
    open_on_start: Vec<(String, bool)>,
    create_default_document: bool,
    /// A document file to open at startup (the file-association entry
    /// point); applied after the default document would be created.
    open_file_on_start: Option<PathBuf>,
    /// The default document type ID (named by the product when there are several types).
    default_doc_type: Option<String>,
    smoke_seconds: Option<f32>,
    /// The product's asset source (SVG icons, …); None means platform standard assets only.
    assets: Option<std::sync::Arc<dyn api::AssetSource>>,
    /// The GUI backend label (for the diagnostics panel/logs; the product fills it in per
    /// build).
    backend_label: String,
    /// Extra GPU device requirements the product declares (passed through opaquely; the
    /// frontend interprets the concrete type).
    gpu_requirements: Option<Box<dyn std::any::Any>>,
    /// The ribbon collapse feature switch (ribbon-bar.md §4.5); false by default.
    ribbon_collapse_enabled: bool,
    /// The UI theme name ("dark"/"light"); written as the settings default, user settings win.
    theme: String,
    /// The product's module resolver: given the module IDs a plugin declares, returns the
    /// Workbenches to assemble.
    ///
    /// A plugin is a runtime unit — it enables modules the product already compiled rather than
    /// carrying code. The product supplies this `module ID -> Workbench` table, and the
    /// platform calls it after loading plugins.
    module_loader: Option<ModuleLoader>,
    /// Injected account capability (Phase 3); None = no login surface.
    /// Where crash reports go if the user agrees; None = they stay local.
    crash_reporter: Option<Arc<dyn api::CrashReporter>>,
    account: Option<Arc<dyn api::AccountService>>,
    /// Where user files live: the local file system on the desktop, plus
    /// whatever the product adds.
    storage: api::Storage,
}

/// The product's module resolver: given a plugin ID and the module IDs it assembles this time,
/// returns the Workbenches.
///
/// Called once per plugin, so the platform can stamp all of that plugin's contributions with
/// their source through [`api::Registry::with_source`], and disabling it at runtime removes
/// exactly those.
pub type ModuleLoader = Box<dyn Fn(&str, &[String]) -> Vec<Box<dyn Workbench>> + Send>;

impl WorkbenchAppBuilder {
    pub fn new(product_id: impl Into<String>, product_name: impl Into<String>) -> Self {
        // Where user files live by default: the local file system on the
        // desktop, the browser's private file system (OPFS) on the web.
        let mut storage = api::Storage::default();
        #[cfg(not(target_arch = "wasm32"))]
        storage.register(api::storage::LOCAL, Arc::new(crate::local_files::LocalFiles));
        #[cfg(target_arch = "wasm32")]
        storage.register(crate::opfs::ID, Arc::new(crate::opfs::Opfs));
        Self {
            product_id: product_id.into(),
            product_name: product_name.into(),
            profile: None,
            workbenches: Vec::new(),
            plugin_stages: Vec::new(),
            default_mode: "General".to_string(),
            work_modes: vec!["General".to_string()],
            layout_path: None,
            open_on_start: Vec::new(),
            create_default_document: false,
            open_file_on_start: None,
            default_doc_type: None,
            smoke_seconds: None,
            assets: None,
            backend_label: "unknown".to_string(),
            gpu_requirements: None,
            ribbon_collapse_enabled: false,
            theme: "dark".to_string(),
            module_loader: None,
            account: None,
            crash_reporter: None,
            storage,
        }
    }

    /// Add a place user files can live (a cloud store, …), under `id`:
    /// documents there have [`api::Location`]s with that provider, and the
    /// open/save dialogs offer it in the in-app picker. Built in: the local
    /// file system on the desktop ([`api::storage::LOCAL`], with the OS
    /// dialogs) and the browser's private file system on the web (`opfs`);
    /// registering either id replaces it.
    pub fn with_storage(
        mut self,
        id: impl Into<String>,
        provider: Arc<dyn api::StorageProvider>,
    ) -> Self {
        self.storage.register(id, provider);
        self
    }

    /// Set the product's module resolver (it assembles the Workbench modules plugins enable).
    pub fn with_module_loader(mut self, loader: ModuleLoader) -> Self {
        self.module_loader = Some(loader);
        self
    }

    /// Set the product identity and branding (the loaded `product.toml`, see
    /// [`crate::product`]). The profile's id/name override the matching
    /// [`WorkbenchAppBuilder::new`] arguments.
    pub fn with_profile(mut self, profile: api::ProductProfile) -> Self {
        self.product_id = profile.id.clone();
        self.product_name = profile.name.clone();
        self.profile = Some(profile);
        self
    }

    /// Inject the account capability (Phase 3). None (the default) hides the
    /// login surface; the reference implementation is
    /// [`crate::account::DeviceFlowClient`].
    pub fn with_account(mut self, service: Arc<dyn api::AccountService>) -> Self {
        self.account = Some(service);
        self
    }

    /// Inject where crash reports go. They are sent only once the user turns on the setting
    /// this adds; by default they never leave the machine.
    pub fn with_crash_reporter(mut self, reporter: Arc<dyn api::CrashReporter>) -> Self {
        self.crash_reporter = Some(reporter);
        self
    }

    /// Set the product's asset source (SVG icons and other static assets). Lookup order:
    /// product source → platform standard source ([`crate::assets::standard_assets`]); the
    /// product can override a standard icon by using the same path.
    pub fn with_assets(
        mut self,
        source: impl api::AssetSource + 'static,
    ) -> Self {
        self.assets = Some(std::sync::Arc::new(source));
        self
    }

    /// The effective asset source: the product source first, the platform standard source as
    /// the fallback.
    pub fn asset_source(&self) -> std::sync::Arc<dyn api::AssetSource> {
        let mut sources: Vec<std::sync::Arc<dyn api::AssetSource>> = Vec::new();
        if let Some(a) = &self.assets {
            sources.push(a.clone());
        }
        sources.push(std::sync::Arc::new(*crate::assets::standard_assets()));
        std::sync::Arc::new(api::ChainedAssets::new(sources))
    }

    /// Declare extra GPU device requirements the product needs (features / limits), in place
    /// before the frontend creates the device. The GUI frontend interprets the type
    /// (workbench-ui expects `workbench_ui::gpu_view::GpuRequirements`); this layer only passes
    /// a `Box<dyn Any>` through (the GUI-independent contract stays). Typical use: the
    /// product's own renderer needs limits above the platform baseline (such as the compute
    /// shader storage buffer count).
    pub fn with_gpu_requirements(mut self, requirements: Box<dyn std::any::Any>) -> Self {
        self.gpu_requirements = Some(requirements);
        self
    }

    /// Take the GPU device requirements (consumed by an assembly step before `build()`).
    pub fn take_gpu_requirements(&mut self) -> Option<Box<dyn std::any::Any>> {
        self.gpu_requirements.take()
    }

    /// Label the GUI backend (for the diagnostics panel/automated queries).
    pub fn with_backend_label(mut self, label: impl Into<String>) -> Self {
        self.backend_label = label.into();
        self
    }

    /// Enable the ribbon collapse feature (ribbon-bar.md §4.5): the toggle
    /// button, Tab double-click and Ctrl+F1 become active.
    pub fn ribbon_collapse_enabled(mut self, on: bool) -> Self {
        self.ribbon_collapse_enabled = on;
        self
    }

    /// The UI theme (`"dark"` / `"light"`). Used as the default of the `ui.theme` setting —
    /// the user's saved setting wins.
    pub fn with_theme(mut self, name: impl Into<String>) -> Self {
        self.theme = name.into();
        self
    }

    pub fn with_workbench(mut self, wb: impl Workbench) -> Self {
        self.workbenches.push(Box::new(wb));
        self
    }

    /// Append a plugin loading stage (such as `workbench_rhai::plugin_stage(dirs)`).
    pub fn with_plugin_stage(mut self, stage: PluginStage) -> Self {
        self.plugin_stages.push(stage);
        self
    }

    /// The work modes and the default mode.
    pub fn with_modes(mut self, modes: &[&str], default: &str) -> Self {
        self.work_modes = modes.iter().map(|s| s.to_string()).collect();
        self.default_mode = default.to_string();
        self
    }

    /// The layout file. On the desktop its directory holds the rest of the
    /// app's state too (settings, autosaves); the default is the product's
    /// config directory. The web keeps state in localStorage and uses only
    /// the file name.
    pub fn with_layout_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.layout_path = Some(path.into());
        self
    }

    /// A center view opened at startup (view type ID, whether it binds the document).
    pub fn open_on_start(mut self, view_type: &str, bind_document: bool) -> Self {
        self.open_on_start.push((view_type.to_string(), bind_document));
        self
    }

    /// Create a default document at startup (of the first registered document type).
    pub fn create_default_document(mut self, on: bool) -> Self {
        self.create_default_document = on;
        self
    }

    /// Open this document file at startup (the file-association entry
    /// point). It is read in the background and opens on an early frame
    /// tick, after the default document, so a launch with both ends up with
    /// the launched file active. A read failure is logged, not fatal.
    pub fn with_open_file_on_start(mut self, path: PathBuf) -> Self {
        self.open_file_on_start = Some(path);
        self
    }

    /// Name the default document type (for products that register several document types).
    ///
    /// Without one, the first registered document type is used.
    pub fn with_default_document_type(mut self, type_id: impl Into<String>) -> Self {
        self.default_doc_type = Some(type_id.into());
        self
    }

    /// Smoke mode: exit automatically `seconds` seconds after the window opens (for automated
    /// verification).
    pub fn smoke(mut self, seconds: f32) -> Self {
        self.smoke_seconds = Some(seconds);
        self
    }

    pub fn smoke_seconds(&self) -> Option<f32> {
        self.smoke_seconds
    }

    /// `Name version (id)`, as the log and crash reports state it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn product_line(&self) -> String {
        let version = self.profile.as_ref().map_or(api::VERSION, |p| p.display_version());
        format!("{} {version} ({})", self.product_name, self.product_id)
    }

    /// The directory the product's state lives in.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn state_dir(&self) -> PathBuf {
        crate::state::dir(&self.product_id, self.layout_path.as_deref())
    }

    /// The product's state store, as the runtime will open it.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn state_store(&self) -> Arc<dyn api::StateStore> {
        crate::state::open(&self.product_id, self.layout_path.as_deref()).0
    }

    /// Assemble the product. Workbench init failures are caught and isolated (acceptance #19).
    pub fn build(mut self) -> Result<AppRuntime, String> {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let (tx, rx) = api::task_channel();
        let mut host = ServiceHost::new(tx, rx, self.default_mode.clone());
        host.ws.set_work_modes(self.work_modes.clone(), &self.default_mode);

        let mut registry = Registry::new();
        for wb in &self.workbenches {
            registry.workbenches.push(wb.id());
            let result = catch_unwind(AssertUnwindSafe(|| wb.init(&mut registry)));
            if let Err(panic) = result {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown panic".to_string());
                registry.error(format!("Workbench `{}` failed to initialize: {msg}", wb.id()));
            }
        }

        // Plugin loading stages (Rhai script plugins, …): the same isolation as Workbenches
        // (acceptance #19)
        for stage in self.plugin_stages {
            let result = catch_unwind(AssertUnwindSafe(|| stage(&mut registry)));
            if let Err(panic) = result {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown panic".to_string());
                registry.error(format!("Plugin loading stage failed: {msg}"));
            }
        }

        // Workbench modules enabled by plugins: a plugin declares IDs, and the product's
        // module resolver turns them into real Workbenches. This runs before the platform
        // commands so the "View" group receives the panels these modules register.
        if let Some(loader) = self.module_loader.take() {
            let mut loaded: Vec<String> = Vec::new();
            for (plugin_id, modules) in registry.plugin_modules.clone() {
                let pending: Vec<String> = modules
                    .iter()
                    .filter(|id| !loaded.contains(id))
                    .cloned()
                    .collect();
                if pending.is_empty() {
                    continue;
                }
                loaded.extend(pending.iter().cloned());
                // Stamp every contribution these modules register with the
                // plugin id, so disabling the plugin can remove them.
                let workbenches = loader(&plugin_id, &pending);
                registry.with_source(&plugin_id, |reg| {
                    for wb in workbenches {
                        reg.workbenches.push(wb.id());
                        let result = catch_unwind(AssertUnwindSafe(|| wb.init(reg)));
                        if let Err(panic) = result {
                            let msg = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                                .unwrap_or_else(|| "unknown panic".to_string());
                            reg.error(format!("Plugin module failed to initialize: {msg}"));
                        }
                    }
                });
            }
        }

        // Platform commands (including each panel's show/hide command and the "View" toolbar
        // group)
        let panels: Vec<(String, String, Option<String>)> = registry
            .panels
            .iter()
            .map(|p| (p.id.0.clone(), p.title.clone(), p.icon.clone()))
            .collect();
        if self.crash_reporter.is_some() {
            registry.register_setting(api::SettingSpec {
                key: crate::diagnostics::SEND_CRASH_REPORTS.into(),
                label: "Send crash reports".into(),
                group: "Diagnostics".into(),
                kind: api::SettingKind::Bool,
                default: serde_json::json!(false),
                scope: api::SettingScope::User,
            });
        }
        platform_cmds::register(&mut registry, &panels);

        // Registry → runtime
        for def in std::mem::take(&mut registry.doc_types) {
            if let Err(e) = host.docs.register_type(def) {
                host.log.error(e);
            }
        }
        for def in std::mem::take(&mut registry.views) {
            if let Err(e) = host.ws.register_view(def) {
                host.log.error(e);
            }
        }
        for def in std::mem::take(&mut registry.panels) {
            if let Err(e) = host.ws.register_panel(def, true) {
                host.log.error(e);
            }
        }
        // Explicit ribbon tabs register first; legacy toolbar groups land in the default tab
        // "Home"
        for tab in std::mem::take(&mut registry.ribbon) {
            host.ws.add_ribbon_tab(tab);
        }
        for group in std::mem::take(&mut registry.toolbar) {
            host.ws.add_toolbar_group(group);
        }
        // The quick access toolbar (a static product set, ribbon-bar.md §4.1)
        host.ws.qat = std::mem::take(&mut registry.qat);
        // The ribbon collapse feature switch (declared by the product through the builder,
        // ribbon-bar.md §4.5)
        host.ws.ribbon_collapse_enabled = self.ribbon_collapse_enabled;
        for (ty, value) in registry.take_services() {
            host.insert_raw(ty, value);
        }
        // Work modes suggested by the Workbenches
        if !registry.work_modes.is_empty() {
            host.ws
                .set_work_modes(registry.work_modes.clone(), &registry.default_mode);
        }
        // The default document type the product names (when there are several types)
        if let Some(id) = self.default_doc_type.clone() {
            host.ws.default_document_type = Some(id);
        }
        for err in &registry.errors {
            host.log.error(format!("[Assembly] {err}"));
        }
        for note in &registry.notes {
            host.log.info(format!("[Assembly] {note}"));
        }
        for plugin in &registry.plugins_loaded {
            host.log.info(format!("[Plugins] Loaded {plugin}"));
        }
        for (id, reason) in &registry.plugins_rejected {
            host.log.error(format!("[Plugins] Rejected {id}: {reason}"));
        }

        // State store: layout + settings + autosaves (phase 5)
        let (state, layout_key) =
            crate::state::open(&self.product_id, self.layout_path.as_deref());
        let (settings, settings_err) = api::Settings::load(state.clone(), SETTINGS);
        host.settings = settings;
        if let Some(e) = settings_err {
            host.log.warn(format!("[Settings] {e}"));
        }
        // The UI theme default: declared by the builder, the user's saved setting wins.
        if host.settings.get("ui.theme").is_none() {
            let _ = host
                .settings
                .set("ui.theme", serde_json::Value::String(self.theme.clone()));
        }
        // Brand accent: product.toml's branding.accent seeds ui.accent when
        // the product ships one and the user has not customized it.
        if let Some(accent) = &self.profile.as_ref().and_then(|p| p.accent.as_ref()) {
            if host.settings.get("ui.accent").is_none() {
                let _ = host
                    .settings
                    .set("ui.accent", serde_json::Value::String(accent.to_string()));
            }
        }
        // Setting specs: registry (platform built-ins + plugins) → host, so
        // the settings panel and `app.settings.set` see one list. Defaults
        // that are not yet stored are seeded (in memory only — a fresh value
        // persists on first write).
        host.setting_specs = std::mem::take(&mut registry.setting_specs);
        for spec in &host.setting_specs {
            if host.settings.get(&spec.key).is_none() {
                let default = spec.effective_default();
                host.settings.seed(&spec.key, default);
            }
        }
        // Locale tables: registry (platform + plugins) → host localizer.
        for (locale, key, text) in std::mem::take(&mut registry.locale_entries) {
            host.localizer.add_table(&locale, [(key, text)]);
        }
        // Default locale from product.toml [i18n]; the user's saved choice wins.
        if let Some(locale) = self.profile.as_ref().and_then(|p| p.default_locale.as_ref()) {
            if host.settings.get("ui.locale").is_none() {
                host.settings
                    .seed("ui.locale", serde_json::Value::String(locale.clone()));
            }
        }
        host.stats.backend = self.backend_label.clone();
        // Account capability (Phase 3): injected by the product; commands and
        // the status bar render from host.account + ws.account.
        host.account = self.account;
        host.storage = self.storage;

        // The plugin registry mirror (for the diagnostics/management panels); tier is the
        // license requirement the manifest declares.
        for id in &registry.plugins_loaded {
            let tier = registry
                .plugin_tiers
                .iter()
                .find(|(pid, _)| pid == id)
                .map(|(_, tier)| *tier);
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Loaded",
                detail: None,
                tier,
            });
        }
        for (id, reason) in &registry.plugins_rejected {
            host.plugins.push(api::PluginEntry {
                id: id.clone(),
                state: "Rejected",
                detail: Some(reason.clone()),
                tier: None,
            });
        }

        // The shared async executor (what Async commands run on)
        let shutdown = Arc::new(AtomicBool::new(false));
        let executor = AsyncExecutor::new(2, shutdown.clone());
        let (io_done_tx, io_done_rx) = std::sync::mpsc::channel();

        let mut runtime = AppRuntime {
            registry,
            host,
            product_id: self.product_id.clone(),
            product_name: self.product_name.clone(),
            profile: self
                .profile
                .clone()
                .unwrap_or_else(|| api::ProductProfile {
                    id: self.product_id.clone(),
                    name: self.product_name.clone(),
                    ..Default::default()
                }),
            snapshot: CtxSnapshot::default(),
            state,
            layout_key,
            saved_layout: None,
            autosave_accum: 0.0,
            dispatch_depth: 0,
            executor,
            shutdown,
            invocations: VecDeque::new(),
            invocation_seq: 0,
            dialog_in_flight: None,
            io_done_tx,
            io_done_rx,
            io_pending: 0,
            crash_reporter: self.crash_reporter.clone(),
        };
        runtime.snapshot = runtime.make_snapshot();

        // Crash recovery: the last session left no clean marker → recover the autosaved
        // documents (phase 5).
        if AUTOSAVE {
            runtime.restore_session();
        }
        runtime.handle_crash_reports();

        // Default document
        if self.create_default_document {
            // The product names its default type when it has more than one;
            // otherwise the only registered type is used.
            let default_type = self
                .default_doc_type
                .as_ref()
                .and_then(|id| {
                    runtime
                        .host
                        .docs
                        .types()
                        .into_iter()
                        .find(|t| &t.id.0 == id)
                        .map(|t| t.id.clone())
                })
                .or_else(|| runtime.host.docs.types().first().map(|t| t.id.clone()));
            if let Some(t) = default_type {
                let id = runtime
                    .host
                    .docs
                    .create(&t, "Untitled")
                    .map_err(|e| format!("Failed to create the default document: {e}"))?;
                runtime.host.docs.set_active(Some(id));
            }
        }

        // Open a file at startup (the file-association entry point; a failure is only logged,
        // it does not block startup).
        if let Some(path) = self.open_file_on_start.clone() {
            let mut ctx = api::CommandCtx::new(&mut runtime);
            crate::platform_cmds::open_document(&mut ctx, api::Location::local(path));
        }

        // Initial views (when a layout file exists the adapter restores the layout, and no
        // initial views are added on top)
        if runtime.state.read(&runtime.layout_key).is_none() {
            for (vt, bind) in &self.open_on_start {
                if let Err(e) = api::AppServices::open_view(&mut runtime, vt, *bind) {
                    runtime.host.log.warn(format!("Failed to open initial view {vt}: {e}"));
                }
            }
        }
        Ok(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> AppRuntime {
        WorkbenchAppBuilder::new("test.dialog", "Dialog test")
            .with_layout_path(std::env::temp_dir().join("workbench-rs-dialog-test/layout.json"))
            .build()
            .expect("assembly failed")
    }

    fn request(
        action: Box<dyn FnOnce(&mut dyn api::AppServices, Option<api::Location>)>,
    ) -> FileDialogRequest {
        FileDialogRequest {
            extensions: vec!["rcad".to_string()],
            default_name: None,
            action,
        }
    }

    /// The OS dialog's pick comes back as I/O: `frame_tick` runs the action
    /// on the UI thread with it. The blocking modal loop itself never runs on
    /// the UI thread.
    #[test]
    fn an_os_dialog_pick_runs_the_action_on_a_frame_tick() {
        use api::AppServices as _;
        let mut app = runtime();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Option<api::Location>>();
        app.dialog_in_flight = Some(request(Box::new(move |_, picked| {
            let _ = done_tx.send(picked);
        })));
        let picked = Some(api::Location::local("C:/tmp/part.rcad"));
        app.spawn_io(Box::pin(async move {
            Box::new(move |app: &mut dyn api::AppServices| app.finish_file_dialog(picked))
                as api::UiThen
        }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.pending_io() > 0 && std::time::Instant::now() < deadline {
            app.frame_tick(0.0);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(done_rx.try_recv().unwrap(), Some(api::Location::local("C:/tmp/part.rcad")));
        assert!(app.dialog_in_flight.is_none());
    }

    /// A cancelled dialog still runs the action, with `None`, and closes the
    /// in-app picker.
    #[test]
    fn a_cancelled_dialog_runs_the_action_with_none() {
        use api::AppServices as _;
        let mut app = runtime();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<Option<api::Location>>();
        app.dialog_in_flight = Some(request(Box::new(move |_, picked| {
            let _ = done_tx.send(picked);
        })));
        app.finish_file_dialog(None);
        assert_eq!(done_rx.try_recv().unwrap(), None);
        assert!(app.host.ws.file_picker.is_none());
    }

    /// Only one dialog may be in flight: a second request while one is
    /// pending is ignored (it neither replaces the pending request nor opens
    /// another dialog).
    #[test]
    fn new_request_is_ignored_while_dialog_in_flight() {
        let mut app = runtime();
        app.dialog_in_flight = Some(FileDialogRequest {
            extensions: vec![],
            default_name: None,
            action: Box::new(|_, _| {}),
        });

        app.request_dialog(request(Box::new(|_, _| {})));

        let pending = app.dialog_in_flight.as_ref().expect("the in-flight request is kept");
        assert!(pending.extensions.is_empty(), "the in-flight request was replaced");
        assert_eq!(app.pending_io(), 0, "an ignored request must not open a dialog");
    }
}
