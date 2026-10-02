//! 后台任务模型（design.md §10.2/§10.3/§10.4）。
//!
//! 任务在线程池线程执行，通过事件通道回报进度；取消为协作式；
//! 终态为 Succeeded / Failed / Cancelled。任务不得直接触碰 UI 或文档内部状态，
//! 结果通过 [`DocCommit`]（带文档版本校验）交回宿主提交。

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
            TaskStatus::Pending => "等待中",
            TaskStatus::Running => "运行中",
            TaskStatus::Succeeded => "已完成",
            TaskStatus::Failed => "失败",
            TaskStatus::Cancelled => "已取消",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

/// 任务运行载体：后台独占线程，或平台共享异步执行器（阶段 2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    Thread,
    Async,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskKind::Thread => "线程",
            TaskKind::Async => "异步",
        }
    }
}

/// 单个任务的展示信息。
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: TaskId,
    pub title: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    /// 确定进度 0.0..=1.0；None 表示不确定进度。
    pub progress: Option<f32>,
    pub stage: Option<String>,
    pub cancellable: bool,
    /// 终态补充信息（错误说明或完成备注）。
    pub detail: Option<String>,
}

/// 任务失败原因。
#[derive(Debug, Clone)]
pub enum TaskFailure {
    Cancelled,
    Error(String),
}

impl TaskFailure {
    pub fn message(&self) -> String {
        match self {
            TaskFailure::Cancelled => "任务已取消".to_string(),
            TaskFailure::Error(e) => e.clone(),
        }
    }
}

impl From<String> for TaskFailure {
    fn from(e: String) -> Self {
        TaskFailure::Error(e)
    }
}

/// 后台任务向宿主提交的文档变更：须通过文档版本校验后由宿主在事务边界提交。
pub struct DocCommit {
    pub doc_id: DocumentId,
    /// 提交者基于的文档版本；宿主校验 `revision == expect_revision`，不匹配则拒绝。
    pub expect_revision: u64,
    pub label: String,
    /// 该变更是否可撤销（写入撤销历史）。
    pub undoable: bool,
    pub content: Box<dyn DocumentContent>,
}

/// 任务终态载荷。
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

/// 任务事件（任务线程 → UI 线程）。
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

/// 任务线程内的进度/取消句柄（可克隆）。
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
    /// 协作式取消检查点：已取消时返回 Err，任务应尽快退出。
    pub fn check_cancelled(&self) -> Result<(), TaskFailure> {
        if self.is_cancelled() {
            Err(TaskFailure::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// 任务执行上下文（任务线程持有）。
pub struct TaskCtx {
    handle: ProgressHandle,
    input: Box<dyn std::any::Any + Send>,
}

impl TaskCtx {
    pub fn new(handle: ProgressHandle, input: Box<dyn std::any::Any + Send>) -> Self {
        Self { handle, input }
    }
    /// 进度/取消句柄克隆（供语言绑定包装，如 Python 插件的 task 对象）。
    pub fn progress_handle(&self) -> ProgressHandle {
        self.handle.clone()
    }
    /// 取命令在 UI 线程预先收集的输入数据（由任务定义者约定类型）。
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

/// 后台任务体：在任务线程执行，返回可选的文档提交。
pub type TaskJob = Box<dyn FnOnce(&mut TaskCtx) -> Result<Option<DocCommit>, TaskFailure> + Send>;

/// Background 命令在 UI 线程收集输入后产生的任务规格。
pub struct TaskSpec {
    pub title: String,
    pub cancellable: bool,
    pub input: Box<dyn std::any::Any + Send>,
    pub job: TaskJob,
}

/// 任务管理器：UI 线程所有。事件经通道由宿主 `frame_tick` 泵出。
pub struct TaskManager {
    next_id: u64,
    tx: mpsc::Sender<TaskEvent>,
    tasks: BTreeMap<u64, TaskInfo>,
    cancel_flags: BTreeMap<u64, Arc<AtomicBool>>,
}

/// 任务会话：核心创建任务后持有的句柄（ID + 进度/取消句柄）。
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

    /// 登记一个新任务（Running）并返回会话；任务体由调用方驱动
    /// （后台线程或异步执行器），完成后须发送 `TaskEvent::Finished`。
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

    /// 任务事件通道发送端（供执行器在任务完成时发送终态事件）。
    pub fn event_sender(&self) -> mpsc::Sender<TaskEvent> {
        self.tx.clone()
    }

    pub fn spawn(&mut self, spec: TaskSpec) -> TaskId {
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
            .map_err(|e| format!("任务线程创建失败: {e}"))
            .ok();
        session.id
    }

    /// 请求协作式取消。
    pub fn request_cancel(&mut self, id: TaskId) {
        if let Some(flag) = self.cancel_flags.get(&id.0) {
            flag.store(true, Ordering::Relaxed);
            if let Some(info) = self.tasks.get_mut(&id.0) {
                if !info.status.is_terminal() {
                    info.stage = Some("等待任务响应取消…".to_string());
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

    /// 应用一个来自事件通道的事件，更新任务状态（借用；事件本身由宿主继续处理）。
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
                            info.detail = Some("用户取消了任务".to_string());
                        }
                    }
                }
            }
        }
    }

    /// 状态栏摘要：运行中任务数与最近一个任务。
    pub fn summary(&self) -> (usize, Option<&TaskInfo>) {
        let running = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Running || t.status == TaskStatus::Pending)
            .count();
        (running, self.tasks.values().next_back())
    }
}

/// 事件接收端（供核心在 UI 线程泵出）。包一层 Mutex 以便 ServiceHost 保持 Send。
pub type TaskEventRx = Arc<Mutex<mpsc::Receiver<TaskEvent>>>;

/// 创建任务事件通道。
pub fn task_channel() -> (mpsc::Sender<TaskEvent>, TaskEventRx) {
    let (tx, rx) = mpsc::channel();
    (tx, Arc::new(Mutex::new(rx)))
}
