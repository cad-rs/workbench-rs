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

// ---- 工具栏（RibbonBar 的简化替身：Group → Items，与 Ribbon 同构，升级只需加 Tab 维度）----

#[derive(Clone, Debug)]
pub struct ToolbarItem {
    /// 绑定的命令稳定 ID。
    pub command: crate::ids::CommandId,
    /// 显示文本；None 时回退到命令标题。
    pub label: Option<String>,
}

impl ToolbarItem {
    pub fn command(command: impl Into<crate::ids::CommandId>) -> Self {
        Self { command: command.into(), label: None }
    }
    pub fn labeled(command: impl Into<crate::ids::CommandId>, label: impl Into<String>) -> Self {
        Self { command: command.into(), label: Some(label.into()) }
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
    /// 是否单例：同类型同时只开一个标签。
    pub singleton: bool,
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
            singleton,
            factory: Box::new(factory),
        }
    }
}

/// Dock Panel 注册。
pub struct PanelDef {
    pub id: PanelId,
    pub title: String,
    pub area: DockArea,
    /// 默认尺寸：左右为宽度，上下为高度（逻辑像素）。
    pub default_size: f32,
    pub order: i32,
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
            area,
            default_size,
            order,
            factory: Box::new(factory),
        }
    }
}

/// 面板运行槽位：定义 + 布局状态 + 实例。
pub struct PanelSlot {
    pub id: PanelId,
    pub title: String,
    pub area: DockArea,
    pub order: i32,
    pub size: f32,
    pub visible: bool,
    pub instance: Option<Box<dyn ViewInstance>>,
}

/// 中央标签槽位。
pub struct TabSlot {
    pub instance_id: TabInstanceId,
    pub type_id: ViewTypeId,
    pub title: String,
    /// 绑定的文档（文档视图）。
    pub document: Option<DocumentId>,
    pub instance: Option<Box<dyn ViewInstance>>,
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
}

impl RibbonTab {
    pub fn new(id: impl Into<StableId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            groups: Vec::new(),
            source: None,
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
    /// GUI 前端私有的物理布局（如 egui_tiles 树）。无 GUI 语义，核心不解析。
    #[serde(default)]
    pub ui: Option<serde_json::Value>,
}

/// 工作区状态。UI 线程所有。
pub struct WorkspaceState {
    /// RibbonBar：Tab 列表（§7.2）。
    pub ribbon: Vec<RibbonTab>,
    /// 活动 Ribbon Tab ID（持久化）。
    pub active_ribbon_tab: String,
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
            area: def.area,
            order: def.order,
            size: def.default_size,
            visible: visible_by_default,
            instance: Some((def.factory)()),
        };
        self.panel_defs.insert(def.id.0.clone(), def);
        if !self.area_active.contains_key(&slot.area) {
            self.area_active.insert(slot.area, slot.id.clone());
        }
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
        let instance = Some((def.factory)());
        let instance_id = TabInstanceId(self.next_tab_instance);
        self.next_tab_instance += 1;
        let title = title_override.unwrap_or_else(|| def.title.clone());
        self.tabs.push(TabSlot {
            instance_id,
            type_id: type_id.clone(),
            title,
            document: bind_document,
            instance,
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
            ui: None,
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
                    let instance = Some((def.factory)());
                    let instance_id = TabInstanceId(self.next_tab_instance);
                    self.next_tab_instance += 1;
                    self.tabs.push(TabSlot {
                        instance_id,
                        type_id: def.type_id.clone(),
                        title: st.title.clone(),
                        document: None, // 文档不跨会话恢复；需要时由工作台重新绑定
                        instance,
                    });
                }
                None => skipped.push(format!("视图类型 `{}` 未注册，跳过", st.type_id)),
            }
        }
        self.active_tab = saved_active.min(self.tabs.len().saturating_sub(1));
        self.work_mode = file.work_mode.clone();
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
