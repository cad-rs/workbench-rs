//! Host API v1 for plugin scripts: scripts register through global functions injected by the
//! host.
//!
//! ```rhai
//! // plugin.rhai
//! fn register() {
//!     register_command("rhai.hello", "Plugin Hello", "hello");
//!     add_toolbar_item("plugin.rhai", "rhai.hello", "Hello");
//! }
//! fn hello(args) { "Hello from Rhai" }   // a returned string is the note; () means no note
//! ```
//!
//! Threading model: rhai's default (non-`sync`) Engine/AST is not Send (gpui-rhai relies on
//! that too), so command callbacks hold no Engine — only the script source (`Arc<String>` is
//! Send) — and build and compile an Engine on the current thread when called. The scripts are
//! all small; building costs milliseconds.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use workbench_api::{CommandArgs, CommandResult, ProgressHandle};

/// Script cancellation sentinel: a background callback that throws an error carrying this
/// marker is mapped to a cooperative cancellation.
pub const CANCEL_SENTINEL: &str = "__wb_cancelled__";

/// One command a script declared in `register()`.
pub struct CollectedCommand {
    pub id: String,
    pub title: String,
    pub callback: String,
    pub background: bool,
}

/// What `register()` collected under control (handed back to the loader, which commits it to
/// the Registry in one go).
#[derive(Default)]
pub struct Collected {
    pub commands: Vec<CollectedCommand>,
    pub toolbar: Vec<(String, String, Option<String>)>,
    /// (panel id, title, absolute script path, dock area string)
    pub panels: Vec<(String, String, std::path::PathBuf, String)>,
    /// (view id, title, absolute script path)
    pub views: Vec<(String, String, std::path::PathBuf)>,
    pub notes: Vec<String>,
}

/// Shared collection bucket (written by the script's global functions, read by the loading
/// thread).
pub type SharedCollected = Arc<Mutex<Collected>>;

/// The script source a command callback refers to (Send, so it can cross into a task closure).
pub type RhaiProgram = Arc<String>;

/// The load-time Engine: injects the host global functions `register()` needs.
/// Used only on the loading thread and dropped when loading ends.
pub fn build_load_engine(plugin_dir: std::path::PathBuf, collected: SharedCollected) -> rhai::Engine {
    let mut engine = rhai::Engine::new();

    // register_command(id, title, callback) — sync command (runs on the UI thread)
    {
        let collected = collected.clone();
        engine.register_fn("register_command", move |id: &str, title: &str, callback: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .commands
                .push(CollectedCommand {
                    id: id.into(),
                    title: title.into(),
                    callback: callback.into(),
                    background: false,
                });
        });
    }
    // register_background_command(id, title, callback) — background thread task
    {
        let collected = collected.clone();
        engine.register_fn(
            "register_background_command",
            move |id: &str, title: &str, callback: &str| {
                collected
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .commands
                    .push(CollectedCommand {
                        id: id.into(),
                        title: title.into(),
                        callback: callback.into(),
                        background: true,
                    });
            },
        );
    }
    // add_toolbar_item(group, command, label)
    {
        let collected = collected.clone();
        engine.register_fn(
            "add_toolbar_item",
            move |group: &str, command: &str, label: &str| {
                collected
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .toolbar
                    .push((group.into(), command.into(), Some(label.into())));
            },
        );
    }
    // add_toolbar_item_no_label(group, command)
    {
        let collected = collected.clone();
        engine.register_fn("add_toolbar_item_no_label", move |group: &str, command: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .toolbar
                .push((group.into(), command.into(), None));
        });
    }
    // register_panel(id, title, script, area) — script UI panel (rendered by gpui-rhai)
    {
        let collected = collected.clone();
        let dir = plugin_dir.clone();
        engine.register_fn(
            "register_panel",
            move |id: &str, title: &str, script: &str, area: &str| {
                let area = area.trim().to_ascii_lowercase();
                if !matches!(area.as_str(), "left" | "right" | "bottom" | "top") {
                    return; // Unknown dock area: silently ignored
                }
                let mut c = collected.lock().unwrap_or_else(|p| p.into_inner());
                c.panels.push((id.into(), title.into(), dir.join(script), area));
            },
        );
    }
    // register_view(id, title, script) — script central view (gpui-rhai rendered)
    {
        let collected = collected.clone();
        let dir = plugin_dir;
        engine.register_fn(
            "register_view",
            move |id: &str, title: &str, script: &str| {
                let mut c = collected.lock().unwrap_or_else(|p| p.into_inner());
                c.views.push((id.into(), title.into(), dir.join(script)));
            },
        );
    }
    // log_info(msg)
    {
        let collected = collected.clone();
        engine.register_fn("log_info", move |message: &str| {
            collected
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .notes
                .push(message.into());
        });
    }

    engine
}

/// The Rhai wrapper of a background task handle (the `task` parameter of
/// `fn progress_job(task, args)`).
#[derive(Clone)]
struct RhaiTask {
    handle: ProgressHandle,
}

/// The callback execution Engine: registers the `task` object methods background callbacks
/// need. Built fresh for every call (never held across threads).
fn build_exec_engine() -> rhai::Engine {
    let mut engine = rhai::Engine::new();

    engine.register_type_with_name::<RhaiTask>("Task");
    engine.register_fn(
        "report",
        |task: &mut RhaiTask, fraction: rhai::FLOAT, stage: &str| {
            task.handle.report(Some(fraction as f32), Some(stage));
        },
    );
    engine.register_fn("report_progress", |task: &mut RhaiTask, fraction: rhai::FLOAT| {
        task.handle.report(Some(fraction as f32), None);
    });
    engine.register_fn("set_stage", |task: &mut RhaiTask, stage: &str| {
        task.handle.report(None, Some(stage));
    });
    engine.register_fn("log", |task: &mut RhaiTask, message: &str| {
        task.handle.log(message);
    });
    engine.register_fn("is_cancelled", |task: &mut RhaiTask| {
        task.handle.is_cancelled()
    });
    // Cooperative cancellation checkpoint: throws the sentinel error once cancelled, and the
    // task loop exits on it.
    engine.register_fn(
        "check_cancelled",
        |task: &mut RhaiTask| -> Result<(), Box<rhai::EvalAltResult>> {
            if task.handle.is_cancelled() {
                Err(format!("{CANCEL_SENTINEL}: cancellation requested").into())
            } else {
                Ok(())
            }
        },
    );
    engine
}

/// `CommandArgs` → Rhai Map (string keys and values; complex types are up to the command
/// script's own conventions).
pub fn args_to_map(args: &CommandArgs) -> rhai::Map {
    let mut map = rhai::Map::new();
    for (k, v) in &args.values {
        map.insert(k.clone().into(), v.clone().into());
    }
    map
}

/// Call a sync callback: a string returned by the script is the note; `()` means no note.
pub fn call_sync_callback(
    program: &RhaiProgram,
    callback: &str,
    args: &CommandArgs,
) -> CommandResult {
    let engine = build_exec_engine();
    let result = (|| -> Result<rhai::Dynamic, String> {
        let ast = engine.compile(&**program).map_err(|e| e.to_string())?;
        let mut scope = rhai::Scope::new();
        let map = args_to_map(args);
        engine
            .call_fn::<rhai::Dynamic>(&mut scope, &ast, callback, (map,))
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(value) => {
            if value.is_unit() {
                CommandResult::done()
            } else {
                match value.into_string() {
                    Ok(note) => CommandResult::done_with(note),
                    Err(_) => CommandResult::done(),
                }
            }
        }
        Err(e) => CommandResult::failed(format!("script callback `{callback}` failed: {e}")),
    }
}

/// The background callback body (task thread). Returning `None` commits no document change.
pub fn run_background_callback(
    program: &RhaiProgram,
    callback: &str,
    handle: ProgressHandle,
) -> Result<Option<workbench_api::DocCommit>, workbench_api::TaskFailure> {
    let engine = build_exec_engine();
    let run = (|| -> Result<rhai::Dynamic, String> {
        let ast = engine.compile(&**program).map_err(|e| e.to_string())?;
        let task = RhaiTask { handle };
        let mut scope = rhai::Scope::new();
        let map = rhai::Map::new();
        engine
            .call_fn::<rhai::Dynamic>(&mut scope, &ast, callback, (task, map))
            .map_err(|e| e.to_string())
    })();
    match run {
        Ok(_) => Ok(None),
        Err(e) => {
            if e.to_string().contains(CANCEL_SENTINEL) {
                Err(workbench_api::TaskFailure::Cancelled)
            } else {
                Err(workbench_api::TaskFailure::Error(format!(
                    "script task `{callback}` failed: {e}"
                )))
            }
        }
    }
}

/// An empty argument map for tests and tools.
pub fn empty_map() -> rhai::Map {
    BTreeMap::new()
}
