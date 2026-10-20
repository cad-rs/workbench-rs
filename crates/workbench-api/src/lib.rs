//! workbench-rs platform contract layer.
//!
//! This crate defines the GUI-agnostic platform APIs: IDs, geometry, commands,
//! tasks, documents, the workspace model, and Workbench extension points.
//! Workbench modules may only depend on this crate (never on egui / gpui /
//! wgpu — design.md acceptance item #21).
//!
//! A view's *content* is a frontend element tree, so the view contract hands
//! the frontend a type-erased element handle rather than a drawing interface;
//! see [`view::ViewInstance`].
//!
//! # Quick reference by downstream role
//!
//! | You are... | Look at |
//! |---|---|
//! | a product developer | workbench-core's WorkbenchAppBuilder and the cli module |
//! | a Workbench developer | workbench::Workbench + registry::Registry + command::CommandDef |
//! | a view/panel author | view::ViewInstance (content is a frontend element tree) |
//! | a plugin host | the workbench-rhai crate (Host API v1, layered on this crate) |

pub mod assets;
pub mod asyncrt;
pub mod command;
pub mod dispatch;
pub mod documents;
pub mod geo;
pub mod ids;
pub mod log;
pub mod registry;
pub mod services;
pub mod settings;
pub mod tasks;
pub mod view;
pub mod workbench;
pub mod workspace;

/// Platform contract version (bumped per release; the plugin Host API version
/// is managed separately — see workbench-rhai)..
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub use assets::{AssetSource, ChainedAssets, StaticAssets};
pub use asyncrt::sleep;
pub use command::{
    AsyncJob, AsyncJobFactory, AsyncSpec, AsyncStarter, BackgroundStarter, ComboState, CommandArgs,
    CommandCtx, CommandDef, CommandExecution, CommandInvocation, CommandKind, CommandResult,
    CtxSnapshot, DocSummary, Hotkey, InvocationRecord, InvocationStatus, KeyCode, SyncHandler,
};
pub use dispatch::{clear_dispatcher, dispatch, publish_dispatcher};
pub use documents::{Document, DocumentContent, DocumentService, DocumentTypeDef, UndoHooks};
pub use geo::{Color, Point, Rect, Size};
pub use ids::{
    CommandId, DocumentId, DocumentTypeId, PanelId, StableId, TabInstanceId, TaskId, ToolbarGroupId,
    ViewTypeId, WorkbenchId,
};
pub use log::{LogEntry, LogLevel, LogStore};
pub use registry::Registry;
pub use services::{AppServices, PluginEntry, RuntimeStats, ServiceHost};
pub use settings::Settings;
pub use tasks::{
    task_channel, DocCommit, ProgressHandle, TaskCtx, TaskEvent, TaskEventRx, TaskFailure,
    TaskInfo, TaskJob, TaskKind, TaskManager, TaskOutcome, TaskSession, TaskSpec, TaskStatus,
};
pub use view::ViewInstance;
pub use workbench::{boxed, Workbench};
pub use workspace::{
    DockArea, ItemSize, LayoutFile, PanelDef, PanelSlot, RibbonItemKind, RibbonTab, SavedPanel,
    SavedTab, TabSlot, ToolbarGroup, ToolbarItem, ViewDef, ViewFactory, WorkspaceState,
};
