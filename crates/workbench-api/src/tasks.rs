//! The background task model (design.md §10.2/§10.3/§10.4).
//!
//! Tasks run on thread-pool threads and report progress through an event channel; cancellation
//! is cooperative; the terminal states are Succeeded / Failed / Cancelled. A task must not touch
//! the UI or a document's internal state directly; its result is handed back to the host for
//! commit through a [`DocCommit`] (with a document revision check).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use crate::documents::DocumentContent;
use crate::ids::{DocumentId, TaskId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "Pending",
            TaskStatus::Running => "Running",
            TaskStatus::Succeeded => "Done",
            TaskStatus::Failed => "Failed",
            TaskStatus::Cancelled => "Cancelled",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

/// What a task runs on: a dedicated background thread, or the platform's shared async executor
/// (phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Thread,
    Async,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Thread => "Thread",
            TaskKind::Async => "Async",
        }
    }
}

/// Display information for one task.
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    /// Determinate progress 0.0..=1.0; None means indeterminate progress.
    pub progress: Option<f32>,
    pub stage: Option<String>,
    pub cancellable: bool,
    /// Extra information about the terminal state (an error description or a completion note).
    pub detail: Option<String>,
}

/// Why a task failed.
#[derive(Debug, Clone)]
pub enum TaskFailure {
    Cancelled,
    Error(String),
}

impl TaskFailure {
    pub fn message(&self) -> String {
        match self {
            TaskFailure::Cancelled => "Task cancelled".to_string(),
            TaskFailure::Error(e) => e.clone(),
        }
    }
}

impl From<String> for TaskFailure {
    fn from(e: String) -> Self {
        TaskFailure::Error(e)
    }
}

/// A document change a background task submits to the host: it must pass the document revision
/// check, and the host commits it at a transaction boundary.
pub struct DocCommit {
    pub doc_id: DocumentId,
    /// The document revision the submitter based it on; the host checks
    /// `revision == expect_revision` and rejects it on a mismatch.
    pub expect_revision: u64,
    pub label: String,
    /// Whether the change can be undone (is written to the undo history).
    pub undoable: bool,
    pub content: Box<dyn DocumentContent>,
}

/// The payload of a task's terminal state.
pub enum TaskOutcome {
    Succeeded {
        note: Option<String>,
        commit: Option<DocCommit>,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

/// A task event (task thread → UI thread).
pub enum TaskEvent {
    Progress {
        id: TaskId,
        progress: Option<f32>,
        stage: Option<String>,
    },
    Log {
        id: TaskId,
        message: String,
    },
    Finished {
        id: TaskId,
        outcome: TaskOutcome,
    },
}

/// The progress/cancellation handle inside a task thread (cloneable).
#[derive(Clone)]
pub struct ProgressHandle {
    id: TaskId,
    tx: mpsc::Sender<TaskEvent>,
    cancel: Arc<AtomicBool>,
}

impl ProgressHandle {
    pub fn task_id(&self) -> TaskId {
        self.id
    }
    pub fn report(&self, progress: Option<f32>, stage: Option<&str>) {
        let _ = self.tx.send(TaskEvent::Progress {
            id: self.id,
            progress,
            stage: stage.map(|s| s.to_string()),
        });
    }
    pub fn log(&self, message: impl Into<String>) {
        let _ = self.tx.send(TaskEvent::Log {
            id: self.id,
            message: message.into(),
        });
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Cooperative cancellation checkpoint: returns Err once cancelled, and the task should exit
    /// as soon as possible.
    pub fn check_cancelled(&self) -> Result<(), TaskFailure> {
        if self.is_cancelled() {
            Err(TaskFailure::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The task execution context (held by the task thread).
pub struct TaskCtx {
    handle: ProgressHandle,
    input: Box<dyn std::any::Any + Send>,
}

impl TaskCtx {
    pub fn new(handle: ProgressHandle, input: Box<dyn std::any::Any + Send>) -> Self {
        Self { handle, input }
    }
    /// A clone of the progress/cancellation handle (for script bindings to wrap, such as the
    /// task object of Rhai plugins).
    pub fn progress_handle(&self) -> ProgressHandle {
        self.handle.clone()
    }
    /// The input data the command collected beforehand on the UI thread (its type is agreed by
    /// whoever defines the task).
    pub fn input<T: 'static>(&self) -> Option<&T> {
        self.input.downcast_ref::<T>()
    }
    pub fn report_progress(&self, fraction: Option<f32>) {
        self.handle.report(fraction, None);
    }
    pub fn set_stage(&self, stage: &str) {
        self.handle.report(None, Some(stage));
    }
    pub fn report(&self, fraction: Option<f32>, stage: &str) {
        self.handle.report(fraction, Some(stage));
    }
    pub fn log(&self, message: impl Into<String>) {
        self.handle.log(message);
    }
    pub fn is_cancelled(&self) -> bool {
        self.handle.is_cancelled()
    }
    pub fn check_cancelled(&self) -> Result<(), TaskFailure> {
        self.handle.check_cancelled()
    }
}

/// A background task body: runs on the task thread and returns an optional document commit.
pub type TaskJob = Box<dyn FnOnce(&mut TaskCtx) -> Result<Option<DocCommit>, TaskFailure> + Send>;

/// The task spec a Background command produces after collecting its input on the UI thread.
pub struct TaskSpec {
    pub title: String,
    pub cancellable: bool,
    pub input: Box<dyn std::any::Any + Send>,
    pub job: TaskJob,
}

/// The task manager: owned by the UI thread. Events are pumped out of the channel by the host's
/// `frame_tick`.
pub struct TaskManager {
    next_id: u64,
    tx: mpsc::Sender<TaskEvent>,
    tasks: BTreeMap<u64, TaskInfo>,
    cancel_flags: BTreeMap<u64, Arc<AtomicBool>>,
}

/// A task session: the handle the core holds after creating a task (ID + progress/cancellation
/// handle).
pub struct TaskSession {
    pub id: TaskId,
    pub handle: ProgressHandle,
}

impl TaskManager {
    pub fn new(tx: mpsc::Sender<TaskEvent>) -> Self {
        Self {
            next_id: 1,
            tx,
            tasks: BTreeMap::new(),
            cancel_flags: BTreeMap::new(),
        }
    }

    /// Register a new task (Running) and return its session; the caller drives the task body (a
    /// background thread or the async executor) and must send `TaskEvent::Finished` when done.
    pub fn begin(&mut self, title: &str, cancellable: bool, kind: TaskKind) -> TaskSession {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        self.tasks.insert(
            id.0,
            TaskInfo {
                id,
                title: title.to_string(),
                status: TaskStatus::Running,
                kind,
                progress: None,
                stage: None,
                cancellable,
                detail: None,
            },
        );
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_flags.insert(id.0, cancel.clone());
        TaskSession {
            id,
            handle: ProgressHandle {
                id,
                tx: self.tx.clone(),
                cancel,
            },
        }
    }

    /// The sender of the task event channel (for the executor to send the terminal event when a
    /// task finishes).
    pub fn event_sender(&self) -> mpsc::Sender<TaskEvent> {
        self.tx.clone()
    }

    pub fn spawn(&mut self, spec: TaskSpec) -> TaskId {
        #[cfg(target_arch = "wasm32")]
        {
            // Web build: no thread support, so a background task ends straight away as failed
            // (use an Async command instead).
            let session = self.begin(&spec.title, spec.cancellable, TaskKind::Thread);
            let _ = self.tx.send(TaskEvent::Finished {
                id: session.id,
                outcome: TaskOutcome::Failed {
                    error: "The web build does not support background thread tasks \
                            (use an async command instead)"
                        .to_string(),
                },
            });
            return session.id;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let session = self.begin(&spec.title, spec.cancellable, TaskKind::Thread);
            let handle = session.handle.clone();
            let tx = self.tx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("wb-task-{}", session.id.0))
                .spawn(move || {
                    let mut ctx = TaskCtx::new(handle, spec.input);
                    let outcome = match (spec.job)(&mut ctx) {
                        Ok(commit) => TaskOutcome::Succeeded { note: None, commit },
                        Err(TaskFailure::Cancelled) => TaskOutcome::Cancelled,
                        Err(TaskFailure::Error(e)) => TaskOutcome::Failed { error: e },
                    };
                    let _ = tx.send(TaskEvent::Finished {
                        id: session.id,
                        outcome,
                    });
                })
                .map(|_| ())
                .map_err(|e| format!("Failed to create the task thread: {e}"))
                .ok();
            session.id
        }
    }

    /// Request cooperative cancellation.
    pub fn request_cancel(&mut self, id: TaskId) {
        if let Some(flag) = self.cancel_flags.get(&id.0) {
            flag.store(true, Ordering::Relaxed);
            if let Some(info) = self.tasks.get_mut(&id.0) {
                if !info.status.is_terminal() {
                    info.stage =
                        Some("Waiting for the task to respond to cancellation…".to_string());
                }
            }
        }
    }

    pub fn get(&self, id: TaskId) -> Option<&TaskInfo> {
        self.tasks.get(&id.0)
    }

    pub fn all(&self) -> impl Iterator<Item = &TaskInfo> {
        self.tasks.values()
    }

    /// Apply an event from the event channel to update the task state (borrowed; the host keeps
    /// handling the event itself).
    pub fn apply_event(&mut self, ev: &TaskEvent) {
        match ev {
            TaskEvent::Progress { id, progress, stage } => {
                if let Some(info) = self.tasks.get_mut(&id.0) {
                    info.progress = *progress;
                    info.stage = stage.clone();
                }
            }
            TaskEvent::Log { id, message } => {
                if let Some(info) = self.tasks.get_mut(&id.0) {
                    info.stage = Some(message.clone());
                }
            }
            TaskEvent::Finished { id, outcome } => {
                if let Some(info) = self.tasks.get_mut(&id.0) {
                    match outcome {
                        TaskOutcome::Succeeded { note, .. } => {
                            info.status = TaskStatus::Succeeded;
                            info.progress = Some(1.0);
                            info.detail = note.clone();
                            info.stage = None;
                        }
                        TaskOutcome::Failed { error } => {
                            info.status = TaskStatus::Failed;
                            info.detail = Some(error.clone());
                        }
                        TaskOutcome::Cancelled => {
                            info.status = TaskStatus::Cancelled;
                            info.detail = Some("The user cancelled the task".to_string());
                        }
                    }
                }
            }
        }
    }

    /// Status bar summary: the number of running tasks and the most recent task.
    pub fn summary(&self) -> (usize, Option<&TaskInfo>) {
        let running = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Running || t.status == TaskStatus::Pending)
            .count();
        (running, self.tasks.values().next_back())
    }
}

/// The event receiver (for the core to pump on the UI thread). Wrapped in a Mutex so ServiceHost
/// stays Send.
pub type TaskEventRx = Arc<Mutex<mpsc::Receiver<TaskEvent>>>;

/// Create the task event channel.
pub fn task_channel() -> (mpsc::Sender<TaskEvent>, TaskEventRx) {
    let (tx, rx) = mpsc::channel();
    (tx, Arc::new(Mutex::new(rx)))
}
