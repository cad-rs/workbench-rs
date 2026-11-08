//! The command system contract (design.md §10): the single execution entry point for the
//! RibbonBar/toolbar, menus, hotkeys and the plugin API.

use std::pin::Pin;

use crate::documents::PendingSave;
use crate::ids::{CommandId, DocumentId};
use crate::services::{AppServices, FileThen, IoTask, UiThen};
use crate::storage::{Entry, Location, StorageFuture};
use crate::tasks::TaskSpec;

// ---- Hotkeys ----

/// Supported key codes (covering the common letter/digit/function/editing keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Enter, Escape, Tab, Backspace, Delete, Space,
    Left, Right, Up, Down, Plus, Minus,
}

impl KeyCode {
    /// Normalized name (lowercase), such as `n`, `0`, `f5`, `enter`.
    pub fn name(&self) -> &'static str {
        use KeyCode::*;
        match self {
            A => "a", B => "b", C => "c", D => "d", E => "e", F => "f", G => "g",
            H => "h", I => "i", J => "j", K => "k", L => "l", M => "m", N => "n",
            O => "o", P => "p", Q => "q", R => "r", S => "s", T => "t", U => "u",
            V => "v", W => "w", X => "x", Y => "y", Z => "z",
            Num0 => "0", Num1 => "1", Num2 => "2", Num3 => "3", Num4 => "4",
            Num5 => "5", Num6 => "6", Num7 => "7", Num8 => "8", Num9 => "9",
            F1 => "f1", F2 => "f2", F3 => "f3", F4 => "f4", F5 => "f5",
            F6 => "f6", F7 => "f7", F8 => "f8", F9 => "f9", F10 => "f10",
            F11 => "f11", F12 => "f12",
            Enter => "enter", Escape => "escape", Tab => "tab",
            Backspace => "backspace", Delete => "delete", Space => "space",
            Left => "left", Right => "right", Up => "up", Down => "down",
            Plus => "plus", Minus => "minus",
        }
    }

    /// Parse from a normalized name.
    pub fn from_name(s: &str) -> Option<KeyCode> {
        use KeyCode::*;
        let s = s.to_ascii_lowercase();
        Some(match s.as_str() {
            "a" => A, "b" => B, "c" => C, "d" => D, "e" => E, "f" => F, "g" => G,
            "h" => H, "i" => I, "j" => J, "k" => K, "l" => L, "m" => M, "n" => N,
            "o" => O, "p" => P, "q" => Q, "r" => R, "s" => S, "t" => T, "u" => U,
            "v" => V, "w" => W, "x" => X, "y" => Y, "z" => Z,
            "0" => Num0, "1" => Num1, "2" => Num2, "3" => Num3, "4" => Num4,
            "5" => Num5, "6" => Num6, "7" => Num7, "8" => Num8, "9" => Num9,
            "f1" => F1, "f2" => F2, "f3" => F3, "f4" => F4, "f5" => F5,
            "f6" => F6, "f7" => F7, "f8" => F8, "f9" => F9, "f10" => F10,
            "f11" => F11, "f12" => F12,
            "enter" => Enter, "escape" | "esc" => Escape, "tab" => Tab,
            "backspace" => Backspace, "delete" | "del" => Delete, "space" => Space,
            "left" => Left, "right" => Right, "up" => Up, "down" => Down,
            "plus" | "+" => Plus, "minus" | "-" => Minus,
            _ => return None,
        })
    }
}

/// A key combination, e.g. Ctrl+N.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: KeyCode,
}

impl Hotkey {
    pub fn new(key: KeyCode) -> Self {
        Self { ctrl: false, alt: false, shift: false, key }
    }
    pub fn ctrl(mut self, on: bool) -> Self {
        self.ctrl = on;
        self
    }
    pub fn alt(mut self, on: bool) -> Self {
        self.alt = on;
        self
    }
    pub fn shift(mut self, on: bool) -> Self {
        self.shift = on;
        self
    }
    /// Parse a string of the form `Ctrl+Shift+N`; returns None on failure (the registration site
    /// records a diagnostic).
    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key = None;
        for part in s.split('+') {
            let p = part.trim();
            match p.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                other => key = KeyCode::from_name(other),
            }
        }
        Some(Hotkey { ctrl, alt, shift, key: key? })
    }
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        write!(f, "{}", self.key.name().to_ascii_uppercase())
    }
}

// ---- Command arguments ----

/// Command arguments (string key/values; plugin-friendly — complex types are agreed on and
/// parsed by the command itself).
#[derive(Clone, Debug, Default)]
pub struct CommandArgs {
    pub values: std::collections::BTreeMap<String, String>,
    /// The value slot for running with a value (a Combo's selected value, …); None = no value.
    pub value: Option<String>,
}

/// A Combo (select box) state snapshot (ribbon-bar.md §4.2): current value + option list.
/// Queried from the command system every frame (`combo_state`); the Ribbon holds no option data,
/// so document state changes such as adding/removing layers show up naturally.
#[derive(Clone, Debug)]
pub struct ComboState {
    /// The currently selected value.
    pub current: String,
    /// The available options (ordered).
    pub options: Vec<String>,
}

impl CommandArgs {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }
    pub fn parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.get(key)?.parse().ok()
    }
}

// ---- Context snapshot ----

/// Active document summary (for enable conditions and UI display).
#[derive(Clone, Debug)]
pub struct DocSummary {
    pub id: DocumentId,
    pub title: String,
    pub dirty: bool,
    pub revision: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// The app context snapshot at command execution (design.md §6.4):
/// the active document, active view and work mode together decide whether a command is enabled.
#[derive(Clone, Debug, Default)]
pub struct CtxSnapshot {
    pub active_document: Option<DocSummary>,
    pub active_view_type: Option<String>,
    pub work_mode: String,
}

impl CtxSnapshot {
    pub fn active_view_is(&self, type_id: &str) -> bool {
        self.active_view_type.as_deref() == Some(type_id)
    }
}

// ---- Command definitions ----

/// The result of running a command.
#[derive(Clone, Debug)]
pub enum CommandResult {
    Done { note: Option<String> },
    Failed { error: String },
}

impl CommandResult {
    pub fn done() -> Self {
        CommandResult::Done { note: None }
    }
    pub fn done_with(note: impl Into<String>) -> Self {
        CommandResult::Done { note: Some(note.into()) }
    }
    pub fn failed(error: impl Into<String>) -> Self {
        CommandResult::Failed { error: error.into() }
    }
    pub fn is_ok(&self) -> bool {
        matches!(self, CommandResult::Done { .. })
    }
}

/// A sync command handler (runs immediately on the UI thread).
pub type SyncHandler = Box<dyn Fn(&mut CommandCtx, &CommandArgs) -> CommandResult + Send + Sync>;

/// A background command starter: collects input on the UI thread (with access to document
/// snapshots) and returns the task spec to submit.
pub type BackgroundStarter = Box<dyn Fn(&mut CommandCtx) -> Result<TaskSpec, String> + Send + Sync>;

// ---- Async commands (phase 2: lightweight futures on the shared executor, unlike background
// tasks with one thread per command) ----

/// An async command body: takes the task context (progress/cancellation/input) and returns an
/// awaitable future. The future runs on the platform's shared executor thread pool and must not
/// block (wait asynchronously, e.g. with `Timer::after`).
pub type AsyncJob =
    Pin<Box<dyn std::future::Future<Output = Result<Option<crate::tasks::DocCommit>, crate::tasks::TaskFailure>> + Send>>;

/// An async task body factory: called once by the executor after it creates the task session, to
/// produce the future.
pub type AsyncJobFactory = Box<dyn FnOnce(crate::tasks::TaskCtx) -> AsyncJob + Send + Sync>;

/// The spec an async command produces after collecting its input on the UI thread.
pub struct AsyncSpec {
    pub title: String,
    pub cancellable: bool,
    pub input: Box<dyn std::any::Any + Send>,
    pub job: AsyncJobFactory,
}

/// An async command starter: collects input on the UI thread and returns the async task spec.
pub type AsyncStarter = Box<dyn Fn(&mut CommandCtx) -> Result<AsyncSpec, String> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandExecution {
    /// Completes immediately on the UI thread.
    Sync,
    /// Submitted to the platform's shared async executor (lightweight, awaitable, cooperative
    /// cancellation).
    Async,
    /// Submitted as a background thread task (heavy CPU/IO, a dedicated thread).
    Background,
}

/// A command's enable condition (evaluated against the CtxSnapshot; None means always enabled).
pub type EnabledWhen = Box<dyn Fn(&CtxSnapshot) -> bool + Send + Sync>;

/// A command's on/off state query (the Toggle checked state, ribbon-bar.md §4.2; None means
/// always unchecked).
pub type CheckedWhen = Box<dyn Fn(&CtxSnapshot) -> bool + Send + Sync>;

/// A command's Combo state query (current value + option list; None means no Combo state).
pub type ComboStateWhen = Box<dyn Fn(&CtxSnapshot) -> Option<ComboState> + Send + Sync>;

/// A command definition.
pub struct CommandDef {
    pub id: CommandId,
    pub title: String,
    pub hotkey: Option<Hotkey>,
    /// Enable condition (None means always enabled). Evaluated against the CtxSnapshot.
    pub enabled: Option<EnabledWhen>,
    /// Toggle checked state query (None is always unchecked). Evaluated against the CtxSnapshot,
    /// in the same pattern as `enabled` — the command system is the single source of the on/off
    /// state (ribbon-bar.md §4.2).
    pub checked: Option<CheckedWhen>,
    /// Combo state query (current value + option list; None means no Combo state).
    pub combo: Option<ComboStateWhen>,
    pub kind: CommandKind,
    /// Icon asset path (such as `"icons/save.svg"`, resolved by the product's AssetSource);
    /// with None the UI falls back to plain text.
    pub icon: Option<String>,
    /// Contribution source (Workbench ID or plugin ID); for diagnostics and cleanup when a plugin
    /// is unloaded.
    pub source: Option<String>,
}

pub enum CommandKind {
    Sync(SyncHandler),
    Async(AsyncStarter),
    Background(BackgroundStarter),
}

impl CommandDef {
    pub fn sync(
        id: impl Into<CommandId>,
        title: impl Into<String>,
        handler: impl Fn(&mut CommandCtx, &CommandArgs) -> CommandResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            hotkey: None,
            enabled: None,
            checked: None,
            combo: None,
            kind: CommandKind::Sync(Box::new(handler)),
            icon: None,
            source: None,
        }
    }
    pub fn background(
        id: impl Into<CommandId>,
        title: impl Into<String>,
        starter: impl Fn(&mut CommandCtx) -> Result<TaskSpec, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            hotkey: None,
            enabled: None,
            checked: None,
            combo: None,
            kind: CommandKind::Background(Box::new(starter)),
            icon: None,
            source: None,
        }
    }
    pub fn async_command(
        id: impl Into<CommandId>,
        title: impl Into<String>,
        starter: impl Fn(&mut CommandCtx) -> Result<AsyncSpec, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            hotkey: None,
            enabled: None,
            checked: None,
            combo: None,
            kind: CommandKind::Async(Box::new(starter)),
            icon: None,
            source: None,
        }
    }
    pub fn hotkey(mut self, hk: Hotkey) -> Self {
        self.hotkey = Some(hk);
        self
    }
    /// Set the icon asset path (such as `"icons/save.svg"`, resolved by the product's
    /// AssetSource). When unset or unresolved, the UI falls back to plain text.
    pub fn icon(mut self, path: impl Into<String>) -> Self {
        self.icon = Some(path.into());
        self
    }
    pub fn enabled_when(
        mut self,
        f: impl Fn(&CtxSnapshot) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.enabled = Some(Box::new(f));
        self
    }
    /// Toggle checked state query (same pattern as [`CommandDef::enabled_when`], evaluated
    /// against the CtxSnapshot).
    pub fn checked_when(
        mut self,
        f: impl Fn(&CtxSnapshot) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.checked = Some(Box::new(f));
        self
    }
    /// Combo state query (current value + option list, evaluated against the CtxSnapshot every
    /// frame).
    pub fn combo_state_when(
        mut self,
        f: impl Fn(&CtxSnapshot) -> Option<ComboState> + Send + Sync + 'static,
    ) -> Self {
        self.combo = Some(Box::new(f));
        self
    }
    /// Mark the contribution source (Workbench/plugin stable ID).
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
    pub fn execution(&self) -> CommandExecution {
        match self.kind {
            CommandKind::Sync(_) => CommandExecution::Sync,
            CommandKind::Async(_) => CommandExecution::Async,
            CommandKind::Background(_) => CommandExecution::Background,
        }
    }
}

// ---- Command context ----

/// The platform capability facade available to command handlers.
pub struct CommandCtx<'a> {
    app: &'a mut dyn AppServices,
}

impl<'a> CommandCtx<'a> {
    pub fn new(app: &'a mut dyn AppServices) -> Self {
        Self { app }
    }
    /// Platform services (documents/tasks/workspace/log/extension services).
    pub fn host(&mut self) -> &mut crate::services::ServiceHost {
        self.app.host()
    }
    pub fn snapshot(&self) -> CtxSnapshot {
        self.app.snapshot()
    }
    /// Run another command (command chaining).
    pub fn run(&mut self, command: &str) -> CommandResult {
        self.app.dispatch(command, &CommandArgs::default())
    }
    pub fn run_with(&mut self, command: &str, args: CommandArgs) -> CommandResult {
        self.app.dispatch(command, &args)
    }
    /// Ask the user for a file (non-blocking); `action` runs on the UI thread
    /// with the pick. See [`AppServices::request_file_dialog`].
    pub fn request_file_dialog(
        &mut self,
        extensions: Vec<String>,
        default_name: Option<String>,
        action: Box<dyn FnOnce(&mut dyn AppServices, Option<Location>)>,
    ) {
        self.app.request_file_dialog(extensions, default_name, action);
    }
    /// Run I/O off the UI thread; see [`AppServices::spawn_io`].
    pub fn spawn_io(&mut self, io: IoTask) {
        self.app.spawn_io(io);
    }
    /// Read the file at `location` through its storage provider, off the UI
    /// thread; `then` gets the bytes (or a user-readable error) on the UI
    /// thread on a later frame tick.
    pub fn read_file(&mut self, location: Location, then: FileThen<Vec<u8>>) {
        let read = self.host().storage.read(&location);
        self.then_on_ui(read, then);
    }
    /// Write `bytes` to the file at `location`; like [`Self::read_file`].
    pub fn write_file(&mut self, location: Location, bytes: Vec<u8>, then: FileThen<()>) {
        let write = self.host().storage.write(&location, bytes);
        self.then_on_ui(write, then);
    }
    /// Write the bytes of a save from `DocumentService::begin_save`, first
    /// keeping the file they replace if the save asks for that, and finish
    /// the save; `then` gets where the document now lives, like
    /// [`Self::read_file`].
    pub fn write_save(&mut self, save: PendingSave, bytes: Vec<u8>, then: FileThen<Location>) {
        let storage = &self.host().storage;
        let write = match &save.keep_old_at {
            Some(keep_at) => storage.write_keeping_old(&save.location, keep_at, bytes),
            None => storage.write(&save.location, bytes),
        };
        self.then_on_ui(write, Box::new(move |app, written| {
            let saved = app.host().docs.finish_save(save, written);
            then(app, saved);
        }));
    }
    /// List the folder at `location`; like [`Self::read_file`].
    pub fn list_dir(&mut self, location: Location, then: FileThen<Vec<Entry>>) {
        let list = self.host().storage.list(&location);
        self.then_on_ui(list, then);
    }
    /// Create the folder at `location`; like [`Self::read_file`].
    pub fn create_dir(&mut self, location: Location, then: FileThen<()>) {
        let create = self.host().storage.create_dir(&location);
        self.then_on_ui(create, then);
    }
    fn then_on_ui<T: Send + 'static>(&mut self, op: StorageFuture<T>, then: FileThen<T>) {
        self.app.spawn_io(Box::pin(async move {
            let result = op.await;
            Box::new(move |app: &mut dyn AppServices| then(app, result)) as UiThen
        }));
    }
    /// Open a central view (optionally bound to the active document).
    pub fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String> {
        self.app.open_view(view_type, bind_document)
    }
    pub fn close_active_view(&mut self) -> Result<(), String> {
        self.app.close_active_view()
    }
    pub fn set_work_mode(&mut self, mode: &str) -> Result<(), String> {
        self.app.set_work_mode(mode)
    }
    pub fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String> {
        self.app.toggle_panel(panel_id)
    }
    /// Disable a plugin at runtime (removes its commands and Ribbon contributions).
    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        self.app.disable_plugin(plugin_id)
    }
    /// Autosave right now; returns the number of documents saved.
    pub fn autosave_now(&mut self) -> Result<usize, String> {
        self.app.autosave_now()
    }
    /// What support needs to see, as plain text.
    pub fn diagnostics_report(&self) -> String {
        self.app.diagnostics_report()
    }
    /// Shortcut to the active document ID.
    pub fn active_document(&self) -> Option<DocumentId> {
        self.app.snapshot().active_document.map(|d| d.id)
    }
}

// ---- Unified command invocation result (design.md §10.1/§16.4) ----

/// Command invocation status: a sync command reaches its terminal state immediately; an
/// async/background command starts as Running and is moved to its terminal state by the task's
/// terminal event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl InvocationStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, InvocationStatus::Running)
    }
}

/// A queryable record of one command invocation (for logs, automated tests and product
/// diagnostics).
#[derive(Clone, Debug)]
pub struct InvocationRecord {
    pub seq: u64,
    pub command: String,
    /// How it runs; None for an unregistered command.
    pub execution: Option<CommandExecution>,
    pub status: InvocationStatus,
    /// The task handle of an async/background invocation.
    pub task_id: Option<crate::ids::TaskId>,
    /// Terminal note (error description / completion note / label of the committed document
    /// change).
    pub note: Option<String>,
}

/// The unified command invocation result: the immediate result + the async handle.
#[derive(Clone, Debug)]
pub struct CommandInvocation {
    pub record: InvocationRecord,
    /// The immediate result of a sync command; for an async/background command, the submission
    /// result (Done).
    pub result: CommandResult,
    /// Shortcut to the task handle.
    pub task_id: Option<crate::ids::TaskId>,
}

impl CommandInvocation {
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }
}
