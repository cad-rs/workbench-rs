//! The settings service (design.md §12.4): app-, product-, user- and plugin-level setting
//! namespaces.
//!
//! Phase 1 implements it as JSON persistence of flat dotted keys (`autosave.interval_secs`,
//! `plugin.<id>.enabled`); writes hit the disk immediately, reads go through an in-memory cache.
//!
//! A setting's **type and metadata** are described by a [`SettingSpec`]: the platform and
//! plugins register them during assembly via [`crate::Registry::register_setting`], the
//! frontend's settings dialog renders controls from the spec and writes back through the
//! `app.settings.set` command (the write side does type checking and range clamping).
//!
//! The store itself lives in `product_kit` (shared with the nanobot harness); this module
//! keeps the workbench schema layer and the adapter that loads the document from the app's
//! keyed [`StateStore`].

use std::sync::Arc;

use serde_json::Value;

use crate::state::StateStore;

pub use product_kit::Settings;

/// A registered setting's value type (drives the control the settings dialog
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
/// by the platform (built-ins) and by plugins; the settings dialog renders one
/// row per spec, the setter validates against it.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    /// Dotted key (`ui.theme`, `autosave.interval_secs`, `plugin.<id>.…`).
    pub key: String,
    /// Human-readable label for the settings dialog.
    pub label: String,
    /// Panel group heading (`"General"`, `"Appearance"`, …).
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
                .ok_or_else(|| format!("{} needs a boolean", self.key)),
            SettingKind::Int { min, max, .. } => {
                let n = value
                    .as_i64()
                    .ok_or_else(|| format!("{} needs an integer", self.key))?;
                Ok(Value::Number(n.clamp(*min, *max).into()))
            }
            SettingKind::Enum(options) => {
                let s = value
                    .as_str()
                    .ok_or_else(|| format!("{} needs a string", self.key))?;
                if options.iter().any(|o| o == s) {
                    Ok(Value::String(s.to_string()))
                } else {
                    Err(format!("{s} is not one of the options for {}: {options:?}", self.key))
                }
            }
            SettingKind::Str => value
                .as_str()
                .map(|s| Value::String(s.to_string()))
                .ok_or_else(|| format!("{} needs a string", self.key)),
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

/// Adapts a keyed [`StateStore`] to product-kit's single-document settings
/// store: the document lives under `key`.
struct KeyedStateStore(Arc<dyn StateStore>, String);

impl product_kit::SettingsStore for KeyedStateStore {
    fn read(&self) -> Option<Vec<u8>> {
        self.0.read(&self.1)
    }

    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        self.0.write(&self.1, bytes)
    }

    fn describe(&self) -> Option<String> {
        Some(self.1.clone())
    }
}

/// Load settings from `key` in the app's state store; empty when missing,
/// reset with a message when it fails to parse.
pub fn load_settings(store: Arc<dyn StateStore>, key: &str) -> (Settings, Option<String>) {
    Settings::load(Arc::new(KeyedStateStore(store, key.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(kind: SettingKind, default: Value) -> SettingSpec {
        SettingSpec {
            key: "test.key".into(),
            label: "Test".into(),
            group: "Test".into(),
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
