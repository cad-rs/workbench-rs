//! workbench-rs 平台契约层。
//!
//! 本 crate 定义 GUI 无关的平台 API：ID、几何与绘制抽象、命令、任务、文档、
//! 工作区模型与 Workbench 扩展点。Workbench 模块只允许依赖本 crate
//! （不依赖 egui / gpui / wgpu，design.md 验收 #21）。
//!
//! # 下游视角速查
//!
//! | 你是谁 | 看哪里 |
//! |---|---|
//! | 产品开发者 | workbench-core 的 WorkbenchAppBuilder 与 cli 模块 |
//! | Workbench 开发者 | workbench::Workbench + registry::Registry + command::CommandDef |
//! | 视图/面板作者 | view::ViewInstance + paint::PaintBackend（GUI 无关绘制） |
//! | 插件宿主 | workbench-python crate（本 crate 之上，Host API v1） |

pub mod command;
pub mod documents;
pub mod geo;
pub mod ids;
pub mod log;
pub mod paint;
pub mod registry;
pub mod services;
pub mod settings;
pub mod tasks;
pub mod view;
pub mod workbench;
pub mod workspace;

/// 平台契约版本（随发布递增；插件 Host API 版本另行管理，见 workbench-python）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use command::{
    AsyncJob, AsyncJobFactory, AsyncSpec, AsyncStarter, BackgroundStarter, CommandArgs,
    CommandCtx, CommandDef, CommandExecution, CommandInvocation, CommandKind, CommandResult,
    CtxSnapshot, DocSummary, Hotkey, InvocationRecord, InvocationStatus, KeyCode, SyncHandler,
};
pub use documents::{Document, DocumentContent, DocumentService, DocumentTypeDef};
pub use geo::{Color, Point, Rect, Size};
pub use ids::{
    CommandId, DocumentId, DocumentTypeId, PanelId, StableId, TabInstanceId, TaskId, ToolbarGroupId,
    ViewTypeId, WorkbenchId,
};
pub use log::{LogEntry, LogLevel, LogStore};
pub use paint::{PaintBackend, PaintHelpers};
pub use registry::Registry;
pub use services::{AppServices, PluginEntry, RuntimeStats, ServiceHost};
pub use settings::Settings;
pub use tasks::{
    task_channel, DocCommit, ProgressHandle, TaskCtx, TaskEvent, TaskEventRx, TaskFailure,
    TaskInfo, TaskJob, TaskKind, TaskManager, TaskOutcome, TaskSession, TaskSpec, TaskStatus,
};
pub use view::{ViewCtx, ViewInput, ViewInstance};
pub use workbench::{boxed, Workbench};
pub use workspace::{
    DockArea, LayoutFile, PanelDef, PanelSlot, RibbonTab, SavedPanel, SavedTab, TabSlot,
    ToolbarGroup, ToolbarItem, ViewDef, ViewFactory, WorkspaceState,
};
