//! The registry: commands, toolbars, panels, views, document types and services contributed by
//! Workbench modules (design.md §8).
//!
//! Registration errors such as duplicate IDs do not panic; they are recorded in `errors`
//! (diagnosable registration results, design.md §7.3).

use std::any::{Any, TypeId};

use crate::command::CommandDef;
use crate::documents::DocumentTypeDef;
use crate::ids::WorkbenchId;
use crate::settings::SettingSpec;
use crate::workspace::{PanelDef, ToolbarGroup, ToolbarItem, ViewDef};

/// The registry: filled by Workbench.init while the product is assembled, read-only at runtime.
#[derive(Default)]
pub struct Registry {
    pub commands: Vec<CommandDef>,
    pub toolbar: Vec<ToolbarGroup>,
    pub ribbon: Vec<crate::workspace::RibbonTab>,
    /// Quick access (QAT, a static product set, ribbon-bar.md §4.1): held during assembly and
    /// moved into [`WorkspaceState::qat`](crate::workspace::WorkspaceState) at build time.
    pub qat: Vec<ToolbarItem>,
    pub panels: Vec<PanelDef>,
    pub views: Vec<ViewDef>,
    pub doc_types: Vec<DocumentTypeDef>,
    /// Registered setting specs (platform built-ins + plugin settings); moved into ServiceHost at
    /// build time, where the settings dialog renders from them and `app.settings.set` validates
    /// against them.
    pub setting_specs: Vec<SettingSpec>,
    /// Locale table entries `(locale, key, text)` from the platform and plugins; merged into
    /// ServiceHost's Localizer at build time.
    pub locale_entries: Vec<(String, String, String)>,
    services: Vec<(TypeId, Box<dyn Any + Send + Sync>)>,
    /// Diagnostics from assembly (duplicate registrations, invalid arguments, …).
    pub errors: Vec<String>,
    /// Notes from assembly (such as plugin loading logs).
    pub notes: Vec<String>,
    /// The assembled Workbenches.
    pub workbenches: Vec<WorkbenchId>,
    /// IDs of the plugins loaded successfully (for diagnostics and test queries).
    pub plugins_loaded: Vec<String>,
    /// The module IDs each loaded plugin declares: (plugin ID, module IDs).
    ///
    /// A plugin is a runtime unit and does not "contain" code — it enables modules the product
    /// has already compiled. The product gets the deduplicated set from
    /// [`Registry::plugin_module_ids`] and assembles it.
    pub plugin_modules: Vec<(String, Vec<String>)>,
    /// The feature each loaded plugin requires (manifest `requires`); moved into the
    /// ServiceHost::plugins entries at build time.
    pub plugin_requirements: Vec<(String, String)>,
    /// What decides gated features, when the product limits any; plugin loaders ask
    /// [`Registry::refusal`].
    pub entitlements: Option<std::sync::Arc<dyn crate::entitlements::Entitlements>>,
    /// Rejected/failed plugins: (plugin ID or directory, reason).
    pub plugins_rejected: Vec<(String, String)>,
    /// IDs of the plugins disabled at runtime (phase 5).
    pub plugins_disabled: Vec<String>,
    /// The work modes and default mode the Workbenches suggest (empty uses the platform default).
    pub work_modes: Vec<String>,
    pub default_mode: String,
    /// The source plugin currently registering contributions (see [`Registry::with_source`]).
    current_source: Option<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Why the user may not use `feature`; None when they may.
    pub fn refusal(&self, feature: &str) -> Option<String> {
        crate::entitlements::refusal(self.entitlements.as_ref(), feature)
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

    // ---- Commands ----

    pub fn register_command(&mut self, mut def: CommandDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.commands.iter().any(|c| c.id == def.id) {
            self.error(format!(
                "Duplicate command registration: {} (from {})",
                def.id, def.title
            ));
            return;
        }
        if let Some(hk) = def.hotkey {
            if let Some(other) = self.commands.iter().find(|c| c.hotkey == Some(hk)) {
                self.error(format!(
                    "Hotkey {} conflict: both {} and {} bind it",
                    hk, other.id, def.id
                ));
            }
        }
        self.commands.push(def);
    }

    /// Take a command out (the core executor removes it temporarily while it runs, to avoid
    /// borrow conflicts when the handler re-enters).
    pub fn take_command(&mut self, id: &str) -> Option<CommandDef> {
        let pos = self.commands.iter().position(|c| c.id.0 == id)?;
        Some(self.commands.remove(pos))
    }

    /// Put back a command that was taken out (called once it has run).
    pub fn put_command(&mut self, def: CommandDef) {
        self.commands.push(def);
    }

    pub fn command(&self, id: &str) -> Option<&CommandDef> {
        self.commands.iter().find(|c| c.id.0 == id)
    }

    /// Remove commands by source (cleanup when a plugin is disabled/unloaded, design.md §16.2).
    /// Also removes their items from toolbar groups and drops the groups that source introduced
    /// once they are empty. Returns the removed command IDs.
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
        // Also remove the groups this source contributed that are now empty
        self.toolbar
            .retain(|g| !(g.items.is_empty() && g.source.as_deref() == Some(source)));
        removed
    }

    pub fn commands(&self) -> impl Iterator<Item = &CommandDef> {
        self.commands.iter()
    }

    /// The module IDs all loaded plugins declare, deduplicated in order of first appearance.
    ///
    /// The product assembles the Workbench modules the plugins enable from this list; the order
    /// does not respect dependencies, so the product has to sort them topologically itself.
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

    // ---- Toolbar ----

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

    /// Register/merge a Ribbon tab (tabs with the same ID merge their groups, groups with the same
    /// ID merge their items).
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

    /// Add a command to the quick access (QAT) set (a static product set, ribbon-bar.md §4.1):
    /// rendered as a small icon-size button in the tab strip, still visible when collapsed.
    /// Adding it twice is not deduplicated (it is declared statically during assembly).
    pub fn add_qat_command(&mut self, cmd: impl Into<crate::ids::CommandId>) {
        self.qat.push(ToolbarItem::command(cmd));
    }

    // ---- Panels / views / document types ----

    pub fn register_panel(&mut self, mut def: PanelDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.panels.iter().any(|p| p.id == def.id) {
            self.error(format!("Duplicate panel registration: {}", def.id));
            return;
        }
        self.panels.push(def);
    }

    /// Register a setting spec (platform built-in or plugin setting). Duplicate
    /// keys are rejected with a registry error, keeping the first registration.
    pub fn register_setting(&mut self, spec: SettingSpec) {
        if self.setting_specs.iter().any(|s| s.key == spec.key) {
            self.error(format!("Duplicate setting registration: {}", spec.key));
            return;
        }
        self.setting_specs.push(spec);
    }

    /// Contribute one locale's translation entries (`key`, `text`). Products
    /// and plugins call this at assembly time; later entries win.
    pub fn register_locales(
        &mut self,
        locale: &str,
        entries: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) {
        let locale = locale.to_string();
        for (key, text) in entries {
            self.locale_entries
                .push((locale.clone(), key.to_string(), text.to_string()));
        }
    }

    pub fn register_view(&mut self, mut def: ViewDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.views.iter().any(|v| v.type_id == def.type_id) {
            self.error(format!("Duplicate view type registration: {}", def.type_id));
            return;
        }
        self.views.push(def);
    }

    pub fn register_document_type(&mut self, mut def: DocumentTypeDef) {
        if def.source.is_none() {
            def.source = self.current_source.clone();
        }
        if self.doc_types.iter().any(|t| t.id == def.id) {
            self.error(format!("Duplicate document type registration: {}", def.id));
            return;
        }
        self.doc_types.push(def);
    }

    /// Remove contributions by source (cleanup when a plugin is disabled/unloaded,
    /// design.md §16.2).
    ///
    /// Removes the commands, view types, panels, document types, Ribbon tabs and toolbar groups
    /// that source registered. Returns the removed command IDs (the caller cleans the workspace
    /// model up with them).
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

    // ---- Extension services ----

    pub fn add_service<T: Any + Send + Sync>(&mut self, value: T) {
        if self.services.iter().any(|(t, _)| *t == TypeId::of::<T>()) {
            self.error(format!(
                "Duplicate extension service registration: {}",
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
