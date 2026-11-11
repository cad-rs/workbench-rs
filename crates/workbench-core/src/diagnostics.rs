//! What support needs when something goes wrong: a rolling log file and crash
//! reports on the desktop, and the text report the Export Diagnostics command
//! hands over on either platform.
//!
//! The log file mechanics, the crash-report text and the clock live in
//! `product_kit` (shared with the nanobot harness); this module keeps the
//! workbench-specific parts: the logger, the state-store backend, and the
//! runtime-shaped export.

use std::env::consts::{ARCH, OS};

use product_kit::diagnostics::section;
use workbench_api as api;

use crate::AppRuntime;

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::init;

/// Where crash reports live in the state store: `crash-<UTC time>-….txt`, so
/// their keys sort oldest first.
pub(crate) const CRASH_DIR: &str = "crashes/";
/// How many crash reports are kept.
pub(crate) const KEPT_REPORTS: usize = 20;
/// The setting a product with a crash reporter offers; off until the user
/// turns it on.
pub(crate) const SEND_CRASH_REPORTS: &str = "diagnostics.send_crash_reports";
/// How many crash reports the diagnostics report includes, newest first.
const REPORTED_CRASHES: usize = 5;

/// The crash reports in the state store, oldest first.
pub(crate) fn crash_reports(state: &dyn api::StateStore) -> Vec<String> {
    let mut keys = state.keys(CRASH_DIR);
    keys.retain(|key| key.ends_with(".txt"));
    keys
}

/// Everything support asks for first, as plain text so the user can read what
/// they are about to send. Account details and plugin settings stay out.
pub(crate) fn report(app: &AppRuntime) -> String {
    let host = &app.host;
    let profile = app.profile();
    let docs: Vec<_> = host.docs.docs().collect();
    let mut lines = vec![
        format!("{} {} ({})", profile.name, profile.display_version(), profile.id),
        format!("Framework: {}", api::VERSION),
        format!("GUI backend: {}", host.stats.backend),
        format!("System: {}", format!("{OS} {ARCH}").trim()),
        format!("Uptime: {:.0} s", host.uptime_secs()),
        format!("Frames: {} (average {:.1} ms)", host.stats.frames, host.stats.avg_frame_ms),
        format!(
            "Documents: {} open, {} unsaved",
            docs.len(),
            docs.iter().filter(|doc| doc.dirty).count()
        ),
        section("Plugins"),
    ];
    lines.extend(host.plugins.iter().map(|plugin| match &plugin.detail {
        Some(detail) => format!("{}: {} ({detail})", plugin.id, plugin.state),
        None => format!("{}: {}", plugin.id, plugin.state),
    }));

    lines.push(section("Settings"));
    lines.extend(
        host.setting_specs
            .iter()
            .filter(|spec| !spec.key.starts_with("plugin."))
            .filter_map(|spec| Some(format!("{} = {}", spec.key, host.settings.get(&spec.key)?))),
    );

    lines.push(section("Log"));
    lines.push(log_text(app));

    let state = app.state().as_ref();
    for key in crash_reports(state).iter().rev().take(REPORTED_CRASHES) {
        lines.push(section(key.strip_prefix(CRASH_DIR).unwrap_or(key)));
        if let Some(bytes) = state.read(key) {
            lines.push(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    lines.join("\n")
}

/// The end of the log file, or this session's log when there is no file (the
/// web, or a product with its own logger).
fn log_text(app: &AppRuntime) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(text) = desktop::recent_log() {
        return text;
    }
    let entries: Vec<String> = app
        .host
        .log
        .entries()
        .map(|entry| format!("{} {} {}", entry.time, entry.level.as_str(), entry.message))
        .collect();
    entries.join("\n")
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    use product_kit::diagnostics::{
        crash_report, now_secs, panic_message, tail, utc, utc_rfc3339, utc_stamp, RollingFile,
        CRASH_LOG_BYTES, MAX_LOG_BYTES, MAX_REPORTS_PER_SESSION, REPORTED_LOG_BYTES,
    };
    use workbench_api as api;

    use crate::WorkbenchAppBuilder;

    use super::CRASH_DIR;

    /// The log file, once [`init`] has installed the logger.
    static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();

    /// Start writing the log to a rolling file in the product's config
    /// directory (`logs/app.log`) and a report for every panic. Call it once,
    /// from `main`, when the builder is configured.
    ///
    /// The file gets warnings, plus information from the workbench crates;
    /// `RUST_LOG` replaces that filter, and sets what goes to stderr as usual.
    /// A product that installed its own logger keeps it and gets the crash
    /// reports only.
    pub fn init(builder: &WorkbenchAppBuilder) {
        let log_file = builder.state_dir().join("logs").join("app.log");
        let product = builder.product_line();
        install_logger(&log_file);
        install_panic_hook(
            format!("{product}, {} {}", std::env::consts::OS, std::env::consts::ARCH),
            builder.state_store(),
            log_file,
        );
        log::info!(target: "workbench", "==== {product} started, pid {} ====", std::process::id());
    }

    /// The end of the log file, if this process writes one.
    pub(super) fn recent_log() -> Option<String> {
        Some(tail(LOG_FILE.get()?, REPORTED_LOG_BYTES))
    }

    struct Logger {
        stderr: env_logger::Logger,
        /// Only its filter is used: which records go to the file.
        file_filter: env_logger::Logger,
        file: Mutex<RollingFile>,
    }

    impl log::Log for Logger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            self.stderr.enabled(metadata) || self.file_filter.enabled(metadata)
        }

        fn log(&self, record: &log::Record) {
            if self.stderr.matches(record) {
                self.stderr.log(record);
            }
            if self.file_filter.matches(record) {
                let line = format!(
                    "{} {:<5} {}: {}\n",
                    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
                    record.level(),
                    record.target(),
                    record.args()
                );
                self.file.lock().unwrap_or_else(PoisonError::into_inner).write_line(&line);
            }
        }

        fn flush(&self) {
            self.stderr.flush();
        }
    }

    fn install_logger(log_file: &Path) {
        let stderr = env_logger::Builder::from_default_env().build();
        let mut file_filter = env_logger::Builder::new();
        match std::env::var("RUST_LOG") {
            Ok(filter) => file_filter.parse_filters(&filter),
            Err(_) => file_filter
                .filter_level(log::LevelFilter::Warn)
                .filter_module("workbench", log::LevelFilter::Info),
        };
        let file_filter = file_filter.build();
        let max_level = stderr.filter().max(file_filter.filter());
        let logger = Logger {
            stderr,
            file_filter,
            file: Mutex::new(RollingFile::open(log_file.to_path_buf(), MAX_LOG_BYTES)),
        };
        if log::set_boxed_logger(Box::new(logger)).is_ok() {
            log::set_max_level(max_level);
            let _ = LOG_FILE.set(log_file.to_path_buf());
        }
    }

    fn install_panic_hook(product: String, state: Arc<dyn api::StateStore>, log_file: PathBuf) {
        static WRITTEN: AtomicU32 = AtomicU32::new(0);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let panic = panic_message(&info);
            let written = WRITTEN.fetch_add(1, Ordering::Relaxed);
            if written < MAX_REPORTS_PER_SESSION {
                let time = utc(now_secs());
                let key = format!(
                    "{}crash-{}-{}-{written}.txt",
                    CRASH_DIR,
                    utc_stamp(time),
                    std::process::id()
                );
                let backtrace = std::backtrace::Backtrace::force_capture();
                let report = crash_report(
                    &product,
                    &utc_rfc3339(time),
                    &panic,
                    &backtrace.to_string(),
                    &tail(&log_file, CRASH_LOG_BYTES),
                );
                let _ = state.write(&key, report.as_bytes());
            }
            log::error!(target: "workbench", "{panic}");
            previous(info);
        }));
    }
}
