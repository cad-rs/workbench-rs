//! 命令系统契约（design.md §10）：RibbonBar/工具栏、菜单、快捷键与插件 API 的统一执行入口。

use std::pin::Pin;

use crate::ids::{CommandId, DocumentId};
use crate::services::AppServices;
use crate::tasks::TaskSpec;

// ---- 快捷键 ----

/// 支持的按键码（覆盖常用字母/数字/功能键/编辑键）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Enter, Escape, Tab, Backspace, Delete, Space,
    Left, Right, Up, Down, Plus, Minus,
}

impl KeyCode {
    /// 规范化名称（小写），如 `n`、`0`、`f5`、`enter`。
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

    /// 由规范化名称解析。
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

/// 组合快捷键，例如 Ctrl+N。
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
    /// 解析 `Ctrl+Shift+N` 形式的字符串；失败返回 None（注册处会记录诊断）。
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

// ---- 命令参数 ----

/// 命令参数（字符串键值；插件友好，复杂类型由命令自行约定并解析）。
#[derive(Clone, Debug, Default)]
pub struct CommandArgs {
    pub values: std::collections::BTreeMap<String, String>,
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

// ---- 上下文快照 ----

/// 活动文档摘要（供启用条件与 UI 展示）。
#[derive(Clone, Debug)]
pub struct DocSummary {
    pub id: DocumentId,
    pub title: String,
    pub dirty: bool,
    pub revision: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// 命令执行时的应用上下文快照（design.md §6.4）：
/// 活动文档、活动视图与工作模式共同决定命令的启用状态。
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

// ---- 命令定义 ----

/// 命令执行结果。
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

/// 同步命令处理器（UI 线程立即执行）。
pub type SyncHandler = Box<dyn Fn(&mut CommandCtx, &CommandArgs) -> CommandResult + Send + Sync>;

/// 后台命令启动器：在 UI 线程收集输入（可访问文档快照），返回要投递的任务规格。
pub type BackgroundStarter = Box<dyn Fn(&mut CommandCtx) -> Result<TaskSpec, String> + Send + Sync>;

// ---- 异步命令（阶段 2：共享执行器上的轻量 future，区别于每命令一线程的后台任务）----

/// 异步命令体：接收任务上下文（进度/取消/输入），返回一个可 await 的 future。
/// future 在平台共享执行器线程池上运行，不得阻塞（用 `Timer::after` 等异步等待）。
pub type AsyncJob =
    Pin<Box<dyn std::future::Future<Output = Result<Option<crate::tasks::DocCommit>, crate::tasks::TaskFailure>> + Send>>;

/// 异步任务体工厂：执行器创建任务会话后调用一次，产出 future。
pub type AsyncJobFactory = Box<dyn FnOnce(crate::tasks::TaskCtx) -> AsyncJob + Send + Sync>;

/// 异步命令在 UI 线程收集输入后产生的规格。
pub struct AsyncSpec {
    pub title: String,
    pub cancellable: bool,
    pub input: Box<dyn std::any::Any + Send>,
    pub job: AsyncJobFactory,
}

/// 异步命令启动器：UI 线程收集输入并返回异步任务规格。
pub type AsyncStarter = Box<dyn Fn(&mut CommandCtx) -> Result<AsyncSpec, String> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandExecution {
    /// UI 线程立即完成。
    Sync,
    /// 提交到平台共享异步执行器（轻量、可 await、协作式取消）。
    Async,
    /// 提交为后台线程任务（重 CPU/IO，独占线程）。
    Background,
}

/// 命令定义。
pub struct CommandDef {
    pub id: CommandId,
    pub title: String,
    pub hotkey: Option<Hotkey>,
    /// 启用条件（None 表示总是可用）。依据 CtxSnapshot 求值。
    pub enabled: Option<Box<dyn Fn(&CtxSnapshot) -> bool + Send + Sync>>,
    pub kind: CommandKind,
    /// 贡献来源（Workbench ID 或插件 ID）；诊断与插件卸载清理用。
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
            kind: CommandKind::Sync(Box::new(handler)),
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
            kind: CommandKind::Background(Box::new(starter)),
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
            kind: CommandKind::Async(Box::new(starter)),
            source: None,
        }
    }
    pub fn hotkey(mut self, hk: Hotkey) -> Self {
        self.hotkey = Some(hk);
        self
    }
    pub fn enabled_when(
        mut self,
        f: impl Fn(&CtxSnapshot) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.enabled = Some(Box::new(f));
        self
    }
    /// 标记贡献来源（Workbench/插件稳定 ID）。
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

// ---- 命令上下文 ----

/// 命令处理器可用的平台能力门面。
pub struct CommandCtx<'a> {
    app: &'a mut dyn AppServices,
}

impl<'a> CommandCtx<'a> {
    pub fn new(app: &'a mut dyn AppServices) -> Self {
        Self { app }
    }
    /// 平台服务（文档/任务/工作区/日志/扩展服务）。
    pub fn host(&mut self) -> &mut crate::services::ServiceHost {
        self.app.host()
    }
    pub fn snapshot(&self) -> CtxSnapshot {
        self.app.snapshot()
    }
    /// 执行另一条命令（命令链）。
    pub fn run(&mut self, command: &str) -> CommandResult {
        self.app.dispatch(command, &CommandArgs::default())
    }
    pub fn run_with(&mut self, command: &str, args: CommandArgs) -> CommandResult {
        self.app.dispatch(command, &args)
    }
    /// 打开中央视图（可绑定活动文档）。
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
    /// 运行时禁用插件（移除其命令与 Ribbon 贡献）。
    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String> {
        self.app.disable_plugin(plugin_id)
    }
    /// 立即自动保存，返回保存文档数。
    pub fn autosave_now(&mut self) -> Result<usize, String> {
        self.app.autosave_now()
    }
    /// 活动文档 ID 快捷访问。
    pub fn active_document(&self) -> Option<DocumentId> {
        self.app.snapshot().active_document.map(|d| d.id)
    }
}

// ---- 统一命令调用结果（design.md §10.1/§16.4）----

/// 命令调用状态：同步命令立即到终态；异步/后台命令从 Running 开始，
/// 由任务终态事件推进到终态。
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

/// 一次命令调用的可查询记录（供日志、自动化测试与产品诊断使用）。
#[derive(Clone, Debug)]
pub struct InvocationRecord {
    pub seq: u64,
    pub command: String,
    /// 执行方式；未注册的命令为 None。
    pub execution: Option<CommandExecution>,
    pub status: InvocationStatus,
    /// 异步/后台调用对应的任务句柄。
    pub task_id: Option<crate::ids::TaskId>,
    /// 终态备注（错误说明 / 完成备注 / 提交的文档变更标签）。
    pub note: Option<String>,
}

/// 统一命令调用结果：立即结果 + 异步句柄。
#[derive(Clone, Debug)]
pub struct CommandInvocation {
    pub record: InvocationRecord,
    /// 同步命令的立即结果；异步/后台命令为提交结果（Done）。
    pub result: CommandResult,
    /// 便捷访问任务句柄。
    pub task_id: Option<crate::ids::TaskId>,
}

impl CommandInvocation {
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }
}
