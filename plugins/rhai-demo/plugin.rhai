// Rhai demo plugin entry: registers commands, toolbar items, one script UI
// panel (ui/main.rhai) and one script central view (ui/view.rhai) through the
// host-injected global functions.

fn register() {
    // Sync command: runs on the UI thread; a returned string is the note (() means no note)
    register_command("rhai.hello", "Rhai Hello", "hello");
    add_toolbar_item("plugin.rhai", "rhai.hello", "Hello");

    // Background command: runs on a task thread; the `task` object provides progress / logging
    // / cooperative cancellation
    register_background_command("rhai.progress", "Rhai Background Task", "progress_job");
    add_toolbar_item("plugin.rhai", "rhai.progress", "Background tasks");

    // Script UI panel: its content is defined entirely by the UiNode tree in ui/main.rhai
    register_panel("rhai.panel", "Rhai Demo", "ui/main.rhai", "right");

    // Script central view: the same mechanism, mounted in a central tab instead of a dock panel
    register_view("rhai.view", "Rhai View", "ui/view.rhai");

    log_info("Rhai demo plugin registered (2 commands, a toolbar group, 1 script panel, 1 script view)");
}

// Sync callback: args is the command argument map (string keys and values); returns the note
fn hello(args) {
    let name = args.name;
    if name == "" {
        "Hello from the Rhai script"
    } else {
        `Hello from the Rhai script ({name})`
    }
}

// Background callback: task.report/set_stage/log drive the status bar; task.check_cancelled()
// throws the host-recognised cancellation sentinel once cancelled (the task then settles in
// the Cancelled terminal state)
fn progress_job(task, args) {
    let steps = 20;
    for i in 0..steps {
        task.check_cancelled();
        task.report(i.to_float() / steps.to_float(), `Processing batch {i + 1}/{steps}`);
        if i == 10 {
            task.log("Halfway done (Rhai background task)");
        }
    }
    task.set_stage("Finishing");
    task.report_progress(1.0);
}
