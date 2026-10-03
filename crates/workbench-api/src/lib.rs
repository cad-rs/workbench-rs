//! workbench-rs platform contract layer.
//!
//! This crate defines the GUI-agnostic platform APIs: IDs, geometry and the
//! painting abstraction, commands, tasks, documents, the workspace model, and
//! Workbench extension points. Workbench modules may only depend on this crate
//! (never on egui / gpui / wgpu — design.md acceptance item #21).
//!
//! # Quick reference by downstream role
//!
//! | You are... | Look at |
//! |---|---|
//! | a product developer | workbench-core's WorkbenchAppBuilder and the cli module |
//! | a Workbench developer | workbench::Workbench + registry::Registry + command::CommandDef |
//! | a view/panel author | view::ViewInstance + paint::PaintBackend (GUI-free drawing) |
//! | a plugin host | the workbench-python crate (Host API v1, layered on this crate) |

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

/// Platform contract version (bumped per release; the plugin Host API version
/// is managed separately — see workbench-python).
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
