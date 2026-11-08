//! Structured logging: an in-memory ring buffer shown by the frontend in the Output panel,
//! which also feeds the `log` facade (and so the log file; see `workbench_core::diagnostics`).

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn facade(self) -> ::log::Level {
        match self {
            LogLevel::Debug => ::log::Level::Debug,
            LogLevel::Info => ::log::Level::Info,
            LogLevel::Warn => ::log::Level::Warn,
            LogLevel::Error => ::log::Level::Error,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO ",
            LogLevel::Warn => "WARN ",
            LogLevel::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    /// Global sequence number (monotonically increasing; the cursor for the stdout log mirror).
    pub seq: u64,
    pub level: LogLevel,
    /// Seconds since the app started, formatted as `mm:ss.mmm`.
    pub time: String,
    pub message: String,
}

/// The platform log store. Written by the UI thread; the task event pump can also forward
/// writes into it.
pub struct LogStore {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    start: web_time::Instant,
    total_written: u64,
}

impl LogStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            start: web_time::Instant::now(),
            total_written: 0,
        }
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        ::log::log!(target: "workbench", level.facade(), "{message}");
        let elapsed = self.start.elapsed().as_secs_f32();
        let time = format!("{:02}:{:02}.{:03}", (elapsed / 60.0) as u32, (elapsed % 60.0) as u32, ((elapsed * 1000.0) as u32) % 1000);
        self.total_written += 1;
        self.entries.push_back(LogEntry {
            seq: self.total_written,
            level,
            time,
            message,
        });
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }
    pub fn warn(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Warn, message);
    }
    pub fn error(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Error, message);
    }
    pub fn debug(&mut self, message: impl Into<String>) {
        self.log(LogLevel::Debug, message);
    }

    pub fn entries(&self) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter()
    }

    pub fn last(&self) -> Option<&LogEntry> {
        self.entries.back()
    }

    /// The last `n` log entries.
    pub fn tail(&self, n: usize) -> Vec<LogEntry> {
        let len = self.entries.len();
        self.entries.iter().skip(len.saturating_sub(n)).cloned().collect()
    }

    /// The entries whose global sequence number is greater than `cursor` (phase 5: for the
    /// stdout log mirror).
    pub fn since(&self, cursor: u64) -> Vec<LogEntry> {
        self.entries
            .iter()
            .filter(|e| e.seq > cursor)
            .cloned()
            .collect()
    }
}
