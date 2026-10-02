//! 设置服务（design.md §12.4）：应用级/产品级/用户级/插件级设置命名空间。
//!
//! 第一阶段实现为扁平点分键（`autosave.interval_secs`、`plugin.<id>.enabled`）
//! 的 JSON 持久化；写入立即落盘，读取走内存缓存。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// 产品设置存储（UI 线程所有）。
#[derive(Default)]
pub struct Settings {
    values: BTreeMap<String, Value>,
    path: Option<PathBuf>,
}

impl Settings {
    /// 从 JSON 文件加载；文件不存在/解析失败时返回空设置（解析失败记录由调用方处理）。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return (Self { values: BTreeMap::new(), path: Some(path.to_path_buf()) }, None),
        };
        match serde_json::from_str::<BTreeMap<String, Value>>(&text) {
            Ok(values) => (
                Self { values, path: Some(path.to_path_buf()) },
                None,
            ),
            Err(e) => (
                Self { values: BTreeMap::new(), path: Some(path.to_path_buf()) },
                Some(format!("设置文件解析失败，已重置: {e}")),
            ),
        }
    }

    /// 无持久化路径的内存设置（测试用）。
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn get_str(&self, key: &str, default: &str) -> String {
        self.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    }

    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }

    /// 写入并立即落盘（无路径时仅内存）。返回错误消息（如有）。
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        self.values.insert(key.to_string(), value);
        self.save()
    }

    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        self.values.remove(key);
        self.save()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(|k| k.as_str())
    }

    /// 全量保存到关联路径。
    pub fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(&self.values).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, json).map_err(|e| format!("写入设置 {}: {e}", path.display()))
    }
}
