//! Plugin discovery and loading: directory scan → manifest validation → script `register()`
//! → committed to the Registry.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use workbench_api as api;
use workbench_api::{CommandDef, Registry};

use crate::host::{build_load_engine, call_sync_callback, run_background_callback, RhaiProgram, SharedCollected};
use crate::manifest::{parse as parse_manifest, split_entry_point, PluginManifest};

/// Scan the plugin directories (every first-level subdirectory holding a `plugin.toml` is a
/// plugin; sorted by name).
pub fn discover(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("plugin.toml").is_file() {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates
}

/// Load one plugin: a failure is recorded in `registry` as a rejection reason and does not
/// stop the other plugins.
fn load_one(registry: &mut Registry, dir: &Path) {
    let dir_name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let Ok(text) = std::fs::read_to_string(dir.join("plugin.toml")) else {
        registry
            .plugins_rejected
            .push((dir_name, "plugin.toml is not readable".to_string()));
        return;
    };
    let manifest: PluginManifest = match parse_manifest(&text) {
        Ok(m) => m,
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

    if let Some(refusal) = manifest.plugin.requires.as_deref().and_then(|f| registry.refusal(f)) {
        registry.plugins_rejected.push((plugin_id, refusal));
        return;
    }

    let (entry_file, entry_fn) = split_entry_point(&manifest.plugin.entry_point);
    let entry_path = dir.join(&entry_file);
    let Ok(source) = std::fs::read_to_string(&entry_path) else {
        registry
            .plugins_rejected
            .push((plugin_id, format!("entry script is not readable: {}", entry_path.display())));
        return;
    };

    let collected: SharedCollected = Arc::new(Mutex::new(crate::host::Collected::default()));
    let engine = build_load_engine(dir.to_path_buf(), collected.clone());
    let ast = match engine.compile(&source) {
        Ok(ast) => ast,
        Err(e) => {
            registry
                .plugins_rejected
                .push((plugin_id, format!("script failed to compile: {e}")));
            return;
        }
    };
    let _ = ast; // Compile-time check; command callbacks rebuild the Engine on the calling thread

    // Run the entry function (default `register`); the script registers through the host's
    // global functions.
    let mut scope = rhai::Scope::new();
    if let Err(e) = engine.call_fn::<()>(&mut scope, &ast, &entry_fn, ()) {
        registry
            .plugins_rejected
            .push((plugin_id, format!("entry `{entry_fn}` failed: {e}")));
        return;
    }

    // Commit: commands + toolbar + script panels + notes
    let program: RhaiProgram = Arc::new(source);
    let mut collected = collected.lock().unwrap_or_else(|p| p.into_inner());
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
    if let Some(feature) = &manifest.plugin.requires {
        registry.plugin_requirements.push((plugin_id.clone(), feature.clone()));
    }
    registry.plugins_loaded.push(plugin_id);
}

fn build_sync_command(
    id: &str,
    title: &str,
    callback: &str,
    program: RhaiProgram,
) -> CommandDef {
    let callback = callback.to_string();
    CommandDef::sync(id, title, move |_ctx, args| {
        call_sync_callback(&program, &callback, args)
    })
}

fn build_background_command(
    id: &str,
    title: &str,
    callback: &str,
    program: RhaiProgram,
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
                run_background_callback(&program, &callback, task.progress_handle())
            }),
        })
    })
}

/// The plugin loading stage: called by the builder at assembly time, contributes commands /
/// toolbar items / panels to the Registry. Failures are recorded in the `Registry` and do not
/// stop assembly (the same isolation policy as any other PluginStage).
pub fn plugin_stage(dirs: Vec<PathBuf>) -> workbench_core::PluginStage {
    Box::new(move |registry: &mut Registry| {
        for dir in discover(&dirs) {
            load_one(registry, &dir);
        }
    })
}
