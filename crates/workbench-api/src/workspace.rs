//! 工作区模型（design.md §6）：工具栏、Dock Panel、中央多标签内容区与布局持久化。
//!
//! 该模型完全 GUI 无关；egui/gpui 前端负责把它渲染成具体控件。
//! 布局持久化只使用稳定 ID；恢复时跳过未注册的面板/视图并记录诊断。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::{DocumentId, PanelId, StableId, TabInstanceId, ToolbarGroupId, ViewTypeId};
use crate::view::ViewInstance;

/// 停靠区域。
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

// ---- 工具栏（RibbonBar：Tab → Group → Items，稳定 ID + 命令绑定 + 按钮形态，ribbon-bar.md §2.4/§6）----

/// Ribbon 条目形态（ribbon-bar.md §2.4）。默认（`Default` 与 serde）为
/// [`RibbonItemKind::Command`]，旧数据与旧注册代码零改动兼容。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RibbonItemKind {
    /// 普通命令按钮：点击执行命令（默认形态，等价现状）。
    Command,
    /// 开关按钮：点击只发命令，checked 态由命令系统查询（`is_checked`）。
    Toggle,
    /// 分裂按钮：主区执行默认命令，箭头区弹出子命令菜单。
    Split {
        /// 下拉子命令条目。
        items: Vec<ToolbarItem>,
    },
    /// 纯菜单按钮：整体只弹出菜单，不直接执行。
    Menu {
        /// 菜单条目。
        items: Vec<ToolbarItem>,
    },
    /// 选择框（select-only）：当前值与选项由命令系统查询（`combo_state`），
    /// 选中执行命令并经 [`CommandArgs::value`](crate::command::CommandArgs) 带值传入。
    Combo,
}

impl Default for RibbonItemKind {
    fn default() -> Self {
        RibbonItemKind::Command
    }
}

/// Ribbon 条目尺寸档（ribbon-bar.md §2.2）。默认（`Default` 与 serde）为
/// [`ItemSize::Small`]（与现状视觉最接近）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ItemSize {
    /// 大按钮：24px 图标上置 + 标签下置，独占一列。
    Large,
    /// 小按钮：16px 图标 + 标签同行（默认档）。
    Small,
    /// 仅图标：无标签（QAT 与图标自明命令；图标缺失时回退标签首字占位）。
    Icon,
}

impl Default for ItemSize {
    fn default() -> Self {
        ItemSize::Small
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolbarItem {
    /// 绑定的命令稳定 ID。
    pub command: crate::ids::CommandId,
    /// 显示文本；None 时回退到命令标题。
    pub label: Option<String>,
    /// 按钮形态；缺省为 [`RibbonItemKind::Command`]（向后兼容）。
    #[serde(default)]
    pub kind: RibbonItemKind,
    /// 尺寸档；缺省为 [`ItemSize::Small`]（向后兼容）。
    #[serde(default)]
    pub size: ItemSize,
    /// Combo 声明宽度（逻辑像素）；None 时用组件默认宽度。
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
    /// Toggle 形态（checked 态由命令系统查询）。
    pub fn toggle(mut self) -> Self {
        self.kind = RibbonItemKind::Toggle;
        self
    }
    /// Split 形态：主按钮执行默认命令，箭头区弹出 `items` 子命令菜单。
    pub fn split(mut self, items: Vec<ToolbarItem>) -> Self {
        self.kind = RibbonItemKind::Split { items };
        self
    }
    /// Menu 形态：整体弹出 `items` 菜单，不直接执行。
    pub fn menu(mut self, items: Vec<ToolbarItem>) -> Self {
        self.kind = RibbonItemKind::Menu { items };
        self
    }
    /// Combo 形态：选项/当前值由命令系统查询（`combo_state`）。
    pub fn combo(mut self) -> Self {
        self.kind = RibbonItemKind::Combo;
        self
    }
    /// 大按钮档。
    pub fn large(mut self) -> Self {
        self.size = ItemSize::Large;
        self
    }
    /// 仅图标档（无标签）。
    pub fn icon_only(mut self) -> Self {
        self.size = ItemSize::Icon;
        self
    }
    /// 声明宽度（Combo 选择框等；逻辑像素）。
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
    /// 贡献来源（Workbench/插件 ID）；插件卸载清理用。
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
    /// 标记贡献来源。
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
    /// 设置条目（builder 风格）。
    pub fn items(mut self, items: Vec<ToolbarItem>) -> Self {
        self.items = items;
        self
    }
}

// ---- 视图 / 面板定义 ----

/// 创建视图实例的工厂。
pub type ViewFactory = Box<dyn Fn() -> Box<dyn ViewInstance> + Send + Sync>;

/// 中央内容视图类型注册。
pub struct ViewDef {
    pub type_id: ViewTypeId,
    pub title: String,
    /// 图标资源路径（如 `"icons/canvas.svg"`，由产品的 AssetSource 解析）；
    /// 中央标签展示用，None 时回退为纯文本。
    pub icon: Option<String>,
    /// 是否单例：同类型同时只开一个标签。
    pub singleton: bool,
    /// Script central view: gpui-rhai script file path (mounted by the gpui
    /// frontend). When set the tab content is fully defined by the script and
    /// `factory` is never called.
    pub script: Option<String>,
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
            source: None,
            factory: Box::new(factory),
        }
    }
    /// 设置图标资源路径（中央标签展示用）。
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
    /// Attribute this view type to a plugin, so disabling the plugin removes it.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Dock Panel 注册。
pub struct PanelDef {
    pub id: PanelId,
    pub title: String,
    /// 图标资源路径（如 `"icons/explorer.svg"`，由产品的 AssetSource 解析）；
    /// 面板头展示用，None 时回退为纯文本。
    pub icon: Option<String>,
    /// 脚本面板：gpui-rhai 脚本文件路径（由 gpui 前端挂载渲染）。
    /// 设置后面板内容完全由脚本定义，`factory` 不再被调用。
    pub script: Option<String>,
    pub area: DockArea,
    /// 默认尺寸：左右为宽度，上下为高度（逻辑像素）。
    pub default_size: f32,
    pub order: i32,
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
            source: None,
            factory: Box::new(factory),
        }
    }
    /// 设置图标资源路径（面板头展示用）。
    pub fn icon(mut self, path: impl Into<String>) -> Self {
        self.icon = Some(path.into());
        self
    }
    /// 脚本面板：内容由 gpui-rhai 脚本（返回 UiNode 树）定义。
    /// `path` 是脚本文件路径；`factory` 不会被调用（传一个占位即可）。
    pub fn script(mut self, path: impl Into<String>) -> Self {
        self.script = Some(path.into());
        self
    }
    /// Attribute this panel to a plugin, so disabling the plugin removes it.
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// 面板运行槽位：定义 + 布局状态 + 实例。
pub struct PanelSlot {
    pub id: PanelId,
    pub title: String,
    /// 图标资源路径（注册时从 [`PanelDef`] 复制）。
    pub icon: Option<String>,
    /// 脚本面板路径（注册时从 [`PanelDef`] 复制）；Some 时不创建 instance。
    pub script: Option<String>,
    pub area: DockArea,
    pub order: i32,
    pub size: f32,
    pub visible: bool,
}

/// 中央标签槽位。
pub struct TabSlot {
    pub instance_id: TabInstanceId,
    pub type_id: ViewTypeId,
    pub title: String,
    /// 图标资源路径（打开时从 [`ViewDef`] 复制）。
    pub icon: Option<String>,
    /// Script path copied from the [`ViewDef`] when the tab was opened; `Some`
    /// means the frontend mounts the script instead of a Rust instance.
    pub script: Option<String>,
    /// 绑定的文档（文档视图）。
    pub document: Option<DocumentId>,
}

// ---- RibbonBar（design.md §7：Tab → Group → Item，稳定 ID + 命令绑定）----

/// Ribbon Tab。Group 复用 [`ToolbarGroup`](crate::ToolbarGroup)（稳定 ID、条目、来源标记）。
#[derive(Clone, Debug)]
pub struct RibbonTab {
    pub id: StableId,
    pub title: String,
    pub groups: Vec<ToolbarGroup>,
    /// 贡献来源（Workbench/插件 ID）。
    pub source: Option<String>,
    /// 上下文标记（ribbon-bar.md §4.7）：空 = 常驻 Tab；
    /// 含当前 work_mode 名时该模式下可见（渲染层过滤）。
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
    /// 平台默认 Tab（兼容入口：`add_toolbar_group` 的组落入此处）。
    pub const DEFAULT_TAB_ID: &'static str = "app.home";
}

// ---- 布局持久化 ----

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
    /// 恢复时是否绑定默认文档。
    pub bind_document: bool,
}

/// 布局文件（design.md §6.5）。`ui` 字段留给 GUI 前端保存各自的物理布局细节。
#[derive(Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub work_mode: String,
    pub active_tab: usize,
    pub panels: Vec<SavedPanel>,
    pub tabs: Vec<SavedTab>,
    /// 各停靠区域的活动面板 ID。
    pub area_active: Vec<(DockArea, String)>,
    /// 活动 Ribbon Tab（稳定 ID；旧布局文件无此字段时回落首个 Tab）。
    #[serde(default)]
    pub ribbon_active_tab: Option<String>,
    /// Ribbon 折叠态（ribbon-bar.md §4.1）；None = 未记录。
    /// 折叠态持久化由前端 bridge 接线（后续里程碑），核心不消费。
    #[serde(default)]
    pub ribbon_collapsed: Option<bool>,
    /// Ribbon 空间不足策略（ribbon-bar.md §4.5）："scroll"（默认）| "collapse"；
    /// None = 未记录，核心不消费。
    #[serde(default)]
    pub ribbon_overflow: Option<String>,
    /// GUI 前端私有的物理布局（如 egui_tiles 树）。无 GUI 语义，核心不解析。
    #[serde(default)]
    pub ui: Option<serde_json::Value>,
    /// 停靠区的物理布局（gpui 前端的 `DockArea::dump` 状态：各停靠区的尺寸、
    /// 分组与标签顺序）。同样无 GUI 语义、核心不解析；恢复时由前端 `load`。
    #[serde(default)]
    pub dock: Option<serde_json::Value>,
}

/// 工作区状态。UI 线程所有。
pub struct WorkspaceState {
    /// RibbonBar：Tab 列表（§7.2）。
    pub ribbon: Vec<RibbonTab>,
    /// 活动 Ribbon Tab ID（持久化）。
    pub active_ribbon_tab: String,
    /// QAT 快速访问工具条（ribbon-bar.md §4.1）：产品静态集，复用 [`ToolbarItem`]
    /// （渲染取 Icon 档），折叠态仍可见。用户自定义增删与持久化二期。
    pub qat: Vec<ToolbarItem>,
    /// Ribbon 折叠态（ribbon-bar.md §4.1）；持久化于 LayoutFile.ribbon_collapsed。
    pub ribbon_collapsed: bool,
    /// Ribbon 折叠能力开关（ribbon-bar.md §4.5）；默认 false（禁用），
    /// 产品经 builder 开启。门控折叠钮渲染、Tab 双击与 Ctrl+F1 快捷键。
    pub ribbon_collapse_enabled: bool,
    pub panels: Vec<PanelSlot>,
    pub tabs: Vec<TabSlot>,
    pub active_tab: usize,
    /// 当前工作模式（Perspective 的简化：影响命令启用状态）。
    pub work_mode: String,
    pub work_modes: Vec<String>,
    /// 各区域的活动面板。
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
            panels: Vec::new(),
            tabs: Vec::new(),
            active_tab: 0,
            work_mode: default_mode.into(),
            work_modes: Vec::new(),
            area_active: HashMap::new(),
            view_defs: HashMap::new(),
            panel_defs: HashMap::new(),
            next_tab_instance: 1,
        }
    }

    // ---- 注册 ----

    pub fn register_view(&mut self, def: ViewDef) -> Result<(), String> {
        if self.view_defs.contains_key(def.type_id.as_str()) {
            return Err(format!("视图类型重复注册: {}", def.type_id));
        }
        self.view_defs.insert(def.type_id.0.clone(), def);
        Ok(())
    }

    pub fn register_panel(&mut self, def: PanelDef, visible_by_default: bool) -> Result<(), String> {
        if self.panel_defs.contains_key(def.id.as_str()) {
            return Err(format!("面板重复注册: {}", def.id));
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

    /// 兼容入口（工具栏时代 API）：组落入平台默认 Tab「主页」。
    /// 同 ID 组在 Tab 内合并条目（受控贡献，§7.3）。
    pub fn add_toolbar_group(&mut self, group: ToolbarGroup) {
        self.add_group_to_tab(RibbonTab::DEFAULT_TAB_ID, "主页", group);
    }

    /// 注册/合并一个 Ribbon Tab：同 ID Tab 合并其组（组内同 ID 合并条目）。
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

    /// 切换活动 Ribbon Tab（稳定 ID）。
    pub fn set_active_ribbon_tab(&mut self, id: &str) {
        if self.ribbon.iter().any(|t| t.id.0 == id) {
            self.active_ribbon_tab = id.to_string();
        }
    }

    /// 当前活动 Tab 的组列表；无 Tab 时为空。
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

    // ---- 中央标签 ----

    pub fn view_def(&self, type_id: &ViewTypeId) -> Option<&ViewDef> {
        self.view_defs.get(type_id.as_str())
    }

    pub fn panel_def(&self, id: &PanelId) -> Option<&PanelDef> {
        self.panel_defs.get(id.as_str())
    }

    pub fn view_type_ids(&self) -> impl Iterator<Item = &ViewTypeId> {
        self.view_defs.values().map(|d| &d.type_id)
    }

    /// 打开一个中央视图；singleton 类型复用已有标签。
    /// `bind_document` 决定新标签是否绑定当前活动文档。
    pub fn open_view(
        &mut self,
        type_id: &ViewTypeId,
        bind_document: Option<DocumentId>,
        title_override: Option<String>,
    ) -> Result<TabInstanceId, String> {
        let def = self
            .view_defs
            .get(type_id.as_str())
            .ok_or_else(|| format!("未注册的视图类型: {type_id}"))?;
        if def.singleton {
            if let Some(slot) = self.tabs.iter_mut().find(|t| t.type_id == *type_id) {
                let id = slot.instance_id;
                // 文档绑定视图：切换单例标签的文档绑定到当前文档
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

    // ---- 面板 ----

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
            .ok_or_else(|| format!("未注册的面板: {id}"))?;
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

    // ---- 模式 ----

    /// 插件禁用/卸载的 Ribbon 侧清理（design.md §16.2）：
    /// 从所有组摘除被移除命令的条目，清掉该来源贡献的空组与整个 Tab。
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

    // ---- 布局持久化 ----

    pub fn save_layout(&self) -> LayoutFile {
        LayoutFile {
            version: 1,
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

    /// 应用布局：跳过未注册的面板/视图（诊断由调用方记录），缺失项保留默认状态。
    pub fn apply_layout(&mut self, file: &LayoutFile) -> Vec<String> {
        let mut skipped = Vec::new();
        // 面板
        for sp in &file.panels {
            if let Some(slot) = self.panels.iter_mut().find(|p| p.id.0 == sp.id) {
                slot.area = sp.area;
                slot.order = sp.order;
                slot.size = sp.size.max(80.0);
                slot.visible = sp.visible;
            } else {
                skipped.push(format!("面板 `{}` 未注册，跳过", sp.id));
            }
        }
        // 区域活动面板
        for (area, pid) in &file.area_active {
            if self.panels.iter().any(|p| &p.id.0 == pid && p.area == *area) {
                self.area_active.insert(*area, PanelId::new(pid.clone()));
            }
        }
        // 中央标签
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
                        document: None, // 文档不跨会话恢复；需要时由工作台重新绑定
                    });
                }
                None => skipped.push(format!("视图类型 `{}` 未注册，跳过", st.type_id)),
            }
        }
        self.active_tab = saved_active.min(self.tabs.len().saturating_sub(1));
        self.work_mode = file.work_mode.clone();
        // Restore the ribbon collapse state (defaults to expanded when absent).
        self.ribbon_collapsed = file.ribbon_collapsed.unwrap_or(false);
        // 活动 Ribbon Tab：未注册的 Tab 回落首个 Tab（§6.5 失效项可诊断）
        match &file.ribbon_active_tab {
            Some(id) if self.ribbon.iter().any(|t| t.id.0 == *id) => {
                self.active_ribbon_tab = id.clone();
            }
            Some(id) if !self.ribbon.is_empty() => {
                skipped.push(format!("RibbonTab `{id}` 未注册，回落首个 Tab"));
                self.active_ribbon_tab = self.ribbon[0].id.0.clone();
            }
            _ => {}
        }
        skipped
    }
}
