//! What support needs when something goes wrong: a rolling log file and crash
//! reports on the desktop, and the text report the Export Diagnostics command
//! hands over on either platform.

use std::env::consts::{ARCH, OS};

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

fn section(title: &str) -> String {
    format!("\n---- {title} ----")
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
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    use workbench_api as api;

    use crate::WorkbenchAppBuilder;

    /// A log file rolls over at this size…
    const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
    /// …and this many older ones stay (`app.1.log` is the newest of them).
    const KEPT_LOGS: u32 = 3;
    /// How much of the log the diagnostics report includes.
    const REPORTED_LOG_BYTES: u64 = 1024 * 1024;
    /// How much of the log a crash report includes.
    const CRASH_LOG_BYTES: u64 = 64 * 1024;
    /// A panic storm stops writing reports here; the first are the telling ones.
    const MAX_REPORTS_PER_SESSION: u32 = 5;

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

    fn install_panic_hook(
        product: String,
        state: Arc<dyn api::StateStore>,
        log_file: PathBuf,
    ) {
        static WRITTEN: AtomicU32 = AtomicU32::new(0);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let panic = format!(
                "Thread {} panicked at {}: {}",
                thread.name().unwrap_or("(unnamed)"),
                info.location().map_or("(unknown)".to_string(), ToString::to_string),
                info.payload_as_str().unwrap_or("(no message)"),
            );
            let written = WRITTEN.fetch_add(1, Ordering::Relaxed);
            if written < MAX_REPORTS_PER_SESSION {
                let now = chrono::Utc::now();
                let key = format!(
                    "{}crash-{}-{}-{written}.txt",
                    super::CRASH_DIR,
                    now.format("%Y%m%d-%H%M%S"),
                    std::process::id()
                );
                let report = format!(
                    "{product}\nTime: {}\n{panic}\n\nBacktrace:\n{}\nRecent log:\n{}",
                    now.to_rfc3339(),
                    std::backtrace::Backtrace::force_capture(),
                    tail(&log_file, CRASH_LOG_BYTES),
                );
                let _ = state.write(&key, report.as_bytes());
            }
            log::error!(target: "workbench", "{panic}");
            previous(info);
        }));
    }

    /// At most the last `limit` bytes of the file, starting at a whole line.
    fn tail(path: &Path, limit: u64) -> String {
        let Ok(mut file) = File::open(path) else {
            return String::new();
        };
        let start = file.metadata().map_or(0, |meta| meta.len()).saturating_sub(limit);
        let mut bytes = Vec::new();
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
            return String::new();
        }
        let text = String::from_utf8_lossy(&bytes);
        match text.find('\n') {
            Some(end_of_cut_line) if start > 0 => text[end_of_cut_line + 1..].to_string(),
            _ => text.into_owned(),
        }
    }

    /// An append-only file that starts over when full, keeping the last few.
    /// Logging must never take the app down, so failures only lose lines.
    struct RollingFile {
        path: PathBuf,
        file: Option<File>,
        size: u64,
        max_size: u64,
    }

    impl RollingFile {
        fn open(path: PathBuf, max_size: u64) -> Self {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let mut rolling = Self { path, file: None, size: 0, max_size };
            rolling.reopen();
            rolling
        }

        fn reopen(&mut self) {
            self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
            self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        }

        fn write_line(&mut self, line: &str) {
            if self.size > 0 && self.size + line.len() as u64 > self.max_size {
                self.roll_over();
            }
            if let Some(file) = &mut self.file {
                if file.write_all(line.as_bytes()).is_ok() {
                    self.size += line.len() as u64;
                }
            }
        }

        fn roll_over(&mut self) {
            self.file = None; // Windows can't rename a file that is open
            for older in (1..KEPT_LOGS).rev() {
                let _ = fs::rename(self.numbered(older), self.numbered(older + 1));
            }
            let _ = fs::rename(&self.path, self.numbered(1));
            self.reopen();
        }

        /// `app.log` → `app.<n>.log`.
        fn numbered(&self, n: u32) -> PathBuf {
            let stem = self.path.file_stem().unwrap_or_default().to_string_lossy();
            self.path.with_file_name(format!("{stem}.{n}.log"))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn empty_dir(test: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("wb-log-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn read(dir: &Path, name: &str) -> String {
            fs::read_to_string(dir.join(name)).unwrap()
        }

        #[test]
        fn a_full_log_rolls_over_and_the_oldest_is_dropped() {
            let dir = empty_dir("roll");
            let mut log = RollingFile::open(dir.join("app.log"), 20);
            for n in 0..7 {
                log.write_line(&format!("line {n} of the log\n"));
            }
            assert_eq!(read(&dir, "app.log"), "line 6 of the log\n");
            assert_eq!(read(&dir, "app.1.log"), "line 5 of the log\n");
            assert_eq!(read(&dir, "app.2.log"), "line 4 of the log\n");
            assert_eq!(read(&dir, "app.3.log"), "line 3 of the log\n");
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 4, "nothing older is kept");
            drop(log);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_new_session_continues_the_log_and_counts_what_is_there() {
            let dir = empty_dir("continue");
            fs::write(dir.join("app.log"), "from the last session\n").unwrap();
            let mut log = RollingFile::open(dir.join("app.log"), 30);
            log.write_line("short\n");
            assert_eq!(read(&dir, "app.log"), "from the last session\nshort\n");
            log.write_line("this one does not fit\n");
            assert_eq!(read(&dir, "app.log"), "this one does not fit\n");
            assert_eq!(read(&dir, "app.1.log"), "from the last session\nshort\n");
            drop(log);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_log_that_cannot_be_opened_only_loses_lines() {
            let dir = empty_dir("unwritable");
            fs::create_dir(dir.join("app.log")).unwrap();
            let mut log = RollingFile::open(dir.join("app.log"), 20);
            log.write_line("nowhere to go\n");
            drop(log);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_tail_starts_at_a_whole_line() {
            let dir = empty_dir("tail");
            let file = dir.join("log");
            fs::write(&file, "first line\nsecond line\nthird\n").unwrap();
            assert_eq!(tail(&file, 1000), "first line\nsecond line\nthird\n");
            assert_eq!(tail(&file, 15), "third\n");
            assert_eq!(tail(&dir.join("missing"), 15), "");
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
