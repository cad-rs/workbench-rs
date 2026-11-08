//! The log file and crash reports, end to end. A logger and a panic hook can
//! be installed once per process and the panic has to be real, so the test
//! runs them in a child process: this same test binary, re-run.

use std::path::Path;
use std::process::Command;

use workbench_core::WorkbenchAppBuilder;

/// Set in the child process: the state directory to use.
const CHILD_DIR: &str = "WB_DIAGNOSTICS_TEST_DIR";

fn builder(dir: &Path) -> WorkbenchAppBuilder {
    WorkbenchAppBuilder::new("test.diagnostics", "Diagnostics Test")
        .with_layout_path(dir.join("layout.json"))
}

/// Does nothing as an ordinary test; in the child process it logs and panics.
#[test]
fn panicking_child() {
    let Ok(dir) = std::env::var(CHILD_DIR) else {
        return;
    };
    workbench_core::diagnostics::init(&builder(Path::new(&dir)));
    log::info!(target: "workbench_core::child", "a line before the crash");
    log::debug!(target: "workbench_core::child", "a line too detailed for the file");
    log::info!(target: "some_dependency", "a dependency's chatter");
    log::warn!(target: "some_dependency", "a dependency's warning");
    panic!("deliberate panic for the test");
}

#[test]
fn a_panic_leaves_a_log_and_a_report_and_the_next_launch_says_so_once() {
    let dir = std::env::temp_dir().join(format!("workbench-rs-diagnostics-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "panicking_child", "--nocapture"])
        .env(CHILD_DIR, &dir)
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(!child.status.success(), "the child panicked");

    let log = std::fs::read_to_string(dir.join("logs").join("app.log")).unwrap();
    assert!(log.contains("Diagnostics Test"), "{log}");
    assert!(log.contains("a line before the crash"), "{log}");
    assert!(!log.contains("too detailed"), "{log}");
    assert!(log.contains("a dependency's warning"), "{log}");
    assert!(!log.contains("chatter"), "{log}");
    assert!(log.contains("deliberate panic for the test"), "the panic is logged too: {log}");

    let reports: Vec<_> = std::fs::read_dir(dir.join("crashes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = std::fs::read_to_string(&reports[0]).unwrap();
    assert!(report.starts_with("Diagnostics Test"), "{report}");
    assert!(report.contains("deliberate panic for the test"), "{report}");
    assert!(report.contains("diagnostics.rs"), "the panic's location: {report}");
    assert!(report.contains("Backtrace:"), "{report}");
    assert!(report.contains("a line before the crash"), "the recent log: {report}");

    let announced = |app: &workbench_core::AppRuntime| {
        app.host.log.entries().any(|entry| entry.message.contains("A crash was recorded"))
    };
    let next = builder(&dir).build().unwrap();
    assert!(announced(&next));
    drop(next);
    let after_that = builder(&dir).build().unwrap();
    assert!(!announced(&after_that), "once is enough");
    drop(after_that);

    let _ = std::fs::remove_dir_all(&dir);
}
