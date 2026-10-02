//! 结构化日志（内存环形缓冲，由前端展示在 Output 面板）。

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
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
    /// 全局序号（单调递增；stdout 日志镜像的游标用）。
    pub seq: u64,
    pub level: LogLevel,
    /// 应用启动以来的秒数，格式化为 `mm:ss.mmm`。
    pub time: String,
    pub message: String,
}

/// 平台日志存储。UI 线程写入；也可由任务事件泵转发写入。
pub struct LogStore {
    entries: VecDeque<LogEntry>,
    capacity: usize,
    start: std::time::Instant,
    total_written: u64,
}

impl LogStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            start: std::time::Instant::now(),
            total_written: 0,
        }
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        let elapsed = self.start.elapsed().as_secs_f32();
        let time = format!("{:02}:{:02}.{:03}", (elapsed / 60.0) as u32, (elapsed % 60.0) as u32, ((elapsed * 1000.0) as u32) % 1000);
        self.total_written += 1;
        self.entries.push_back(LogEntry {
            seq: self.total_written,
            level,
            time,
            message: message.into(),
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

    /// 截取最后 `n` 条日志。
    pub fn tail(&self, n: usize) -> Vec<LogEntry> {
        let len = self.entries.len();
        self.entries.iter().skip(len.saturating_sub(n)).cloned().collect()
    }

    /// 返回全局序号大于 `cursor` 的条目（阶段 5：stdout 日志镜像用）。
    pub fn since(&self, cursor: u64) -> Vec<LogEntry> {
        self.entries
            .iter()
            .filter(|e| e.seq > cursor)
            .cloned()
            .collect()
    }
}
