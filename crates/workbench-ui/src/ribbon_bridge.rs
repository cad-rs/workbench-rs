//! Bridge between the workbench platform model and the `ribbon` component.
//!
//! This module is the single place where the workbench couples to the ribbon
//! crate (docs/ribbon-bar.md §5.1): every frame it projects the workspace
//! snapshot into a [`ribbon::RibbonModel`] (command enablement / checked /
//! combo state resolved through the [`AppRuntime`] queries), and every ribbon
//! event back into a workbench action (`execute_command`,
//! `set_active_ribbon_tab`, repaint).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, AppContext as _, Entity, WeakEntity};
use ribbon::{
    ItemSize, MenuEntry, MenuEntryVm, OverflowStrategy, RibbonEvent, RibbonGroupVm,
    RibbonItemKindVm, RibbonItemVm, RibbonModel, RibbonState, RibbonTabVm,
};
use workbench_api as wb;
use workbench_core::AppRuntime;

use crate::{RootView, SharedCore, notify};

// ---- Model projection (pure; unit-testable without a window) ----

/// Projects the workspace snapshot into the ribbon view model.
///
/// `collapsed` is owned by the host (this bridge), not by the component: it is
/// handed back in on every frame so the model stays a pure function of the
/// workspace state plus the host-owned chrome state.
pub(crate) fn build_model(core: &AppRuntime, collapsed: bool) -> RibbonModel {
    let ws = &core.host.ws;
    RibbonModel {
        // Product logo mount is deferred to a later milestone.
        logo: None,
        qat: ws.qat.iter().map(|item| qat_item_vm(core, item)).collect(),
        tabs: ws
            .ribbon
            .iter()
            // Contextual tabs are visible when untagged or when the current
            // work mode is listed (ribbon-bar.md §4.7).
            .filter(|tab| tab.contexts.is_empty() || tab.contexts.contains(&ws.work_mode))
            .map(|tab| tab_vm(core, tab))
            .collect(),
        active_tab_id: ws.active_ribbon_tab.clone(),
        collapsed,
        // The collapse feature is a product capability, disabled by default
        // and enabled via WorkbenchAppBuilder::ribbon_collapse_enabled.
        collapse_enabled: ws.ribbon_collapse_enabled,
        overflow: OverflowStrategy::Scroll,
        work_mode: ws.work_mode.clone(),
    }
}

/// QAT items always render as Icon-tier plain command buttons.
fn qat_item_vm(core: &AppRuntime, item: &wb::ToolbarItem) -> RibbonItemVm {
    RibbonItemVm {
        size: ItemSize::Icon,
        kind: RibbonItemKindVm::Command,
        ..item_vm(core, item)
    }
}

fn tab_vm(core: &AppRuntime, tab: &wb::RibbonTab) -> RibbonTabVm {
    RibbonTabVm {
        id: tab.id.0.clone(),
        title: tab.title.clone(),
        contextual: !tab.contexts.is_empty(),
        groups: tab
            .groups
            .iter()
            .map(|group| RibbonGroupVm {
                id: group.id.0.clone(),
                title: group.title.clone(),
                items: group.items.iter().map(|item| item_vm(core, item)).collect(),
            })
            .collect(),
    }
}

/// One `ToolbarItem` (workbench model) to one `RibbonItemVm` (component view
/// model). Labels fall back to the command title and then to the command id,
/// matching the retired inline implementation.
fn item_vm(core: &AppRuntime, item: &wb::ToolbarItem) -> RibbonItemVm {
    let command = item.command.0.clone();
    let label = item
        .label
        .clone()
        .unwrap_or_else(|| fallback_label(core, &command));
    let enabled = core.is_enabled(&command);
    let icon = core.command_icon(&command);
    let size = match item.size {
        wb::ItemSize::Large => ItemSize::Large,
        wb::ItemSize::Small => ItemSize::Small,
        wb::ItemSize::Icon => ItemSize::Icon,
    };
    let kind = match &item.kind {
        wb::RibbonItemKind::Command => RibbonItemKindVm::Command,
        // Toggle checked state is a command system query (ribbon-bar.md §4.2).
        wb::RibbonItemKind::Toggle => RibbonItemKindVm::Toggle {
            checked: core.is_checked(&command),
        },
        wb::RibbonItemKind::Split { items } => RibbonItemKindVm::Split {
            items: menu_entries(core, items),
        },
        wb::RibbonItemKind::Menu { items } => RibbonItemKindVm::Menu {
            items: menu_entries(core, items),
        },
        // Combo current value and options are a per-frame command system query.
        wb::RibbonItemKind::Combo => {
            let (current, options) = core
                .combo_state(&command)
                .map(|state| (state.current, state.options))
                .unwrap_or_default();
            RibbonItemKindVm::Combo { current, options }
        }
    };
    RibbonItemVm {
        id: command,
        label,
        icon,
        enabled,
        size,
        width: item.width,
        kind,
    }
}

fn menu_entries(core: &AppRuntime, items: &[wb::ToolbarItem]) -> Vec<MenuEntry> {
    items
        .iter()
        .map(|item| {
            let command = item.command.0.clone();
            MenuEntry::Item(MenuEntryVm {
                id: command.clone(),
                label: item
                    .label
                    .clone()
                    .unwrap_or_else(|| fallback_label(core, &command)),
                icon: core.command_icon(&command),
                enabled: core.is_enabled(&command),
                checked: false,
            })
        })
        .collect()
}

fn fallback_label(core: &AppRuntime, command: &str) -> String {
    core.command_title(command)
        .unwrap_or_else(|| command.to_string())
}

// ---- Host (entity mount + events) ----

/// The root view's ribbon mount point (Option-lazy-build, same pattern as the
/// dock shell): the entity is created on the first frame and re-modelled on
/// every frame after.
pub(crate) struct RibbonHost {
    entity: Option<Entity<RibbonState>>,
    /// Collapsed (minimized) chrome state, shared with the event callback.
    /// Runtime-only this milestone; persistence lands in a later one
    /// (ribbon-bar.md M3).
    collapsed: Rc<Cell<bool>>,
}

impl RibbonHost {
    pub(crate) fn new() -> Self {
        Self {
            entity: None,
            collapsed: Rc::new(Cell::new(false)),
        }
    }

    /// Creates the ribbon entity on first use, seeded with the current
    /// snapshot and the workbench-wired event callback.
    pub(crate) fn ensure_mounted(
        &mut self,
        core: &SharedCore,
        weak: WeakEntity<RootView>,
        cx: &mut App,
    ) {
        if self.entity.is_some() {
            return;
        }
        // Seed the collapsed runtime state from the persisted workspace value.
        let persisted_collapsed = {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            core.host.ws.ribbon_collapsed
        };
        self.collapsed.set(persisted_collapsed);
        let model = {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            build_model(&core, self.collapsed.get())
        };
        let core = core.clone();
        let collapsed = Rc::clone(&self.collapsed);
        self.entity = Some(cx.new(move |_| {
            RibbonState::new(model, move |state, event, _window, cx| {
                handle_event(&core, &weak, &collapsed, state, event, cx);
            })
        }));
    }

    /// Feeds the fresh snapshot to the component for this frame.
    pub(crate) fn sync(&self, core: &SharedCore, cx: &mut App) {
        let Some(entity) = self.entity.as_ref() else {
            return;
        };
        // Reconcile: if ws.ribbon_collapsed was toggled externally (Ctrl+F1
        // command path), adopt it so the runtime stays in sync.
        {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            let ws_collapsed = core.host.ws.ribbon_collapsed;
            drop(core);
            if ws_collapsed != self.collapsed.get() {
                self.collapsed.set(ws_collapsed);
            }
        }
        let model = {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            build_model(&core, self.collapsed.get())
        };
        entity.update(cx, |state, cx| state.set_model(model, cx));
    }

    /// The ribbon element for this frame's tree.
    pub(crate) fn element(&self) -> Entity<RibbonState> {
        self.entity
            .clone()
            .expect("ribbon must be mounted before render")
    }
}

/// Ribbon event to workbench action (lock core, act, log failures), the same
/// pattern as the retired inline on_click handlers; always repaints through
/// the root view.
fn handle_event(
    core: &SharedCore,
    weak: &WeakEntity<RootView>,
    collapsed: &Rc<Cell<bool>>,
    state: &mut RibbonState,
    event: RibbonEvent,
    cx: &mut App,
) {
    match event {
        RibbonEvent::Command { id, value } => {
            let result = core
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .execute_command(&id, &wb::CommandArgs { value, ..Default::default() });
            if let wb::CommandResult::Failed { error } = result {
                core.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .host
                    .log
                    .error(error);
            }
        }
        RibbonEvent::ActiveTabChanged { id } => {
            core.lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .ws
                .set_active_ribbon_tab(&id);
        }
        RibbonEvent::ToggleCollapse => {
            // The bridge owns the collapsed state; the component only reports
            // the request. Sync to the workspace so it persists across sessions.
            let new_collapsed = !state.model().collapsed;
            collapsed.set(new_collapsed);
            core.lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .ws
                .ribbon_collapsed = new_collapsed;
        }
    }
    notify(weak, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real runtime from the example workbench (same recipe as the dock
    /// snapshot test in lib.rs).
    fn example_runtime() -> AppRuntime {
        workbench_core::WorkbenchAppBuilder::new("test.ribbon", "Ribbon bridge")
            .with_workbench(wb_example::ExampleWorkbench)
            .open_on_start("example.welcome", false)
            .build()
            .expect("runtime builds")
    }

    fn group_of<'a>(tab: &'a RibbonTabVm, id: &str) -> &'a RibbonGroupVm {
        tab.groups.iter().find(|g| g.id == id).expect("group exists")
    }

    fn item_of<'a>(group: &'a RibbonGroupVm, id: &str) -> &'a RibbonItemVm {
        group.items.iter().find(|i| i.id == id).expect("item exists")
    }

    /// Tab list, order, active id and chrome defaults.
    #[test]
    fn model_projects_tabs_active_id_and_chrome_defaults() {
        let runtime = example_runtime();
        let model = build_model(&runtime, false);

        let ids: Vec<&str> = model.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["app.home", "wb.example.tools"]);
        assert_eq!(model.active_tab_id, "app.home");
        // Logo is deferred; overflow defaults to scroll; collapsed comes from
        // the caller.
        assert!(model.logo.is_none());
        assert_eq!(model.overflow, OverflowStrategy::Scroll);
        assert!(!model.collapsed);
        assert!(build_model(&runtime, true).collapsed);
        assert_eq!(model.work_mode, runtime.host.ws.work_mode);
        assert!(!model.tabs[0].contextual, "resident tabs are not contextual");
    }

    /// Item labels fall back to the command title; enabled mirrors the
    /// command system query.
    #[test]
    fn item_labels_and_enabled_follow_command_system() {
        let runtime = example_runtime();
        let model = build_model(&runtime, false);

        // Unlabeled item: label falls back to the command title; enabled is
        // the is_enabled query result (undo needs undoable changes, so a fresh
        // runtime has it disabled).
        let edit = group_of(&model.tabs[0], "example.edit");
        let undo = item_of(edit, "app.undo");
        assert_eq!(undo.label, runtime.command_title("app.undo").unwrap());
        assert_eq!(undo.enabled, runtime.is_enabled("app.undo"));
        assert!(!undo.enabled, "fresh runtime has no undoable changes");
        assert_eq!(undo.icon, runtime.command_icon("app.undo"));
        assert_eq!(undo.size, ItemSize::Small, "registry default size tier");
        assert_eq!(undo.kind, RibbonItemKindVm::Command);

        // Labeled item: the item label wins over the command title.
        let shapes = group_of(&model.tabs[0], "example.shapes");
        let generate = item_of(shapes, "example.generate_shapes");
        let ws_label = runtime
            .host
            .ws
            .ribbon
            .iter()
            .flat_map(|t| t.groups.iter())
            .flat_map(|g| g.items.iter())
            .find(|i| i.command.0 == "example.generate_shapes")
            .and_then(|i| i.label.clone())
            .expect("the item carries its own label");
        assert_eq!(generate.label, ws_label);
        assert_ne!(generate.label, runtime.command_title("example.generate_shapes").unwrap());
    }

    /// Toggle / Menu / Combo kinds project with their per-frame command
    /// system state.
    #[test]
    fn item_kinds_map_toggle_menu_and_combo() {
        let mut runtime = example_runtime();
        let home = runtime
            .host
            .ws
            .ribbon
            .iter_mut()
            .find(|t| t.id.0 == "app.home")
            .unwrap();
        let edit = home
            .groups
            .iter_mut()
            .find(|g| g.id.0 == "example.edit")
            .unwrap();
        // Unregistered commands: is_checked/is_enabled/combo_state all fall
        // back to their defaults.
        edit.items
            .push(wb::ToolbarItem::command("test.unregistered").toggle());
        edit.items.push(
            wb::ToolbarItem::command("test.menu_host")
                .menu(vec![wb::ToolbarItem::labeled("test.menu_child", "Menu Child")]),
        );
        edit.items
            .push(wb::ToolbarItem::command("test.combo_host").combo().width(120.0));

        let model = build_model(&runtime, false);
        let edit = group_of(&model.tabs[0], "example.edit");

        let toggle = item_of(edit, "test.unregistered");
        assert_eq!(toggle.kind, RibbonItemKindVm::Toggle { checked: false });
        assert!(!toggle.enabled, "unregistered commands are disabled");

        let menu = item_of(edit, "test.menu_host");
        assert_eq!(
            menu.kind,
            RibbonItemKindVm::Menu {
                items: vec![MenuEntry::Item(MenuEntryVm {
                    id: "test.menu_child".to_string(),
                    label: "Menu Child".to_string(),
                    icon: None,
                    enabled: false,
                    checked: false,
                })],
            }
        );

        let combo = item_of(edit, "test.combo_host");
        assert_eq!(
            combo.kind,
            RibbonItemKindVm::Combo {
                current: String::new(),
                options: Vec::new(),
            }
        );
        assert_eq!(combo.width, Some(120.0));
    }

    /// QAT items render as Icon-tier plain command buttons.
    #[test]
    fn qat_projects_as_icon_tier_commands() {
        let mut runtime = example_runtime();
        assert!(build_model(&runtime, false).qat.is_empty(), "no QAT yet");

        runtime.host.ws.qat.push(wb::ToolbarItem::command("app.save_document"));
        let model = build_model(&runtime, false);
        assert_eq!(model.qat.len(), 1);
        let qat = &model.qat[0];
        assert_eq!(qat.id, "app.save_document");
        assert_eq!(qat.size, ItemSize::Icon, "QAT is always Icon tier");
        assert_eq!(qat.kind, RibbonItemKindVm::Command);
        assert_eq!(qat.label, runtime.command_title("app.save_document").unwrap());
        assert_eq!(qat.enabled, runtime.is_enabled("app.save_document"));
        assert_eq!(qat.icon, runtime.command_icon("app.save_document"));
    }

    /// Contextual tabs appear only in their work mode (and are flagged
    /// contextual once visible).
    #[test]
    fn contextual_tabs_filter_by_work_mode() {
        let mut runtime = example_runtime();
        let mut contextual = wb::RibbonTab::new("test.ctx.tab", "Ctx");
        contextual.contexts = vec!["test.mode.absent".to_string()];
        runtime.host.ws.add_ribbon_tab(contextual);

        let model = build_model(&runtime, false);
        assert!(
            model.tabs.iter().all(|t| t.id != "test.ctx.tab"),
            "tab is hidden outside its work mode"
        );

        runtime.host.ws.set_work_mode("test.mode.absent");
        let model = build_model(&runtime, false);
        let ctx_tab = model
            .tabs
            .iter()
            .find(|t| t.id == "test.ctx.tab")
            .expect("tab is visible in its work mode");
        assert!(ctx_tab.contextual);
        assert_eq!(model.work_mode, "test.mode.absent");
    }

    /// The host starts expanded and can flip its collapsed flag.
    #[test]
    fn host_starts_expanded() {
        let host = RibbonHost::new();
        assert!(!host.collapsed.get());
        host.collapsed.set(true);
        assert!(host.collapsed.get());
    }
}
