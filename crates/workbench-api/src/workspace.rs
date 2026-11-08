//! The workspace model (design.md §6): toolbar, dock panels, the central multi-tab content area
//! and layout persistence.
//!
//! The model is entirely GUI-independent; the egui/gpui frontends render it as concrete widgets.
//! Layout persistence only uses stable IDs; restoring skips unregistered panels/views and records
//! a diagnostic.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::{DocumentId, PanelId, StableId, TabInstanceId, ToolbarGroupId, ViewTypeId};
use crate::view::ViewInstance;

/// Dock area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DockArea {
    Left,
    Right,
    Top,
    Bottom,
}

impl DockArea {
    pub fn as_str(&self) -> &'static str {
        match self {
            DockArea::Left => "left",
            DockArea::Right => "right",
            DockArea::Top => "top",
            DockArea::Bottom => "bottom",
        }
    }
}

// ---- Toolbar (RibbonBar: Tab → Group → Items, stable IDs + command binding + button kinds,
// ribbon-bar.md §2.4/§6) ----

/// Ribbon item kind (ribbon-bar.md §2.4). Defaults (`Default` and serde) to
/// [`RibbonItemKind::Command`], so old data and old registration code keep working unchanged.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RibbonItemKind {
    /// Plain command button: a click runs the command (the default kind, same as before).
    Command,
    /// Toggle button: a click only sends the command; the checked state is queried from the
    /// command system (`is_checked`).
    Toggle,
    /// Split button: the main area runs the default command, the arrow area pops up a menu of
    /// sub-commands.
    Split {
        /// The drop-down sub-command items.
        items: Vec<ToolbarItem>,
    },
    /// Pure menu button: the whole button only pops up a menu and runs nothing directly.
    Menu {
        /// The menu items.
        items: Vec<ToolbarItem>,
    },
    /// Select box (select-only): the current value and options are queried from the command
    /// system (`combo_state`); picking one runs the command with the value passed in
    /// [`CommandArgs::value`](crate::command::CommandArgs).
    Combo,
}

impl Default for RibbonItemKind {
    fn default() -> Self {
        RibbonItemKind::Command
    }
}

/// Ribbon item size (ribbon-bar.md §2.2). Defaults (`Default` and serde) to [`ItemSize::Small`]
/// (closest to the previous look).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ItemSize {
    /// Large button: a 24px icon above the label below, taking a column of its own.
    Large,
    /// Small button: a 16px icon and the label on one line (the default size).
    Small,
    /// Icon only: no label (QAT and self-explanatory icon commands; without an icon it falls back
    /// to the label's first character as a placeholder).
    Icon,
}

impl Default for ItemSize {
    fn default() -> Self {
        ItemSize::Small
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolbarItem {
    /// Stable ID of the bound command.
    pub command: crate::ids::CommandId,
    /// Display text; None falls back to the command title.
    pub label: Option<String>,
    /// Button kind; defaults to [`RibbonItemKind::Command`] (backward compatible).
    #[serde(default)]
    pub kind: RibbonItemKind,
    /// Size; defaults to [`ItemSize::Small`] (backward compatible).
    #[serde(default)]
    pub size: ItemSize,
    /// Declared Combo width (logical pixels); None uses the component's default width.
    #[serde(default)]
    pub width: Option<f32>,
}

impl ToolbarItem {
    pub fn command(command: impl Into<crate::ids::CommandId>) -> Self {
        Self {
            command: command.into(),
            label: None,
            kind: RibbonItemKind::Command,
            size: ItemSize::Small,
            width: None,
        }
    }
    pub fn labeled(command: impl Into<crate::ids::CommandId>, label: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            label: Some(label.into()),
            kind: RibbonItemKind::Command,
            size: ItemSize::Small,
            width: None,
        }
    }
    /// Toggle kind (the checked state is queried from the command system).
    pub fn toggle(mut self) -> Self {
        self.kind = RibbonItemKind::Toggle;
        self
    }
    /// Split kind: the main button runs the default command, the arrow area pops up the `items`
    /// sub-command menu.
    pub fn split(mut self, items: Vec<ToolbarItem>) -> Self {
        self.kind = RibbonItemKind::Split { items };
        self
    }
    /// Menu kind: the whole button pops up the `items` menu and runs nothing directly.
    pub fn menu(mut self, items: Vec<ToolbarItem>) -> Self {
        self.kind = RibbonItemKind::Menu { items };
        self
    }
    /// Combo kind: the options/current value are queried from the command system
    /// (`combo_state`).
    pub fn combo(mut self) -> Self {
        self.kind = RibbonItemKind::Combo;
        self
    }
    /// Large button size.
    pub fn large(mut self) -> Self {
        self.size = ItemSize::Large;
        self
    }
    /// Icon-only size (no label).
    pub fn icon_only(mut self) -> Self {
        self.size = ItemSize::Icon;
        self
    }
    /// Declare a width (Combo select boxes, …; logical pixels).
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
}

#[derive(Clone, Debug)]
pub struct ToolbarGroup {
    pub id: ToolbarGroupId,
    pub title: String,
    pub items: Vec<ToolbarItem>,
    /// Contribution source (Workbench/plugin ID); for cleanup when a plugin is unloaded.
    pub source: Option<String>,
}

impl ToolbarGroup {
    pub fn new(id: impl Into<ToolbarGroupId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            items: Vec::new(),
            source: None,
        }
    }
    /// Mark the contribution source.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
    /// Set the items (builder style).
    pub fn items(mut self, items: Vec<ToolbarItem>) -> Self {
        self.items = items;
        self
    }
}

// ---- View / panel definitions ----

/// A factory that creates view instances.
pub type ViewFactory = Box<dyn Fn() -> Box<dyn ViewInstance> + Send + Sync>;

/// Registration of a central content view type.
pub struct ViewDef {
    pub type_id: ViewTypeId,
    pub title: String,
    /// Icon asset path (such as `"icons/canvas.svg"`, resolved by the product's AssetSource);
    /// shown on the central tab, and None falls back to plain text.
    pub icon: Option<String>,
    /// Whether it is a singleton: only one tab of this type is open at a time.
    pub singleton: bool,
    /// Script central view: gpui-rhai script file path (mounted by the gpui
    /// frontend). When set the tab content is fully defined by the script and
    /// `factory` is never called.
    pub script: Option<String>,
    /// The feature this view needs (see [`crate::Entitlements`]); without it the view does
    /// not open, and an open tab closes.
    pub requires: Option<String>,
    /// The plugin this view type was contributed by, if any.
    pub source: Option<String>,
    pub factory: ViewFactory,
}

impl ViewDef {
    pub fn new(
        type_id: impl Into<ViewTypeId>,
        title: impl Into<String>,
        singleton: bool,
        factory: impl Fn() -> Box<dyn ViewInstance> + Send + Sync + 'static,
    ) -> Self {
        Self {
            type_id: type_id.into(),
            title: title.into(),
            icon: None,
            singleton,
            script: None,
            requires: None,
            source: None,
            factory: Box::new(factory),
        }
    }
    /// Set the icon asset path (shown on the central tab).
    pub fn icon(mut self, path: impl Into<String>) -> Self {
        self.icon = Some(path.into());
        self
    }
    /// Script central view: content is defined by a gpui-rhai script (returns a
    /// UiNode tree). `path` is the script file path; `factory` is never called
    /// (pass a placeholder).
    pub fn script(mut self, path: impl Into<String>) -> Self {
        self.script = Some(path.into());
        self
    }
    /// Put this view behind a feature the product defines.
    pub fn requires(mut self, feature: impl Into<String>) -> Self {
        self.requires = Some(feature.into());
        self
    }
    /// Attribute this view type to a plugin, so disabling the plugin removes it.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Dock panel registration.
pub struct PanelDef {
    pub id: PanelId,
    pub title: String,
    /// Icon asset path (such as `"icons/explorer.svg"`, resolved by the product's AssetSource);
    /// shown on the panel header, and None falls back to plain text. The platform's panel toggle
    /// commands (the Home tab's "View" group) use the same path, so the ribbon and the panel
    /// header show the same icon.
    pub icon: Option<String>,
    /// Script panel: gpui-rhai script file path (mounted and rendered by the gpui frontend).
    /// When set the panel content is fully defined by the script and `factory` is never called.
    pub script: Option<String>,
    pub area: DockArea,
    /// Default size: the width for left/right, the height for top/bottom (logical pixels).
    pub default_size: f32,
    pub order: i32,
    /// The feature this panel needs (see [`crate::Entitlements`]); without it the panel stays
    /// hidden and its toggle command is disabled.
    pub requires: Option<String>,
    /// The plugin this panel was contributed by, if any.
    pub source: Option<String>,
    pub factory: ViewFactory,
}

impl PanelDef {
    pub fn new(
        id: impl Into<PanelId>,
        title: impl Into<String>,
        area: DockArea,
        default_size: f32,
        order: i32,
        factory: impl Fn() -> Box<dyn ViewInstance> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: None,
            script: None,
            area,
            default_size,
            order,
            requires: None,
            source: None,
            factory: Box::new(factory),
        }
    }
    /// Set the icon asset path (shown on the panel header).
    pub fn icon(mut self, path: impl Into<String>) -> Self {
        self.icon = Some(path.into());
        self
    }
    /// Script panel: the content is defined by a gpui-rhai script (returning a UiNode tree).
    /// `path` is the script file path; `factory` is never called (pass a placeholder).
    pub fn script(mut self, path: impl Into<String>) -> Self {
        self.script = Some(path.into());
        self
    }
    /// Put this panel behind a feature the product defines.
    pub fn requires(mut self, feature: impl Into<String>) -> Self {
        self.requires = Some(feature.into());
        self
    }
    /// Attribute this panel to a plugin, so disabling the plugin removes it.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// A panel's runtime slot: definition + layout state + instance.
pub struct PanelSlot {
    pub id: PanelId,
    pub title: String,
    /// Icon asset path (copied from the [`PanelDef`] at registration).
    pub icon: Option<String>,
    /// Script panel path (copied from the [`PanelDef`] at registration); with Some no instance
    /// is created.
    pub script: Option<String>,
    pub area: DockArea,
    pub order: i32,
    pub size: f32,
    pub visible: bool,
}

/// A central tab slot.
pub struct TabSlot {
    pub instance_id: TabInstanceId,
    pub type_id: ViewTypeId,
    pub title: String,
    /// Icon asset path (copied from the [`ViewDef`] when opened).
    pub icon: Option<String>,
    /// Script path copied from the [`ViewDef`] when the tab was opened; `Some`
    /// means the frontend mounts the script instead of a Rust instance.
    pub script: Option<String>,
    /// The bound document (document views).
    pub document: Option<DocumentId>,
}

// ---- RibbonBar (design.md §7: Tab → Group → Item, stable IDs + command binding) ----

/// A Ribbon tab. Groups reuse [`ToolbarGroup`](crate::ToolbarGroup) (stable ID, items, source
/// mark).
#[derive(Clone, Debug)]
pub struct RibbonTab {
    pub id: StableId,
    pub title: String,
    pub groups: Vec<ToolbarGroup>,
    /// Contribution source (Workbench/plugin ID).
    pub source: Option<String>,
    /// Context marks (ribbon-bar.md §4.7): empty = an always-present tab; when it contains the
    /// current work_mode name the tab is visible in that mode (filtered by the render layer).
    pub contexts: Vec<String>,
}

impl RibbonTab {
    pub fn new(id: impl Into<StableId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            groups: Vec::new(),
            source: None,
            contexts: Vec::new(),
        }
    }
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
    pub fn groups(mut self, groups: Vec<ToolbarGroup>) -> Self {
        self.groups = groups;
        self
    }
    /// The platform default tab (compatibility entry: groups from `add_toolbar_group` land here).
    pub const DEFAULT_TAB_ID: &'static str = "app.home";
}

// ---- Layout persistence ----

#[derive(Serialize, Deserialize)]
pub struct SavedPanel {
    pub id: String,
    pub area: DockArea,
    pub order: i32,
    pub size: f32,
    pub visible: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SavedTab {
    pub type_id: String,
    pub title: String,
    /// Whether to bind the default document on restore.
    pub bind_document: bool,
}

/// The [`LayoutFile`] version this build writes. Older files load through `#[serde(default)]`;
/// newer ones are ignored, since a layout can always fall back to the default.
pub const LAYOUT_VERSION: u32 = 1;

/// The layout file (design.md §6.5). The `ui` field is left for GUI frontends to save their own
/// physical layout details.
#[derive(Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub work_mode: String,
    pub active_tab: usize,
    pub panels: Vec<SavedPanel>,
    pub tabs: Vec<SavedTab>,
    /// The active panel ID of each dock area.
    pub area_active: Vec<(DockArea, String)>,
    /// The active Ribbon tab (stable ID; old layout files without this field fall back to the
    /// first tab).
    #[serde(default)]
    pub ribbon_active_tab: Option<String>,
    /// Ribbon collapsed state (ribbon-bar.md §4.1); None = not recorded.
    /// Persisting the collapsed state is wired up by the frontend bridge (a later milestone); the
    /// core does not consume it.
    #[serde(default)]
    pub ribbon_collapsed: Option<bool>,
    /// Ribbon policy when space runs out (ribbon-bar.md §4.5): "scroll" (default) | "collapse";
    /// None = not recorded, and the core does not consume it.
    #[serde(default)]
    pub ribbon_overflow: Option<String>,
    /// The GUI frontend's private physical layout (such as an egui_tiles tree). It has no GUI
    /// semantics and the core does not parse it.
    #[serde(default)]
    pub ui: Option<serde_json::Value>,
    /// The physical layout of the dock areas (the gpui frontend's `DockArea::dump` state: each
    /// dock area's size, grouping and tab order). Likewise without GUI semantics and not parsed
    /// by the core; the frontend `load`s it on restore.
    #[serde(default)]
    pub dock: Option<serde_json::Value>,
}

/// One in-progress text edit (see [`WorkspaceState::text_edit`]): a settings
/// value, or a value prompt opened by any command.
#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    /// Key of the value being edited (`ui.accent`, …); handed back to the
    /// commit command as its `key` argument.
    pub key: String,
    /// Edit buffer (what the user has typed so far).
    pub buffer: String,
    /// Command run on Enter with the arguments `key` and `value`; `None` is
    /// the settings flow (`app.settings.set`).
    pub commit: Option<String>,
    /// Caption shown above the input; `None` shows the key.
    pub label: Option<String>,
}

impl TextEdit {
    /// A settings value edit (commits through `app.settings.set`).
    pub fn setting(key: impl Into<String>, buffer: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            buffer: buffer.into(),
            commit: None,
            label: None,
        }
    }

    /// A value prompt: Enter runs `commit` with the arguments `key` and
    /// `value` (the typed text); Esc or a backdrop click discards.
    pub fn prompt(
        key: impl Into<String>,
        buffer: impl Into<String>,
        commit: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            buffer: buffer.into(),
            commit: Some(commit.into()),
            label: Some(label.into()),
        }
    }

    /// The command Enter runs.
    pub fn commit_command(&self) -> &str {
        self.commit.as_deref().unwrap_or("app.settings.set")
    }

    /// The caption shown above the input.
    pub fn caption(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.key)
    }
}

/// The login/session state the UI renders (status bar account entry). The
/// platform state machine transitions it via the `app.account.*` commands;
/// the account service itself lives on the runtime.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum AccountPhase {
    /// No session.
    #[default]
    LoggedOut,
    /// Device auth in progress: show the user code + verification URI.
    Pending(crate::account::DeviceAuthRequest),
    /// Logged in.
    LoggedIn(crate::account::AccountUser),
    /// Login failed or was refused (message for the user).
    Failed(String),
}

/// Workspace state. Owned by the UI thread.
pub struct WorkspaceState {
    /// RibbonBar: the tab list (§7.2).
    pub ribbon: Vec<RibbonTab>,
    /// Active Ribbon tab ID (persisted).
    pub active_ribbon_tab: String,
    /// The quick access toolbar (QAT, ribbon-bar.md §4.1): a static product set reusing
    /// [`ToolbarItem`] (rendered at icon size), still visible when collapsed. User customization
    /// (add/remove) and its persistence are phase 2.
    pub qat: Vec<ToolbarItem>,
    /// Ribbon collapsed state (ribbon-bar.md §4.1); persisted in LayoutFile.ribbon_collapsed.
    pub ribbon_collapsed: bool,
    /// Whether the Ribbon can collapse (ribbon-bar.md §4.5); false (disabled) by default, the
    /// product turns it on through the builder. Gates rendering the collapse button, tab
    /// double-click and the Ctrl+F1 hotkey.
    pub ribbon_collapse_enabled: bool,
    /// About dialog open flag (set by the `app.about` command; the frontend renders the dialog
    /// and closes it).
    pub about_open: bool,
    /// Active settings text edit (`app.settings.edit_begin`): the key being
    /// edited and the edit buffer. Frontend renders an input overlay and
    /// routes keystrokes into the buffer while this is Some; Enter commits
    /// via `app.settings.set`, Esc discards.
    pub text_edit: Option<TextEdit>,
    /// The open in-app file picker (open/save for storage without an OS
    /// dialog); the frontend renders it while Some.
    pub file_picker: Option<crate::picker::FilePicker>,
    /// Login/session state (Phase 3 account integration); the status bar
    /// account entry renders from this.
    pub account: AccountPhase,
    pub panels: Vec<PanelSlot>,
    pub tabs: Vec<TabSlot>,
    pub active_tab: usize,
    /// The current work mode (a simplified Perspective: affects whether commands are enabled).
    pub work_mode: String,
    pub work_modes: Vec<String>,
    /// The default document type ID the product specifies (used with several document types;
    /// None = the first registered).
    pub default_document_type: Option<String>,
    /// The active panel of each area.
    pub area_active: HashMap<DockArea, PanelId>,

    view_defs: HashMap<String, ViewDef>,
    panel_defs: HashMap<String, PanelDef>,
    next_tab_instance: u64,
}

impl WorkspaceState {
    pub fn new(default_mode: impl Into<String>) -> Self {
        Self {
            ribbon: Vec::new(),
            active_ribbon_tab: String::new(),
            qat: Vec::new(),
            ribbon_collapsed: false,
            ribbon_collapse_enabled: false,
            about_open: false,
            text_edit: None,
            file_picker: None,
            account: AccountPhase::default(),
            panels: Vec::new(),
            tabs: Vec::new(),
            active_tab: 0,
            work_mode: default_mode.into(),
            work_modes: Vec::new(),
            default_document_type: None,
            area_active: HashMap::new(),
            view_defs: HashMap::new(),
            panel_defs: HashMap::new(),
            next_tab_instance: 1,
        }
    }

    // ---- Registration ----

    pub fn register_view(&mut self, def: ViewDef) -> Result<(), String> {
        if self.view_defs.contains_key(def.type_id.as_str()) {
            return Err(format!("Duplicate view type registration: {}", def.type_id));
        }
        self.view_defs.insert(def.type_id.0.clone(), def);
        Ok(())
    }

    pub fn register_panel(&mut self, def: PanelDef, visible_by_default: bool) -> Result<(), String> {
        if self.panel_defs.contains_key(def.id.as_str()) {
            return Err(format!("Duplicate panel registration: {}", def.id));
        }
        let slot = PanelSlot {
            id: def.id.clone(),
            title: def.title.clone(),
            icon: def.icon.clone(),
            script: def.script.clone(),
            area: def.area,
            order: def.order,
            size: def.default_size,
            visible: visible_by_default,
        };
        self.panel_defs.insert(def.id.0.clone(), def);
        self.area_active
            .entry(slot.area)
            .or_insert_with(|| slot.id.clone());
        self.panels.push(slot);
        self.panels.sort_by_key(|p| p.order);
        Ok(())
    }

    // ---- RibbonBar ----

    /// Compatibility entry (the toolbar-era API): the group lands in the platform default tab
    /// "Home". Groups with the same ID merge their items within the tab (controlled
    /// contributions, §7.3).
    pub fn add_toolbar_group(&mut self, group: ToolbarGroup) {
        self.add_group_to_tab(RibbonTab::DEFAULT_TAB_ID, "Home", group);
    }

    /// Register/merge a Ribbon tab: tabs with the same ID merge their groups (groups with the
    /// same ID merge their items).
    pub fn add_ribbon_tab(&mut self, tab: RibbonTab) {
        if let Some(existing) = self.ribbon.iter_mut().find(|t| t.id == tab.id) {
            for group in tab.groups {
                Self::merge_group(&mut existing.groups, group);
            }
        } else {
            self.ribbon.push(tab);
        }
        if self.active_ribbon_tab.is_empty() {
            self.active_ribbon_tab = self.ribbon[0].id.0.clone();
        }
    }

    /// Switch the active Ribbon tab (stable ID).
    pub fn set_active_ribbon_tab(&mut self, id: &str) {
        if self.ribbon.iter().any(|t| t.id.0 == id) {
            self.active_ribbon_tab = id.to_string();
        }
    }

    /// The active tab's group list; empty when there are no tabs.
    pub fn active_ribbon(&self) -> Option<&RibbonTab> {
        self.ribbon
            .iter()
            .find(|t| t.id.0 == self.active_ribbon_tab)
            .or_else(|| self.ribbon.first())
    }

    fn add_group_to_tab(&mut self, tab_id: &str, tab_title: &str, group: ToolbarGroup) {
        if let Some(tab) = self.ribbon.iter_mut().find(|t| t.id.0 == tab_id) {
            Self::merge_group(&mut tab.groups, group);
        } else {
            self.ribbon.push(
                RibbonTab::new(tab_id, tab_title).groups(vec![group]),
            );
            if self.active_ribbon_tab.is_empty() {
                self.active_ribbon_tab = tab_id.to_string();
            }
        }
    }

    fn merge_group(groups: &mut Vec<ToolbarGroup>, group: ToolbarGroup) {
        if let Some(existing) = groups.iter_mut().find(|g| g.id == group.id) {
            existing.items.extend(group.items);
        } else {
            groups.push(group);
        }
    }

    pub fn set_work_modes(&mut self, modes: Vec<String>, default: impl Into<String>) {
        self.work_modes = modes;
        self.work_mode = default.into();
    }

    // ---- Central tabs ----

    pub fn view_def(&self, type_id: &ViewTypeId) -> Option<&ViewDef> {
        self.view_defs.get(type_id.as_str())
    }

    pub fn panel_def(&self, id: &PanelId) -> Option<&PanelDef> {
        self.panel_defs.get(id.as_str())
    }

    pub fn view_type_ids(&self) -> impl Iterator<Item = &ViewTypeId> {
        self.view_defs.values().map(|d| &d.type_id)
    }

    /// Open a central view; singleton types reuse the existing tab.
    /// `bind_document` decides whether the new tab is bound to the active document.
    pub fn open_view(
        &mut self,
        type_id: &ViewTypeId,
        bind_document: Option<DocumentId>,
        title_override: Option<String>,
    ) -> Result<TabInstanceId, String> {
        let def = self
            .view_defs
            .get(type_id.as_str())
            .ok_or_else(|| format!("Unregistered view type: {type_id}"))?;
        if def.singleton {
            if let Some(slot) = self.tabs.iter_mut().find(|t| t.type_id == *type_id) {
                let id = slot.instance_id;
                // Document-bound view: rebind the singleton tab to the current document
                if let Some(doc) = bind_document {
                    slot.document = Some(doc);
                }
                self.activate_tab_by_instance(id);
                return Ok(id);
            }
        }
        let instance_id = TabInstanceId(self.next_tab_instance);
        self.next_tab_instance += 1;
        let title = title_override.unwrap_or_else(|| def.title.clone());
        self.tabs.push(TabSlot {
            instance_id,
            type_id: type_id.clone(),
            title,
            icon: def.icon.clone(),
            script: def.script.clone(),
            document: bind_document,
        });
        self.active_tab = self.tabs.len() - 1;
        Ok(instance_id)
    }

    pub fn close_tab(&mut self, index: usize) -> Option<TabSlot> {
        if index >= self.tabs.len() {
            return None;
        }
        let removed = self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }
        Some(removed)
    }

    pub fn close_tab_by_instance(&mut self, instance_id: TabInstanceId) -> bool {
        if let Some(i) = self.tabs.iter().position(|t| t.instance_id == instance_id) {
            self.close_tab(i).is_some()
        } else {
            false
        }
    }

    pub fn activate_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    pub fn activate_tab_by_instance(&mut self, instance_id: TabInstanceId) {
        if let Some(i) = self.tabs.iter().position(|t| t.instance_id == instance_id) {
            self.active_tab = i;
        }
    }

    pub fn active_tab(&self) -> Option<&TabSlot> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabSlot> {
        self.tabs.get_mut(self.active_tab)
    }

    // ---- Panels ----

    pub fn panels_in_area(&self, area: DockArea) -> Vec<&PanelSlot> {
        let mut list: Vec<&PanelSlot> = self.panels.iter().filter(|p| p.area == area).collect();
        list.sort_by_key(|p| p.order);
        list
    }

    pub fn toggle_panel(&mut self, id: &PanelId) -> Result<bool, String> {
        let slot = self
            .panels
            .iter_mut()
            .find(|p| &p.id == id)
            .ok_or_else(|| format!("Unregistered panel: {id}"))?;
        slot.visible = !slot.visible;
        if slot.visible {
            self.area_active.insert(slot.area, slot.id.clone());
        }
        Ok(slot.visible)
    }

    pub fn active_panel_in(&self, area: DockArea) -> Option<&PanelSlot> {
        let active_id = self.area_active.get(&area)?;
        self.panels
            .iter()
            .find(|p| p.area == area && &p.id == active_id && p.visible)
    }

    // ---- Modes ----

    /// Ribbon-side cleanup when a plugin is disabled/unloaded (design.md §16.2):
    /// removes the items of the removed commands from every group, and drops the groups and whole
    /// tabs that source contributed once they are empty.
    pub fn remove_commands(&mut self, removed: &[String], source: &str) {
        for tab in &mut self.ribbon {
            for group in &mut tab.groups {
                group.items.retain(|it| !removed.contains(&it.command.0));
            }
            tab.groups.retain(|g| {
                !(g.items.is_empty() && g.source.as_deref() == Some(source))
            });
        }
        let before = self.ribbon.len();
        self.ribbon
            .retain(|t| !(t.groups.is_empty() && t.source.as_deref() == Some(source)));
        if self.ribbon.len() != before && self.active_ribbon_tab_is_gone() {
            if let Some(first) = self.ribbon.first() {
                self.active_ribbon_tab = first.id.0.clone();
            }
        }
    }

    fn active_ribbon_tab_is_gone(&self) -> bool {
        !self.ribbon.iter().any(|t| t.id.0 == self.active_ribbon_tab)
    }

    /// Remove a plugin's contributions from the workspace model (design.md
    /// §16.2): its view types, panel definitions and slots, and the ribbon
    /// tabs it contributed.
    ///
    /// Tabs whose view type disappeared are closed; panels whose definition
    /// disappeared are dropped.
    pub fn remove_from_source(&mut self, source: &str) {
        self.view_defs
            .retain(|_, def| def.source.as_deref() != Some(source));
        self.panel_defs
            .retain(|_, def| def.source.as_deref() != Some(source));

        let live_panels: std::collections::HashSet<String> =
            self.panel_defs.keys().cloned().collect();
        self.panels.retain(|slot| live_panels.contains(&slot.id.0));

        let live_views: std::collections::HashSet<String> =
            self.view_defs.keys().cloned().collect();
        self.tabs.retain(|tab| live_views.contains(&tab.type_id.0));

        // Ribbon tabs this source contributed, and the groups inside shared
        // tabs, go with it.
        self.ribbon
            .retain(|tab| tab.source.as_deref() != Some(source));
        for tab in &mut self.ribbon {
            tab.groups
                .retain(|group| group.source.as_deref() != Some(source));
        }
        self.ribbon.retain(|tab| !tab.groups.is_empty());
        if self.active_ribbon_tab_is_gone() {
            if let Some(first) = self.ribbon.first() {
                self.active_ribbon_tab = first.id.0.clone();
            }
        }
    }

    pub fn set_work_mode(&mut self, mode: impl Into<String>) {
        self.work_mode = mode.into();
    }

    // ---- Layout persistence ----

    pub fn save_layout(&self) -> LayoutFile {
        LayoutFile {
            version: LAYOUT_VERSION,
            work_mode: self.work_mode.clone(),
            active_tab: self.active_tab,
            panels: self
                .panels
                .iter()
                .map(|p| SavedPanel {
                    id: p.id.0.clone(),
                    area: p.area,
                    order: p.order,
                    size: p.size,
                    visible: p.visible,
                })
                .collect(),
            tabs: self
                .tabs
                .iter()
                .map(|t| SavedTab {
                    type_id: t.type_id.0.clone(),
                    title: t.title.clone(),
                    bind_document: t.document.is_some(),
                })
                .collect(),
            area_active: self
                .area_active
                .iter()
                .map(|(a, p)| (*a, p.0.clone()))
                .collect(),
            ribbon_active_tab: Some(self.active_ribbon_tab.clone()),
            ribbon_collapsed: Some(self.ribbon_collapsed),
            ribbon_overflow: None,
            ui: None,
            dock: None,
        }
    }

    /// Apply a layout: skips unregistered panels/views (the caller records the diagnostics);
    /// missing entries keep their default state.
    pub fn apply_layout(&mut self, file: &LayoutFile) -> Vec<String> {
        let mut skipped = Vec::new();
        // Panels
        for sp in &file.panels {
            if let Some(slot) = self.panels.iter_mut().find(|p| p.id.0 == sp.id) {
                slot.area = sp.area;
                slot.order = sp.order;
                slot.size = sp.size.max(80.0);
                slot.visible = sp.visible;
            } else {
                skipped.push(format!("Panel `{}` is not registered; skipped", sp.id));
            }
        }
        // Active panel of each area
        for (area, pid) in &file.area_active {
            if self.panels.iter().any(|p| &p.id.0 == pid && p.area == *area) {
                self.area_active.insert(*area, PanelId::new(pid.clone()));
            }
        }
        // Central tabs
        let saved_active = file.active_tab;
        self.tabs.clear();
        self.active_tab = 0;
        for st in &file.tabs {
            match self.view_defs.get(st.type_id.as_str()) {
                Some(def) => {
                    let instance_id = TabInstanceId(self.next_tab_instance);
                    self.next_tab_instance += 1;
                    self.tabs.push(TabSlot {
                        instance_id,
                        type_id: def.type_id.clone(),
                        title: st.title.clone(),
                        icon: def.icon.clone(),
                        script: def.script.clone(),
                        // Documents are not restored across sessions; the workbench rebinds
                        // them when needed
                        document: None,
                    });
                }
                None => skipped.push(format!(
                    "View type `{}` is not registered; skipped",
                    st.type_id
                )),
            }
        }
        self.active_tab = saved_active.min(self.tabs.len().saturating_sub(1));
        // A mode the product no longer has (renamed, or from another product)
        // keeps the current one, like an unregistered panel or tab.
        if self.work_modes.is_empty() || self.work_modes.contains(&file.work_mode) {
            self.work_mode = file.work_mode.clone();
        } else {
            skipped.push(format!(
                "Work mode `{}` is not registered; kept `{}`",
                file.work_mode, self.work_mode
            ));
        }
        // Restore the ribbon collapse state (defaults to expanded when absent).
        self.ribbon_collapsed = file.ribbon_collapsed.unwrap_or(false);
        // Active Ribbon tab: an unregistered tab falls back to the first tab (§6.5: stale entries
        // are diagnosable)
        match &file.ribbon_active_tab {
            Some(id) if self.ribbon.iter().any(|t| t.id.0 == *id) => {
                self.active_ribbon_tab = id.clone();
            }
            Some(id) if !self.ribbon.is_empty() => {
                skipped.push(format!(
                    "RibbonTab `{id}` is not registered; fell back to the first tab"
                ));
                self.active_ribbon_tab = self.ribbon[0].id.0.clone();
            }
            _ => {}
        }
        skipped
    }
}

#[cfg(test)]
mod tests {
    use super::TextEdit;

    #[test]
    fn a_setting_edit_commits_through_the_settings_command() {
        let edit = TextEdit::setting("ui.accent", "#336699");
        assert_eq!(edit.commit_command(), "app.settings.set");
        assert_eq!(edit.caption(), "ui.accent");
    }

    #[test]
    fn a_prompt_carries_its_command_and_caption() {
        let edit = TextEdit::prompt("3/width", "12", "rcad.part.set_property", "Box · width");
        assert_eq!(edit.commit_command(), "rcad.part.set_property");
        assert_eq!(edit.caption(), "Box · width");
        assert_eq!(edit.key, "3/width");
        assert_eq!(edit.buffer, "12");
    }

    #[test]
    fn a_layout_with_an_unknown_work_mode_keeps_the_current_one() {
        let mut ws = super::WorkspaceState::new("Modeling");
        ws.set_work_modes(vec!["Modeling".into(), "Sketch".into()], "Modeling");
        let mut file = ws.save_layout();

        file.work_mode = "Sketch".into();
        assert!(ws.apply_layout(&file).is_empty());
        assert_eq!(ws.work_mode, "Sketch");

        // A layout saved before the modes were renamed.
        file.work_mode = "Retired".into();
        let skipped = ws.apply_layout(&file);
        assert_eq!(ws.work_mode, "Sketch");
        assert!(skipped.iter().any(|s| s.contains("Work mode")), "{skipped:?}");
    }
}
