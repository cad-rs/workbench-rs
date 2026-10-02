//! 注册表：Workbench 模块贡献的命令、工具栏、面板、视图、文档类型与服务（design.md §8）。
//!
//! 重复 ID 等注册错误不会 panic，而是记录到 `errors`（可诊断的注册结果，design.md §7.3）。

use std::any::{Any, TypeId};

use crate::command::CommandDef;
use crate::documents::DocumentTypeDef;
use crate::ids::WorkbenchId;
use crate::workspace::{PanelDef, ToolbarGroup, ViewDef};

/// 注册表：产品装配阶段由 Workbench.init 填充，运行期只读。
#[derive(Default)]
pub struct Registry {
    pub commands: Vec<CommandDef>,
    pub toolbar: Vec<ToolbarGroup>,
    pub ribbon: Vec<crate::workspace::RibbonTab>,
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
    /// 被拒绝/失败的插件：(插件 ID 或目录, 原因)。
    pub plugins_rejected: Vec<(String, String)>,
    /// 运行时被禁用的插件 ID（阶段 5）。
    pub plugins_disabled: Vec<String>,
    /// Workbench 建议的工作模式与默认模式（空则用平台默认）。
    pub work_modes: Vec<String>,
    pub default_mode: String,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    // ---- 命令 ----

    pub fn register_command(&mut self, def: CommandDef) {
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

    // ---- 工具栏 ----

    pub fn add_toolbar_group(&mut self, group: ToolbarGroup) {
        if let Some(existing) = self.toolbar.iter_mut().find(|g| g.id == group.id) {
            existing.items.extend(group.items);
        } else {
            self.toolbar.push(group);
        }
    }

    /// 注册/合并一个 Ribbon Tab（同 ID Tab 合并组，组内同 ID 合并条目）。
    pub fn add_ribbon_tab(&mut self, tab: crate::workspace::RibbonTab) {
        if let Some(existing) = self.ribbon.iter_mut().find(|t| t.id == tab.id) {
            for group in tab.groups {
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

    // ---- 面板 / 视图 / 文档类型 ----

    pub fn register_panel(&mut self, def: PanelDef) {
        if self.panels.iter().any(|p| p.id == def.id) {
            self.error(format!("面板重复注册: {}", def.id));
            return;
        }
        self.panels.push(def);
    }

    pub fn register_view(&mut self, def: ViewDef) {
        if self.views.iter().any(|v| v.type_id == def.type_id) {
            self.error(format!("视图类型重复注册: {}", def.type_id));
            return;
        }
        self.views.push(def);
    }

    pub fn register_document_type(&mut self, def: DocumentTypeDef) {
        if self.doc_types.iter().any(|t| t.id == def.id) {
            self.error(format!("文档类型重复注册: {}", def.id));
            return;
        }
        self.doc_types.push(def);
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
