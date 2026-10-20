//! 注册表：Workbench 模块贡献的命令、工具栏、面板、视图、文档类型与服务（design.md §8）。
//!
//! 重复 ID 等注册错误不会 panic，而是记录到 `errors`（可诊断的注册结果，design.md §7.3）。

use std::any::{Any, TypeId};

use crate::command::CommandDef;
use crate::documents::DocumentTypeDef;
use crate::ids::WorkbenchId;
use crate::workspace::{PanelDef, ToolbarGroup, ToolbarItem, ViewDef};

/// 注册表：产品装配阶段由 Workbench.init 填充，运行期只读。
#[derive(Default)]
pub struct Registry {
    pub commands: Vec<CommandDef>,
    pub toolbar: Vec<ToolbarGroup>,
    pub ribbon: Vec<crate::workspace::RibbonTab>,
    /// QAT 快速访问（产品静态集，ribbon-bar.md §4.1）：装配期暂存，
    /// build 时转入 [`WorkspaceState::qat`](crate::workspace::WorkspaceState)。
    pub qat: Vec<ToolbarItem>,
    pub panels: Vec<PanelDef>,
    pub views: Vec<ViewDef>,
    pub doc_types: Vec<DocumentTypeDef>,
    services: Vec<(TypeId, Box<dyn Any + Send + Sync>)>,
    /// 装配期间的诊断信息（重复注册、非法参数等）。
    pub errors: Vec<String>,
    /// 装配期间的提示信息（如插件加载日志）。
    pub notes: Vec<String>,
    /// 已装配的 Workbench。
    pub workbenches: Vec<WorkbenchId>,
    /// 已成功装载的插件 ID（诊断与测试查询）。
    pub plugins_loaded: Vec<String>,
    /// 每个已装载插件声明的模块 ID：(插件 ID, 模块 ID 列表)。
    ///
    /// 插件是运行期单位，不"包含"代码——它启用产品已编译的模块。产品用
    /// [`Registry::plugin_module_ids`] 拿到去重后的集合去装配。
    pub plugin_modules: Vec<(String, Vec<String>)>,
    /// 被拒绝/失败的插件：(插件 ID 或目录, 原因)。
    pub plugins_rejected: Vec<(String, String)>,
    /// 运行时被禁用的插件 ID（阶段 5）。
    pub plugins_disabled: Vec<String>,
    /// Workbench 建议的工作模式与默认模式（空则用平台默认）。
    pub work_modes: Vec<String>,
    pub default_mode: String,
    /// 当前正在注册贡献的来源插件（见 [`Registry::with_source`]）。
    current_source: Option<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    /// Run `f` with `source` stamped on every contribution it registers.
    ///
    /// The platform wraps a plugin's module loading in this so that disabling
    /// the plugin later can find and remove exactly what its modules added.
    /// An explicitly set source wins over the stamp.
    pub fn with_source(&mut self, source: &str, f: impl FnOnce(&mut Self)) {
        let previous = self.current_source.take();
        self.current_source = Some(source.to_string());
        f(self);
        self.current_source = previous;
    }

    /// The source that contributions registered right now are attributed to.
    pub fn current_source(&self) -> Option<&str> {
        self.current_source.as_deref()
    }

    // ---- 命令 ----

    pub fn register_command(&mut self, mut def: CommandDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.commands.iter().any(|c| c.id == def.id) {
            self.error(format!("命令重复注册: {}（来自 {}）", def.id, def.title));
            return;
        }
        if let Some(hk) = def.hotkey {
            if let Some(other) = self.commands.iter().find(|c| c.hotkey == Some(hk)) {
                self.error(format!(
                    "快捷键 {} 冲突：{} 与 {} 都绑定",
                    hk, other.id, def.id
                ));
            }
        }
        self.commands.push(def);
    }

    /// 取出命令（核心执行器在执行期间临时移出，避免处理器重入借用冲突）。
    pub fn take_command(&mut self, id: &str) -> Option<CommandDef> {
        let pos = self.commands.iter().position(|c| c.id.0 == id)?;
        Some(self.commands.remove(pos))
    }

    /// 放回取出的命令（执行完成后调用）。
    pub fn put_command(&mut self, def: CommandDef) {
        self.commands.push(def);
    }

    pub fn command(&self, id: &str) -> Option<&CommandDef> {
        self.commands.iter().find(|c| c.id.0 == id)
    }

    /// 按来源移除命令（插件禁用/卸载时清理，design.md §16.2）。
    /// 同时从工具栏组摘除其条目并清理由该来源引入的空组。返回移除的命令 ID。
    pub fn remove_commands_from_source(&mut self, source: &str) -> Vec<String> {
        let removed: Vec<String> = self
            .commands
            .iter()
            .filter(|c| c.source.as_deref() == Some(source))
            .map(|c| c.id.0.clone())
            .collect();
        self.commands
            .retain(|c| c.source.as_deref() != Some(source));
        for group in &mut self.toolbar {
            group
                .items
                .retain(|it| !removed.contains(&it.command.0));
        }
        // 条目被摘空的、由该来源贡献的组一并移除
        self.toolbar
            .retain(|g| !(g.items.is_empty() && g.source.as_deref() == Some(source)));
        removed
    }

    pub fn commands(&self) -> impl Iterator<Item = &CommandDef> {
        self.commands.iter()
    }

    /// 所有已装载插件声明的模块 ID，按首次出现顺序去重。
    ///
    /// 产品用这份清单装配插件启用的 Workbench 模块；顺序不保证依赖关系，
    /// 产品需自行拓扑排序。
    pub fn plugin_module_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (_, modules) in &self.plugin_modules {
            for id in modules {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids
    }

    // ---- 工具栏 ----

    pub fn add_toolbar_group(&mut self, mut group: ToolbarGroup) {
        if group.source.is_none() {
            group.source = self.current_source.clone();
        }
        if let Some(existing) = self.toolbar.iter_mut().find(|g| g.id == group.id) {
            existing.items.extend(group.items);
        } else {
            self.toolbar.push(group);
        }
    }

    /// 注册/合并一个 Ribbon Tab（同 ID Tab 合并组，组内同 ID 合并条目）。
    pub fn add_ribbon_tab(&mut self, mut tab: crate::workspace::RibbonTab) {
        if tab.source.is_none() {
            tab.source = self.current_source.clone();
        }
        if let Some(existing) = self.ribbon.iter_mut().find(|t| t.id == tab.id) {
            for mut group in tab.groups {
                if group.source.is_none() {
                    group.source = self.current_source.clone();
                }
                if let Some(g) = existing.groups.iter_mut().find(|x| x.id == group.id) {
                    g.items.extend(group.items);
                } else {
                    existing.groups.push(group);
                }
            }
        } else {
            self.ribbon.push(tab);
        }
    }

    /// 把命令加入 QAT 快速访问集（产品静态集，ribbon-bar.md §4.1）：
    /// 渲染为 Tab 条内 Icon 档小按钮，折叠态仍可见。重复加入不去重（静态装配期声明）。
    pub fn add_qat_command(&mut self, cmd: impl Into<crate::ids::CommandId>) {
        self.qat.push(ToolbarItem::command(cmd));
    }

    // ---- 面板 / 视图 / 文档类型 ----

    pub fn register_panel(&mut self, mut def: PanelDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.panels.iter().any(|p| p.id == def.id) {
            self.error(format!("面板重复注册: {}", def.id));
            return;
        }
        self.panels.push(def);
    }

    pub fn register_view(&mut self, mut def: ViewDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.views.iter().any(|v| v.type_id == def.type_id) {
            self.error(format!("视图类型重复注册: {}", def.type_id));
            return;
        }
        self.views.push(def);
    }

    pub fn register_document_type(&mut self, mut def: DocumentTypeDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.doc_types.iter().any(|t| t.id == def.id) {
            self.error(format!("文档类型重复注册: {}", def.id));
            return;
        }
        self.doc_types.push(def);
    }

    /// 按来源移除贡献（插件禁用/卸载时清理，design.md §16.2）。
    ///
    /// 移除该来源注册的命令、视图类型、面板、文档类型、Ribbon Tab 与工具栏组。
    /// 返回被移除的命令 ID（调用方据此清理工作区模型）。
    pub fn remove_from_source(&mut self, source: &str) -> Vec<String> {
        let removed = self
            .commands
            .iter()
            .filter(|c| c.source.as_deref() == Some(source))
            .map(|c| c.id.0.clone())
            .collect();
        self.commands
            .retain(|c| c.source.as_deref() != Some(source));
        self.views
            .retain(|v| v.source.as_deref() != Some(source));
        self.panels
            .retain(|p| p.source.as_deref() != Some(source));
        self.doc_types
            .retain(|t| t.source.as_deref() != Some(source));
        self.ribbon
            .retain(|t| t.source.as_deref() != Some(source));
        self.toolbar
            .retain(|g| g.source.as_deref() != Some(source));
        removed
    }

    // ---- 扩展服务 ----

    pub fn add_service<T: Any + Send + Sync>(&mut self, value: T) {
        if self.services.iter().any(|(t, _)| *t == TypeId::of::<T>()) {
            self.error(format!(
                "扩展服务重复注册: {}",
                std::any::type_name::<T>()
            ));
            return;
        }
        self.services.push((TypeId::of::<T>(), Box::new(value)));
    }

    pub fn take_services(&mut self) -> Vec<(TypeId, Box<dyn Any + Send + Sync>)> {
        std::mem::take(&mut self.services)
    }
}
