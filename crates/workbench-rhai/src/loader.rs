//! The workbench adapter: discovers plugin directories, gates them on the
//! product's entitlements, runs their entry points and commits the declarations
//! into the [`Registry`].
//!
//! The loading mechanics live in `plugin-host`; this module owns only the
//! workbench-specific commit and the neutral→`workbench_api` type mapping.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use plugin_host as ph;
use workbench_api as api;
use workbench_api::{CommandDef, Registry};

/// Scan the plugin directories (every first-level subdirectory holding a
/// `plugin.toml` is a plugin; sorted by name).
pub fn discover(dirs: &[PathBuf]) -> Vec<PathBuf> {
    ph::discover(dirs)
}

/// Load one plugin: a failure is recorded in `registry` as a rejection reason
/// and does not stop the other plugins.
fn load_one(registry: &mut Registry, dir: &Path) {
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let manifest = match ph::read_manifest(dir) {
        Ok(manifest) => manifest,
        Err(reason) => {
            registry.plugins_rejected.push((dir_name, reason));
            return;
        }
    };
    let plugin_id = manifest.plugin.id.clone();
    if registry.plugins_loaded.contains(&plugin_id) {
        registry.plugins_rejected.push((
            dir_name,
            format!("duplicate plugin ID ({plugin_id} is already taken)"),
        ));
        return;
    }
    // The product's entitlement answer: the first refused capability rejects,
    // and the plugin's script never compiles.
    if let Some(refusal) = manifest
        .plugin
        .requires
        .iter()
        .find_map(|capability| registry.refusal(capability))
    {
        registry.plugins_rejected.push((plugin_id, refusal));
        return;
    }

    let (program, mut collected) = match ph::run_entry(dir, &manifest) {
        Ok(loaded) => loaded,
        Err(reason) => {
            registry.plugins_rejected.push((plugin_id, reason));
            return;
        }
    };

    // Commit: commands + toolbar + script panels + notes
    for cmd in collected.commands.drain(..) {
        let def = if cmd.background {
            build_background_command(&cmd.id, &cmd.title, &cmd.callback, program.clone())
        } else {
            build_sync_command(&cmd.id, &cmd.title, &cmd.callback, program.clone())
        };
        registry.register_command(def.from_source(plugin_id.clone()));
    }
    for (group, command, label) in collected.toolbar.drain(..) {
        let group_id = api::ToolbarGroupId::new(group.clone());
        let item = match label {
            Some(l) => api::ToolbarItem::labeled(command, l),
            None => api::ToolbarItem::command(command),
        };
        // Controlled contribution: append to an existing group, or create the plugin's own
        // group (§7.3)
        if let Some(existing) = registry.toolbar.iter_mut().find(|g| g.id == group_id) {
            existing.items.push(item);
        } else {
            registry.toolbar.push(
                api::ToolbarGroup::new(group.clone(), group)
                    .from_source(plugin_id.clone())
                    .items(vec![item]),
            );
        }
    }
    for (id, title, script, area) in collected.panels.drain(..) {
        let area = match area.as_str() {
            "left" => api::DockArea::Left,
            "right" => api::DockArea::Right,
            "bottom" => api::DockArea::Bottom,
            "top" => api::DockArea::Top,
            _ => api::DockArea::Right,
        };
        let order = registry.panels.len() as i32;
        let def = api::PanelDef::new(
            id,
            title,
            area,
            300.0,
            order,
            || -> Box<dyn api::ViewInstance> {
                unreachable!("script panel creates no instance (content is mounted by the frontend from script)")
            },
        )
        .script(script.to_string_lossy().to_string());
        registry.register_panel(def);
    }
    // Script central views: singleton, content mounted by the gpui frontend.
    for (id, title, script) in collected.views.drain(..) {
        let def = api::ViewDef::new(
            id,
            title,
            true,
            || -> Box<dyn api::ViewInstance> {
                unreachable!("script view creates no instance (content is mounted by the frontend)")
            },
        )
        .script(script.to_string_lossy().to_string());
        registry.register_view(def);
    }
    for note in collected.notes.drain(..) {
        registry.notes.push(note);
    }
    // The plugin enables compiled-in modules; record what it declared so the
    // product can assemble them (see `Registry::plugin_module_ids`).
    registry
        .plugin_modules
        .push((plugin_id.clone(), manifest.plugin.modules.clone()));
    for capability in &manifest.plugin.requires {
        registry
            .plugin_requirements
            .push((plugin_id.clone(), capability.clone()));
    }
    registry.plugins_loaded.push(plugin_id);
}

fn build_sync_command(
    id: &str,
    title: &str,
    callback: &str,
    program: ph::RhaiProgram,
) -> CommandDef {
    let callback = callback.to_string();
    CommandDef::sync(id, title, move |_ctx, args| {
        let args = ph::CommandArgs {
            values: args.values.clone(),
            value: args.value.clone(),
        };
        match ph::call_sync_callback(&program, &callback, &args) {
            ph::CommandResult::Done { note } => api::CommandResult::Done { note },
            ph::CommandResult::Failed { error } => api::CommandResult::Failed { error },
        }
    })
}

fn build_background_command(
    id: &str,
    title: &str,
    callback: &str,
    program: ph::RhaiProgram,
) -> CommandDef {
    let title_spec = title.to_string();
    let callback_owned = callback.to_string();
    CommandDef::background(id, title_spec.clone(), move |_ctx: &mut api::CommandCtx| {
        let title = title_spec.clone();
        let program = program.clone();
        let callback = callback_owned.clone();
        Ok(api::TaskSpec {
            title,
            cancellable: true,
            input: Box::new(()),
            job: Box::new(move |task: &mut api::TaskCtx| {
                let sink = ScriptProgress(task.progress_handle());
                match ph::run_background_callback(&program, &callback, Arc::new(sink)) {
                    Ok(()) => Ok(None),
                    Err(ph::TaskFailure::Cancelled) => Err(api::TaskFailure::Cancelled),
                    Err(ph::TaskFailure::Error(message)) => Err(api::TaskFailure::Error(message)),
                }
            }),
        })
    })
}

/// Adapts the workbench progress handle to the neutral [`ph::ProgressSink`].
struct ScriptProgress(api::ProgressHandle);

impl ph::ProgressSink for ScriptProgress {
    fn report(&self, fraction: Option<f32>, stage: Option<String>) {
        self.0.report(fraction, stage.as_deref());
    }

    fn log(&self, message: String) {
        self.0.log(message);
    }

    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// The plugin loading stage: called by the builder at assembly time, contributes commands /
/// toolbar items / panels to the Registry. Failures are recorded in the `Registry` and do not
/// stop assembly (the same isolation policy as any other PluginStage).
pub fn plugin_stage(dirs: Vec<PathBuf>) -> workbench_core::PluginStage {
    Box::new(move |registry: &mut Registry| {
        for dir in ph::discover(&dirs) {
            load_one(registry, &dir);
        }
    })
}
