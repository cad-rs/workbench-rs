//! 设置服务（design.md §12.4）：应用级/产品级/用户级/插件级设置命名空间。
//!
//! 第一阶段实现为扁平点分键（`autosave.interval_secs`、`plugin.<id>.enabled`）
//! 的 JSON 持久化；写入立即落盘，读取走内存缓存。
//!
//! 设置项的**类型与元数据**由 [`SettingSpec`] 描述：平台与插件在装配期经
//! [`crate::Registry::register_setting`] 登记，前端设置面板按 spec 渲染控件
//! 并经 `app.settings.set` 命令写回（写入侧做类型校验与范围钳制）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// A registered setting's value type (drives the control the settings panel
/// renders and the validation `app.settings.set` performs).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    /// On/off; rendered as a toggle.
    Bool,
    /// Integer in `min..=max` with `step` increments; rendered as a stepper.
    Int { min: i64, max: i64, step: i64 },
    /// One of a fixed option list; rendered as a cycle/select control.
    Enum(Vec<String>),
    /// Free-form string (display-only until the text input lands).
    Str,
}

/// Where a setting's value comes from (Phase 1: user only; product-scope
/// read-only defaults arrive with the enterprise-policy milestone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingScope {
    /// Editable by the user, persisted to `settings.json`.
    #[default]
    User,
}

/// Metadata for one setting key: type, label, default and group. Registered
/// by the platform (built-ins) and by plugins; the settings panel renders one
/// row per spec, the setter validates against it.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    /// Dotted key (`ui.theme`, `autosave.interval_secs`, `plugin.<id>.…`).
    pub key: String,
    /// Human-readable label for the settings panel.
    pub label: String,
    /// Panel group heading (`"常规"`, `"外观"`, …).
    pub group: String,
    pub kind: SettingKind,
    /// Default applied when no user value is stored.
    pub default: Value,
    pub scope: SettingScope,
}

impl SettingSpec {
    /// Validate `value` against this spec: type check, integer clamping into
    /// `min..=max`, enum membership. Returns the normalized value.
    pub fn validate(&self, value: Value) -> Result<Value, String> {
        match &self.kind {
            SettingKind::Bool => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| format!("{} 需要布尔值", self.key)),
            SettingKind::Int { min, max, .. } => {
                let n = value
                    .as_i64()
                    .ok_or_else(|| format!("{} 需要整数", self.key))?;
                Ok(Value::Number(n.clamp(*min, *max).into()))
            }
            SettingKind::Enum(options) => {
                let s = value
                    .as_str()
                    .ok_or_else(|| format!("{} 需要字符串", self.key))?;
                if options.iter().any(|o| o == s) {
                    Ok(Value::String(s.to_string()))
                } else {
                    Err(format!("{s} 不在 {} 的可选项 {options:?}", self.key))
                }
            }
            SettingKind::Str => value
                .as_str()
                .map(|s| Value::String(s.to_string()))
                .ok_or_else(|| format!("{} 需要字符串", self.key)),
        }
    }

    /// The value to use when nothing is stored: the spec default, or the
    /// first enum option when the default was not one of the options.
    pub fn effective_default(&self) -> Value {
        if let SettingKind::Enum(options) = &self.kind {
            if !self.default.as_str().is_some_and(|s| options.iter().any(|o| o == s)) {
                return Value::String(options[0].clone());
            }
        }
        self.default.clone()
    }
}


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

    /// Seed a default into memory only (no disk write): used at build time so
    /// a registered spec's default is readable before the user ever changes
    /// it, without polluting settings.json with untouched defaults.
    pub fn seed(&mut self, key: &str, value: Value) {
        self.values.entry(key.to_string()).or_insert(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(kind: SettingKind, default: Value) -> SettingSpec {
        SettingSpec {
            key: "test.key".into(),
            label: "Test".into(),
            group: "测试".into(),
            kind,
            default,
            scope: SettingScope::User,
        }
    }

    #[test]
    fn validate_bool() {
        let s = spec(SettingKind::Bool, json!(false));
        assert_eq!(s.validate(json!(true)), Ok(json!(true)));
        assert!(s.validate(json!("yes")).is_err());
    }

    #[test]
    fn validate_int_clamps_into_range() {
        let s = spec(SettingKind::Int { min: 5, max: 300, step: 5 }, json!(30));
        assert_eq!(s.validate(json!(60)), Ok(json!(60)));
        assert_eq!(s.validate(json!(1)), Ok(json!(5)), "clamped to min");
        assert_eq!(s.validate(json!(9999)), Ok(json!(300)), "clamped to max");
        assert!(s.validate(json!("30")).is_err());
    }

    #[test]
    fn validate_enum_membership() {
        let s = spec(
            SettingKind::Enum(vec!["dark".into(), "light".into()]),
            json!("dark"),
        );
        assert_eq!(s.validate(json!("light")), Ok(json!("light")));
        assert!(s.validate(json!("blue")).is_err());
    }

    #[test]
    fn effective_default_repairs_bad_enum_default() {
        let s = spec(
            SettingKind::Enum(vec!["dark".into(), "light".into()]),
            json!("blue"),
        );
        assert_eq!(s.effective_default(), json!("dark"));
        let ok = spec(
            SettingKind::Enum(vec!["dark".into(), "light".into()]),
            json!("light"),
        );
        assert_eq!(ok.effective_default(), json!("light"));
    }
}
