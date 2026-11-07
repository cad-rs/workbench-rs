//! Headless selfcheck: without opening a window, drives the platform core through its key
//! paths to verify the architecture end to end. Products invoke it with `--selfcheck`; it
//! prints PASS/FAIL lines and exits with code 0/1.
//!
//! Domain content is checked through the serialization interface registered with the document
//! type + generic JSON reads, so the core depends on no domain crate.

use std::time::{Duration, Instant};

use workbench_api as api;

use crate::app::AppRuntime;

struct Check {
    name: String,
    ok: bool,
    skipped: bool,
    detail: String,
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self { name: name.into(), ok, skipped: false, detail: detail.into() }
    }
    /// Skipped when a precondition is not met (a probing selfcheck: any product can run it,
    /// irrelevant checks SKIP automatically).
    fn skip(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { name: name.into(), ok: false, skipped: true, detail: reason.into() }
    }
}

/// Recovered-document title prefix (`recover_autosaves` renames to
/// `[Recovered] <title>`), so a probe can identify what recovery produced.
const RECOVERED_PREFIX: &str = "[Recovered]";

/// Session A of the crash-recovery probe (phase 5, `--selfcheck-crash` with
/// `WB_CRASH_PHASE=A`): give the product's own default document a probe title,
/// make it dirty, autosave it, then exit **without running `Drop`** — so the
/// `session.clean` marker is never written and the next launch must recover.
///
/// The probe is product-agnostic: it only uses the platform's document service
/// (rename + commit a type-erased content duplicate), never domain commands.
pub fn crash_probe_crash(mut session_a: AppRuntime, dir: &std::path::Path) -> ! {
    session_a.frame_tick(0.0);
    let title = format!("crash-probe-{}", std::process::id());
    let prepared = prepare_probe_document(&mut session_a, &title);
    let saved = session_a.autosave_now().unwrap_or(0);
    // Recovery deletes the autosave directory, so snapshot the persisted bytes
    // here for session B's content-consistency check.
    let expected = read_autosaved_bytes(&session_a, &title);
    let _ = std::fs::create_dir_all(dir);
    if let Some(bytes) = &expected {
        let _ = std::fs::write(dir.join("expected.bin"), bytes);
    }
    let _ = std::fs::write(
        dir.join("probe.json"),
        serde_json::json!({ "title": title, "prepared": prepared, "saved": saved }).to_string(),
    );
    println!("[crash-probe] session A: prepared={prepared} saved={saved} title={title:?}");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    // Abnormal exit: no Drop, therefore no `session.clean` marker.
    std::process::exit(if prepared && saved > 0 && expected.is_some() { 0 } else { 1 });
}

/// Session B of the crash-recovery probe (`WB_CRASH_PHASE=B`): the launch after
/// the simulated crash. `build()` has already run recovery (the marker is
/// absent); verify the recovered document exists and that its content matches
/// what session A persisted.
///
/// It then prepares a *fresh* dirty document and autosaves it again, so that
/// session C can tell "recovery suppressed by the clean marker" apart from
/// "nothing left to recover". The runtime is dropped on return, which writes
/// the marker — that clean exit is exactly what session C checks.
pub fn crash_probe_recover(mut session_b: AppRuntime, dir: &std::path::Path) -> i32 {
    session_b.frame_tick(0.0);
    let expected = std::fs::read(dir.join("expected.bin")).ok();
    let recovered_id = session_b
        .host
        .docs
        .docs()
        .find(|d| d.title.starts_with(RECOVERED_PREFIX))
        .map(|d| d.id);
    let content_ok = match (recovered_id, expected.as_deref()) {
        (Some(id), Some(bytes)) => serialize_doc(&session_b, id)
            .map(|b| b == bytes)
            .unwrap_or(false),
        _ => false,
    };
    let recovered: Vec<String> = session_b
        .host
        .docs
        .docs()
        .filter(|d| d.title.starts_with(RECOVERED_PREFIX))
        .map(|d| d.title.clone())
        .collect();
    let _ = std::fs::write(
        dir.join("result.json"),
        serde_json::to_string(&recovered).unwrap_or_else(|_| "[]".to_string()),
    );
    let ok = !recovered.is_empty() && content_ok;
    println!("[crash-probe] session B: recovered={recovered:?} content_ok={content_ok}");

    // Leave a fresh dirty document behind, then exit cleanly (Drop writes the
    // marker). Session C must see the marker and recover nothing.
    let title = format!("crash-probe-clean-{}", std::process::id());
    let prepared = prepare_probe_document(&mut session_b, &title);
    let saved = session_b.autosave_now().unwrap_or(0);
    let left = count_autosave_files(&session_b);
    let _ = std::fs::write(
        dir.join("clean-input.json"),
        serde_json::json!({ "prepared": prepared, "saved": saved, "autosave_files": left })
            .to_string(),
    );
    println!("[crash-probe] session B clean exit: saved={saved} autosave_files={left}");
    if ok && prepared && saved > 0 && left > 0 {
        0
    } else {
        1
    }
}

/// Session C of the crash-recovery probe (`WB_CRASH_PHASE=C`): the launch after
/// a *clean* exit, so the marker is present and `build()` must not recover —
/// even though session B left autosave files behind. Writes `clean.json`.
pub fn crash_probe_clean(mut session_c: AppRuntime, dir: &std::path::Path) -> i32 {
    session_c.frame_tick(0.0);
    let recovered: Vec<String> = session_c
        .host
        .docs
        .docs()
        .filter(|d| d.title.starts_with(RECOVERED_PREFIX))
        .map(|d| d.title.clone())
        .collect();
    let suppressed = recovered.is_empty();
    let _ = std::fs::write(
        dir.join("clean.json"),
        serde_json::to_string(&serde_json::json!({
            "recovered": recovered,
            "suppressed": suppressed,
        }))
        .unwrap_or_else(|_| "{}".to_string()),
    );
    println!("[crash-probe] session C: recovered={recovered:?} suppressed={suppressed}");
    if suppressed {
        0
    } else {
        1
    }
}

/// Give the active document a probe-specific title and commit a duplicate of
/// its content so it becomes dirty (autosave only persists dirty documents).
/// The commit is type-erased: the platform never touches domain content.
fn prepare_probe_document(app: &mut AppRuntime, title: &str) -> bool {
    let Some(id) = app.host.docs.active_id() else {
        return false;
    };
    if let Some(doc) = app.host.docs.get_mut(id) {
        doc.title = title.to_string();
    }
    let content = app.host.docs.get(id).map(|d| d.content.dup());
    match content {
        Some(c) => app.host.docs.commit(id, "crash probe", true, c).is_ok(),
        None => false,
    }
}

/// The bytes `autosave_now` persisted for `title` (its `autosave/<title>_.<ext>`
/// entry).
fn read_autosaved_bytes(app: &AppRuntime, title: &str) -> Option<Vec<u8>> {
    let safe: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let state = app.state();
    let key = state
        .keys(&format!("autosave/{safe}"))
        .into_iter()
        .find(|key| !key.ends_with(".json"))?;
    state.read(&key)
}

/// How many content entries the autosave area currently holds (metadata
/// `.json` sidecars excluded).
fn count_autosave_files(app: &AppRuntime) -> usize {
    app.state()
        .keys("autosave/")
        .iter()
        .filter(|key| !key.ends_with(".json"))
        .count()
}

/// Serialize a document through its registered type (platform-agnostic read).
fn serialize_doc(app: &AppRuntime, id: api::DocumentId) -> Option<Vec<u8>> {
    let doc = app.host.docs.get(id)?;
    let def = app.host.docs.type_of(&doc.type_id)?;
    (def.serialize)(doc.content.as_ref()).ok()
}

/// Launch one crash-probe phase as a subprocess. Times out instead of hanging
/// when a product does not implement the `--selfcheck-crash` mode.
fn run_probe_phase(
    exe: &std::path::Path,
    dir: &std::path::Path,
    phase: &str,
) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .arg("--selfcheck-crash")
        .env("WB_CRASH_DIR", dir)
        .env("WB_CRASH_PHASE", phase)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start the subprocess: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("The subprocess timed out (phase {phase})"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("Failed to wait for the subprocess: {e}")),
        }
    };
    let mut out = child
        .wait_with_output()
        .map_err(|e| format!("Failed to read the subprocess output: {e}"))?;
    out.status = status;
    Ok(out)
}

/// The crash-recovery subprocess bootstrap check (returns (passed, detail)).
///
/// Three subprocess launches cover both directions of the mechanism:
/// - A: prepare a dirty document + autosave, then exit without running Drop (no clean marker);
/// - B: recover at startup and read the content back for consistency; then leave another dirty
///   document and exit normally (writes the marker);
/// - C: the marker is present → must not recover (even though the autosave directory still
///   holds files).
fn crash_probe_check() -> (bool, String) {
    let dir = std::env::temp_dir().join("workbench-rs-crash-test");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let mut ok = false;
    let detail = match std::env::current_exe() {
        Err(_) => "Cannot locate the current exe".to_string(),
        Ok(exe) => {
            let mut outcome: Result<(), String> = Ok(());
            for phase in ["A", "B", "C"] {
                outcome = match run_probe_phase(&exe, &dir, phase) {
                    Err(e) => Err(e),
                    Ok(o) if !o.status.success() => Err(format!(
                        "Session {phase} failed status={:?} stdout={} stderr={}",
                        o.status,
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    )),
                    Ok(_) => Ok(()),
                };
                if outcome.is_err() {
                    break;
                }
            }
            match outcome {
                Err(e) => e,
                Ok(()) => {
                    let recovered: Vec<String> =
                        read_json_vec(&dir.join("result.json"));
                    let clean = read_json_value(&dir.join("clean.json"));
                    ok = recovered.iter().any(|t| t.contains(RECOVERED_PREFIX))
                        && clean.get("suppressed").and_then(|v| v.as_bool()) == Some(true);
                    format!(
                        "recovered={recovered:?} clean_exit_suppressed={}",
                        clean.get("suppressed").and_then(|v| v.as_bool()).unwrap_or(false)
                    )
                }
            }
        }
    };
    (ok, detail)
}

fn read_json_value(path: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn read_json_vec(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

/// Run the headless selfcheck and exit the process (for a product main's `--selfcheck` mode).
pub fn exit_with(app: &mut AppRuntime) -> ! {
    std::process::exit(run(app))
}

pub fn run(app: &mut AppRuntime) -> i32 {
    let mut checks: Vec<Check> = Vec::new();
    app.frame_tick(0.0);
    // The plugins at assembly time: step 18 disables plugins (after which `plugins_loaded()` is
    // empty), so the assembly probe (step 19) must use this early snapshot.
    let plugins_at_assembly: Vec<String> = app.plugins_loaded().to_vec();

    let has_cmd = |app: &AppRuntime, id: &str| app.command_title(id).is_some();
    let has_plugin = |app: &AppRuntime, id: &str| app.plugins_loaded().contains(&id.to_string());

    // 1. Assembly: the example Workbench's commands are registered (probing: skipped without the
    //    example Workbench)
    if has_cmd(app, "example.add_shape") {
        checks.push(Check::new(
            "Assembly: example commands registered",
            true,
            format!("example.add_shape = {:?}", app.command_title("example.add_shape")),
        ));
    } else {
        checks.push(Check::skip(
            "Assembly: example commands registered",
            "Example Workbench (wb-example) not assembled",
        ));
    }

    // 2. Open the canvas view and check the context snapshot
    if has_cmd(app, "example.open_canvas") {
        let _ = app.execute_command("example.open_canvas", &api::CommandArgs::default());
        app.frame_tick(0.0);
        let has_doc = app.snapshot().active_document.is_some();
        let canvas_active = app.snapshot().active_view_is("example.canvas");
        checks.push(Check::new(
            "Context: default document + canvas view",
            has_doc && canvas_active,
            format!("doc={has_doc} view={canvas_active}"),
        ));
    } else {
        checks.push(Check::skip(
            "Context: default document + canvas view",
            "Example Workbench not assembled",
        ));
    }

    // 3. Sync command + document change (judged by the document revision growing; domain
    //    agnostic)
    if has_cmd(app, "example.add_shape") {
        let doc_id = app.snapshot().active_document.as_ref().map(|d| d.id);
        let rev_before = doc_id.and_then(|id| app.host.docs.revision(id));
        let mut added = true;
        for _ in 0..3 {
            let r = app.execute_command("example.add_shape", &api::CommandArgs::default());
            added &= r.is_ok();
        }
        let rev_after = doc_id.and_then(|id| app.host.docs.revision(id));
        let grew = matches!((rev_before, rev_after), (Some(b), Some(a)) if a > b);
        checks.push(Check::new(
            "Commands: sync command changes the document",
            added && grew,
            format!("added={added} revision {rev_before:?}→{rev_after:?}"),
        ));
    } else {
        checks.push(Check::skip(
            "Commands: sync command changes the document",
            "Example Workbench not assembled",
        ));
    }

    // 4. Undo / redo
    if has_cmd(app, "example.add_shape") {
        app.execute_command("app.undo", &api::CommandArgs::default());
        let after_undo = shape_count(app);
        app.execute_command("app.redo", &api::CommandArgs::default());
        let after_redo = shape_count(app);
        checks.push(Check::new(
            "Undo/redo",
            after_undo == 2 && after_redo == 3,
            format!("undo={after_undo} redo={after_redo}"),
        ));
    } else {
        checks.push(Check::skip("Undo/redo", "Example Workbench not assembled"));
    }

    // 5. Failing command propagation
    if has_cmd(app, "example.failing_cmd") {
        let r = app.execute_command("example.failing_cmd", &api::CommandArgs::default());
        checks.push(Check::new(
            "Errors: a failing command returns Failed without crashing",
            matches!(r, api::CommandResult::Failed { .. }),
            format!("{r:?}"),
        ));
    } else {
        checks.push(Check::skip(
            "Errors: a failing command returns Failed without crashing",
            "Example Workbench not assembled",
        ));
    }

    // 6. Background task: progress + revision-checked commit
    if has_cmd(app, "example.generate_shapes") {
        let before = shape_count(app);
        let _ = app.execute_command("example.generate_shapes", &api::CommandArgs::default());
        let done = pump_until_idle(app, Duration::from_secs(20));
        let after = shape_count(app);
        checks.push(Check::new(
            "Background tasks: completion + document commit",
            done && after > before,
            format!("done={done} shapes {before}→{after}"),
        ));
    } else {
        checks.push(Check::skip(
            "Background tasks: completion + document commit",
            "Example Workbench not assembled",
        ));
    }

    // 7. Background task: cooperative cancellation
    if has_cmd(app, "example.slow_job") {
        let _ = app.execute_command("example.slow_job", &api::CommandArgs::default());
        std::thread::sleep(Duration::from_millis(200));
        let task_id = app
            .host
            .tasks
            .all()
            .find(|t| t.title.contains("Slow Task"))
            .map(|t| t.id);
        if let Some(id) = task_id {
            app.host.tasks.request_cancel(id);
        }
        let settled = pump_until_idle(app, Duration::from_secs(10));
        let status = task_id.and_then(|id| app.host.tasks.get(id)).map(|t| t.status);
        checks.push(Check::new(
            "Background tasks: cooperative cancel → Cancelled",
            settled && status == Some(api::TaskStatus::Cancelled),
            format!("status={status:?}"),
        ));
    } else {
        checks.push(Check::skip(
            "Background tasks: cooperative cancel → Cancelled",
            "Example Workbench not assembled",
        ));
    }

    // 8. Save / open (precondition: an active document exists)
    let doc_id = app.snapshot().active_document.as_ref().map(|d| d.id);
    if doc_id.is_some() {
        let dir = std::env::temp_dir().join("workbench-rs-selfcheck");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "check.{}",
            doc_id
                .and_then(|id| app.host.docs.get(id))
                .and_then(|d| app.host.docs.type_of(&d.type_id).and_then(|t| t.extensions.first().cloned()))
                .unwrap_or_else(|| "dat".to_string())
        ));
        let _ = std::fs::remove_file(&path);
        let location = api::Location::local(&path);
        let id = doc_id.expect("checked above");
        // Both go through the storage provider off the UI thread; pump until
        // their results have been applied.
        let started = crate::platform_cmds::save_document(
            &mut api::CommandCtx::new(app),
            id,
            Some(location.clone()),
        );
        let saved_ok = !matches!(started, api::CommandResult::Failed { .. })
            && pump_until_idle(app, Duration::from_secs(10))
            && path.exists()
            && app.host.docs.get(id).is_some_and(|d| d.location.as_ref() == Some(&location));
        crate::platform_cmds::open_document(&mut api::CommandCtx::new(app), location.clone());
        let opened = pump_until_idle(app, Duration::from_secs(10))
            && app
                .host
                .docs
                .active()
                .is_some_and(|d| d.id != id && d.location.as_ref() == Some(&location));
        checks.push(Check::new(
            "Documents: save + open",
            saved_ok && opened,
            format!("saved={saved_ok} opened={opened}"),
        ));
    } else {
        checks.push(Check::skip("Documents: save + open", "No active document"));
    }

    // 9. Layout persistence: a layout with an unknown panel restores safely (acceptance #9)
    let mut layout = app.host.ws.save_layout();
    layout.panels.push(api::SavedPanel {
        id: "ghost.panel".to_string(),
        area: api::DockArea::Left,
        order: 99,
        size: 200.0,
        visible: true,
    });
    let skipped = app.host.ws.apply_layout(&layout);
    let tab_restored = !app.host.ws.tabs.is_empty();
    checks.push(Check::new(
        "Layout: unknown panel skipped, tabs restored",
        skipped.iter().any(|m| m.contains("ghost.panel")) && tab_restored,
        format!("skipped={} tabs={}", skipped.len(), app.host.ws.tabs.len()),
    ));

    // 10. (Phase 2) Async command: completes on the shared executor + its invocation record
    //     reaches a final state
    if has_cmd(app, "example.async_scan") {
        let r = app.execute_command("example.async_scan", &api::CommandArgs::default());
        let submitted = r.is_ok();
        let done = pump_until_idle(app, Duration::from_secs(15));
        let async_record_ok = app
            .recent_invocations()
            .find(|rec| rec.command == "example.async_scan")
            .map(|rec| rec.status == api::InvocationStatus::Succeeded && rec.task_id.is_some())
            .unwrap_or(false);
        checks.push(Check::new(
            "Phase 2: async command (shared executor) completes, its record reaches a final state",
            submitted && done && async_record_ok,
            format!("submitted={submitted} done={done} record_ok={async_record_ok}"),
        ));
    } else {
        checks.push(Check::skip(
            "Phase 2: async command (shared executor)",
            "Example Workbench not assembled",
        ));
    }

    // 11. (Phase 2) Unified invocation results: sync commands are final at once; an
    //     unregistered command leaves a queryable failure record
    let known_cmd = if has_cmd(app, "example.open_welcome") {
        Some("example.open_welcome".to_string())
    } else {
        None
    };
    if let Some(known) = known_cmd {
        let inv = app.execute_command_tracked(&known, &api::CommandArgs::default());
        let sync_ok = inv.is_ok()
            && inv.record.status == api::InvocationStatus::Succeeded
            && inv.task_id.is_none();
        let inv_bad = app.execute_command_tracked("no.such.cmd", &api::CommandArgs::default());
        let unknown_ok = !inv_bad.is_ok()
            && inv_bad.record.status == api::InvocationStatus::Failed
            && inv_bad.record.execution.is_none();
        checks.push(Check::new(
            "Phase 2: unified invocation results (sync final state + unregistered queryable)",
            sync_ok && unknown_ok,
            format!(
                "sync_ok={sync_ok} unknown_ok={unknown_ok} history={}",
                app.recent_invocations().count()
            ),
        ));
    } else {
        checks.push(Check::skip(
            "Phase 2: unified invocation results",
            "The product has no registered commands",
        ));
    }

    // 12. (Phase 2) Document revision query API (the data the commit policy builds on;
    //     precondition: an active document)
    if app.snapshot().active_document.is_some() {
        let revision_ok = app
            .snapshot()
            .active_document
            .as_ref()
            .map(|d| app.host.docs.revision(d.id) == Some(d.revision))
            .unwrap_or(false);
        checks.push(Check::new(
            "Phase 2: document revision query API",
            revision_ok,
            format!("revision_ok={revision_ok}"),
        ));
    } else {
        checks.push(Check::skip("Phase 2: document revision query API", "No active document"));
    }

    // 13. (Phase 3) Rhai script plugin: discovery, command registration and ribbon
    //     contributions (precondition: the rhai demo plugin's commands are present)
    if has_plugin(app, "com.example.demo") {
        let plugin_cmd = app.command_title("rhai.hello");
        let toolbar_contrib = app.host.ws.ribbon.iter().any(|t| {
            t.groups
                .iter()
                .any(|g| g.id.0 == "plugin.rhai" && g.items.len() >= 2)
        });
        let script_panel = app
            .host
            .ws
            .panels
            .iter()
            .any(|p| p.id.0 == "rhai.panel" && p.script.is_some());
        let plugin_ok = plugin_cmd.is_some() && toolbar_contrib && script_panel;
        checks.push(Check::new(
            "Phase 3: Rhai script plugin discovery/registration/toolbar/script panel",
            plugin_ok,
            format!(
                "rhai.hello={plugin_cmd:?} toolbar={toolbar_contrib} script_panel={script_panel} loaded={:?}",
                app.plugins_loaded()
            ),
        ));
    } else {
        checks.push(Check::skip(
            "Phase 3: Rhai script plugin discovery/registration/toolbar/script panel",
            "rhai demo plugin not loaded",
        ));
    }

    // 14. (Phase 3) Plugin command execution: a sync note comes back + a background Rhai task
    //     completes
    if has_cmd(app, "rhai.hello") {
        let inv = app.execute_command_tracked(
            "rhai.hello",
            &api::CommandArgs::new().with("name", "selfcheck"),
        );
        let hello_ok = match &inv.result {
            api::CommandResult::Done { note } => note.as_deref().map(|n| n.contains("Hello")).unwrap_or(false),
            _ => false,
        };
        let r = app.execute_command("rhai.progress", &api::CommandArgs::default());
        let bg_done = pump_until_idle(app, Duration::from_secs(20));
        let rhai_task_ok = app.host.tasks.all().any(|t| {
            t.title.contains("Rhai") && t.status == api::TaskStatus::Succeeded
        });
        checks.push(Check::new(
            "Phase 3: plugin command execution (sync note + background progress task)",
            hello_ok && r.is_ok() && bg_done && rhai_task_ok,
            format!("hello_ok={hello_ok} bg_done={bg_done} rhai_task_ok={rhai_task_ok}"),
        ));
    } else {
        checks.push(Check::skip(
            "Phase 3: plugin command execution",
            "rhai demo plugin not loaded",
        ));
    }

    // 15. (Ribbon) Tab model (precondition: the default tab exists — the platform's "View"
    //     group merges into it)
    let ribbon_ok = if app.host.ws.ribbon.iter().any(|t| t.id.0 == "app.home") {
        let ws = &mut app.host.ws;
        let home = ws.ribbon.iter().find(|t| t.id.0 == "app.home");
        let home_groups = home.map(|t| t.groups.len()).unwrap_or(0);
        // Legacy groups (the platform's "View" group) merge into the default tab
        let has_window_group = home
            .map(|t| t.groups.iter().any(|g| g.id.0 == "app.window"))
            .unwrap_or(false);
        // Active tab switching + layout persistence round trip: switching is tested only when
        // there is a second tab
        let other = ws
            .ribbon
            .iter()
            .find(|t| t.id.0 != "app.home")
            .map(|t| t.id.0.clone());
        let (restored_active, restored) = match &other {
            Some(id) => {
                ws.set_active_ribbon_tab(id);
                let saved = ws.save_layout();
                let active_ok = saved.ribbon_active_tab.as_deref() == Some(id.as_str());
                let skipped = ws.apply_layout(&saved);
                (active_ok, ws.active_ribbon_tab == *id && skipped.is_empty())
            }
            None => (false, false),
        };
        if let Some(id) = &other {
            ws.set_active_ribbon_tab(id);
        }
        // With a single tab there is nothing to switch to; the round-trip is
        // only meaningful when a second tab exists.
        let switching_ok = other
            .as_ref()
            .map(|_| restored_active && restored)
            .unwrap_or(true);
        let ok = home_groups >= 1 && has_window_group && switching_ok;
        (
            ok,
            format!(
                "home_groups={home_groups} window={has_window_group} other_tab={} persist={restored_active}/{restored}",
                other.as_deref().unwrap_or("(none)")
            ),
        )
    } else {
        (false, "No default ribbon tab".to_string())
    };
    checks.push(Check::new(
        "Ribbon: tab merging/legacy compatibility/active tab persistence",
        ribbon_ok.0,
        ribbon_ok.1,
    ));

    // 16. (Phase 5) Settings service: write → reload → read back
    let settings_ok = {
        let ok = app.host.settings.set("selfcheck.key", serde_json::json!(42)).is_ok();
        let (reloaded, err) = api::Settings::load(app.state().clone(), crate::app::SETTINGS);
        ok && err.is_none() && reloaded.get_f64("selfcheck.key", 0.0) == 42.0
    };
    checks.push(Check::new(
        "Phase 5: settings service persistence round trip",
        settings_ok,
        format!("settings_ok={settings_ok}"),
    ));

    // 17. (Phase 5) Autosave + crash recovery: subprocess launches make up a real crash —
    //     session A makes the product's own default document dirty and autosaves it, then
    //     exits without running Drop (no clean marker) → session B recovers at startup and
    //     checks the recovered document's content matches the bytes A persisted. The probe
    //     only uses the platform document service (rename + commit a content duplicate) and is
    //     domain agnostic, so it runs for any product that registers a document type (a product
    //     without --selfcheck-crash is caught by the subprocess timeout and FAILs instead of
    //     hanging).
    if !app.host.docs.types().is_empty() {
        let crash_ok = crash_probe_check();
        checks.push(Check::new(
            "Phase 5: autosave + crash recovery",
            crash_ok.0,
            crash_ok.1,
        ));
    } else {
        checks.push(Check::skip(
            "Phase 5: autosave + crash recovery",
            "The product registers no document types",
        ));
    }

    // 18. (Phase 5) Runtime plugin disable: commands removed + ribbon cleaned + state mirrored
    // Note: must run last (the rhai demo plugin's commands are gone once it is disabled)
    if has_plugin(app, "com.example.demo") {
        let disable_ok = {
            let before = app.command_title("rhai.hello").is_some();
            let r = app.execute_command(
                "app.plugins.disable",
                &api::CommandArgs::new().with("id", "com.example.demo"),
            );
            let after_gone = app.command_title("rhai.hello").is_none();
            let ribbon_clean = !app.host.ws.ribbon.iter().any(|t| {
                t.groups.iter().any(|g| g.id.0 == "plugin.rhai")
            });
            let mirrored = app
                .host
                .plugins
                .iter()
                .find(|p| p.id == "com.example.demo")
                .map(|p| p.state == "Disabled")
                .unwrap_or(false);
            (
                before && r.is_ok() && after_gone && ribbon_clean && mirrored,
                format!(
                    "before={before} removed={:?} gone={after_gone} ribbon_clean={ribbon_clean} mirrored={mirrored}",
                    r.is_ok()
                ),
            )
        };
        checks.push(Check::new(
            "Phase 5: runtime plugin disable",
            disable_ok.0,
            disable_ok.1,
        ));
    } else {
        checks.push(Check::skip("Phase 5: runtime plugin disable", "demo plugin not loaded"));
    }

    // 19. Assembly probes: view types / document types / plugins (what any product should
    //     have registered)
    let view_types: Vec<String> = app
        .host
        .ws
        .view_type_ids()
        .map(|t| t.0.clone())
        .collect();
    checks.push(Check::new(
        "Assembly: view types registered",
        !view_types.is_empty(),
        format!("views={view_types:?}"),
    ));

    let doc_types: Vec<String> = app
        .host
        .docs
        .types()
        .iter()
        .map(|t| t.id.0.clone())
        .collect();
    checks.push(Check::new(
        "Assembly: document types registered",
        !doc_types.is_empty(),
        format!("doc_types={doc_types:?}"),
    ));

    checks.push(Check::new(
        "Assembly: plugins loaded",
        !plugins_at_assembly.is_empty(),
        format!("plugins={plugins_at_assembly:?}"),
    ));

    // Summary
    let failed = checks.iter().filter(|c| !c.ok && !c.skipped).count();
    let skipped = checks.iter().filter(|c| c.skipped).count();
    for c in &checks {
        let tag = if c.skipped {
            "SKIP"
        } else if c.ok {
            "PASS"
        } else {
            "FAIL"
        };
        println!("[{}] {} — {}", tag, c.name, c.detail);
    }
    let run_total = checks.len() - skipped;
    println!(
        "selfcheck: {}/{} passed ({} skipped)",
        run_total - failed,
        run_total,
        skipped
    );
    if failed == 0 {
        0
    } else {
        1
    }
}

/// Read the shape count through the document type's serialization interface (the platform
/// knows nothing of domain types).
fn shape_count(app: &AppRuntime) -> usize {
    let Some(summary) = app.snapshot().active_document.as_ref() else {
        return 0;
    };
    let doc_id = summary.id;
    let Some(doc) = app.host.docs.get(doc_id) else {
        return 0;
    };
    let Some(def) = app.host.docs.type_of(&doc.type_id) else {
        return 0;
    };
    let Ok(bytes) = (def.serialize)(doc.content.as_ref()) else {
        return 0;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("shapes").and_then(|s| s.as_array()).map(|a| a.len()))
        .unwrap_or(0)
}

/// Call frame_tick repeatedly until no task is running and no I/O is in flight (or timeout).
fn pump_until_idle(app: &mut AppRuntime, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        app.frame_tick(0.016);
        let running = app.host.tasks.all().any(|t| !t.status.is_terminal())
            || app.pending_io() > 0;
        if !running {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
