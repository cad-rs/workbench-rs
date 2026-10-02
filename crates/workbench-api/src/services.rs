//! 平台服务聚合与工作台可见的能力门面（design.md §4.1 Platform）。

use std::any::{Any, TypeId};
use std::sync::{Arc, Mutex};

use crate::command::{CommandArgs, CommandResult, CtxSnapshot};
use crate::documents::DocumentService;
use crate::log::LogStore;
use crate::settings::Settings;
use crate::tasks::{TaskEventRx, TaskManager};
use crate::workspace::WorkspaceState;

/// 运行时统计（诊断面板/自动化测试用）。
#[derive(Clone, Debug, Default)]
pub struct RuntimeStats {
    /// 渲染帧数（由 GUI 前端每帧上报；无头运行恒为 0）。
    pub frames: u64,
    /// 帧耗时指数滑动平均（毫秒）。
    pub avg_frame_ms: f32,
    /// 最近一帧耗时（毫秒）。
    pub last_frame_ms: f32,
    /// GUI 后端标签（产品装配时写入，如 "egui"/"gpui"）。
    pub backend: String,
    /// 自动保存次数。
    pub autosaves: u64,
}

/// 插件注册条目（诊断/管理面板用）。
#[derive(Clone, Debug)]
pub struct PluginEntry {
    pub id: String,
    /// Loaded / Rejected / Disabled
    pub state: &'static str,
    /// Rejected/Disabled 的原因。
    pub detail: Option<String>,
}

/// 平台服务聚合。UI 线程所有。
pub struct ServiceHost {
    pub docs: DocumentService,
    pub tasks: TaskManager,
    pub ws: WorkspaceState,
    pub log: LogStore,
    pub settings: Settings,
    pub stats: RuntimeStats,
    /// 插件注册表镜像（build 时填充，运行时禁用同步更新）。
    pub plugins: Vec<PluginEntry>,
    /// 平台启动时刻。
    pub started: std::time::Instant,
    extras: std::collections::HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    task_rx: TaskEventRx,
}

impl ServiceHost {
    pub fn new(
        tx: std::sync::mpsc::Sender<crate::tasks::TaskEvent>,
        task_rx: TaskEventRx,
        default_mode: impl Into<String>,
    ) -> Self {
        Self {
            docs: DocumentService::new(),
            tasks: TaskManager::new(tx),
            ws: WorkspaceState::new(default_mode),
            log: LogStore::new(1000),
            settings: Settings::in_memory(),
            stats: RuntimeStats::default(),
            plugins: Vec::new(),
            started: std::time::Instant::now(),
            extras: std::collections::HashMap::new(),
            task_rx,
        }
    }

    /// 渲染帧上报（由 GUI 前端每帧调用）。
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

    /// 平台运行时长（秒）。
    pub fn uptime_secs(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// 取出任务事件接收端的锁（核心 frame_tick 泵事件用）。
    pub fn task_events(&self) -> Arc<Mutex<std::sync::mpsc::Receiver<crate::tasks::TaskEvent>>> {
        self.task_rx.clone()
    }

    // ---- Workbench/插件扩展服务（design.md §8.1 “服务”贡献）----

    pub fn set_service<T: Any + Send + Sync>(&mut self, value: T) {
        self.extras.insert(TypeId::of::<T>(), Box::new(value));
    }

    pub fn service<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extras.get(&TypeId::of::<T>()).and_then(|b| b.downcast_ref::<T>())
    }

    pub fn service_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.extras.get_mut(&TypeId::of::<T>()).and_then(|b| b.downcast_mut::<T>())
    }

    /// 装配阶段的原样搬入（来自 Registry.take_services）。
    pub fn insert_raw(&mut self, ty: TypeId, value: Box<dyn Any + Send + Sync>) {
        self.extras.insert(ty, value);
    }
}

/// 工作台（Workbench 模块、Python 插件宿主、视图）可见的平台能力。
///
/// 由核心的 AppRuntime 实现；CommandCtx / ViewCtx 经由该 trait 访问平台，
/// 以保证命令与视图代码只面对稳定 API（design.md §15）。
pub trait AppServices {
    /// 平台服务聚合。
    fn host(&mut self) -> &mut ServiceHost;
    /// 当前上下文快照。
    fn snapshot(&self) -> CtxSnapshot;
    /// 按稳定 ID 执行命令（统一入口）。
    fn dispatch(&mut self, command: &str, args: &CommandArgs) -> CommandResult;
    /// 打开中央视图；`bind_document` 为 true 时绑定当前活动文档。
    fn open_view(&mut self, view_type: &str, bind_document: bool) -> Result<(), String>;
    /// 关闭当前活动的中央标签。
    fn close_active_view(&mut self) -> Result<(), String>;
    /// 切换工作模式。
    fn set_work_mode(&mut self, mode: &str) -> Result<(), String>;
    /// 显示/隐藏面板。
    fn toggle_panel(&mut self, panel_id: &str) -> Result<(), String>;
    /// 运行时禁用插件：移除其命令与 Ribbon 贡献（阶段 5，§16.2）。
    /// 返回被移除的命令 ID 列表。
    fn disable_plugin(&mut self, plugin_id: &str) -> Result<Vec<String>, String>;
    /// 立即执行一次自动保存，返回保存的文档数（阶段 5）。
    fn autosave_now(&mut self) -> Result<usize, String>;
}
