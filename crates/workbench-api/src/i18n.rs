//! i18n: locale tables and the translation lookup (Phase 2).
//!
//! Products and plugins contribute `(locale, key, text)` entries at assembly
//! time (Registry::register_locales); the platform merges them into one
//! [`Localizer`] on the ServiceHost. Display sites call `host.tr(key)`:
//! the current locale comes from the `ui.locale` setting, resolution falls
//! back `zh-CN -> zh -> en -> the key itself`, so an untranslated key
//! degrades to its literal text instead of breaking the UI.

use std::collections::HashMap;

/// Merged locale tables: locale -> (key -> text).
///
/// Convention (Qt tr() style): the source-language string is the key. The
/// [`Localizer::source_locale`] therefore needs no table — its keys *are*
/// the display text — and never falls back to `en` (that would translate the
/// source language against its own table).
pub struct Localizer {
    tables: HashMap<String, HashMap<String, String>>,
    /// Locale whose strings double as keys (`zh-CN` for this platform).
    source_locale: String,
}

impl Default for Localizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Localizer {
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            source_locale: "zh-CN".to_string(),
        }
    }

    /// Override the source locale (the one whose strings are the keys).
    pub fn set_source_locale(&mut self, locale: &str) {
        self.source_locale = locale.to_string();
    }

    /// The source locale (keys render verbatim here).
    pub fn source_locale(&self) -> &str {
        &self.source_locale
    }

    /// Merge one locale's entries into the table (later registrations win).
    pub fn add_table(&mut self, locale: &str, entries: impl IntoIterator<Item = (String, String)>) {
        self.tables
            .entry(locale.to_string())
            .or_default()
            .extend(entries);
    }

    /// Translate `key` for `locale` with fallback: exact locale -> base
    /// language (`zh-CN` -> `zh`) -> `en` (unless the locale IS the source
    /// locale) -> None (the caller renders the key itself).
    pub fn translate(&self, locale: &str, key: &str) -> Option<&str> {
        if let Some(text) = self.tables.get(locale).and_then(|t| t.get(key)) {
            return Some(text);
        }
        if let Some(lang) = locale.split(['-', '_']).next() {
            if lang != locale {
                if let Some(text) = self.tables.get(lang).and_then(|t| t.get(key)) {
                    return Some(text);
                }
            }
        }
        // The source locale renders keys verbatim; falling through to `en`
        // here would translate the source language against its own table.
        if locale == self.source_locale {
            return None;
        }
        if locale != "en" {
            if let Some(text) = self.tables.get("en").and_then(|t| t.get(key)) {
                return Some(text);
            }
        }
        None
    }

    /// Whether any table carries `key` (diagnostics/tests).
    pub fn has(&self, key: &str) -> bool {
        self.tables.values().any(|t| t.contains_key(key))
    }

    /// Locales that have at least one entry.
    pub fn locales(&self) -> Vec<&str> {
        let mut locales: Vec<&str> = self.tables.keys().map(|s| s.as_str()).collect();
        locales.sort_unstable();
        locales
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> Localizer {
        let mut l = Localizer::new();
        l.add_table("zh-CN", vec![("greeting".into(), "你好".into())]);
        l.add_table("en", vec![("greeting".into(), "Hello".into())]);
        l
    }

    #[test]
    fn exact_locale_wins() {
        assert_eq!(loc().translate("zh-CN", "greeting"), Some("你好"));
        assert_eq!(loc().translate("en", "greeting"), Some("Hello"));
    }

    #[test]
    fn base_language_fallback() {
        let mut l = Localizer::new();
        l.add_table("zh", vec![("k".into(), "中文".into())]);
        assert_eq!(l.translate("zh-TW", "k"), Some("中文"), "zh-TW -> zh");
    }

    #[test]
    fn english_fallback_then_none() {
        let mut l = Localizer::new();
        l.add_table("en", vec![("k".into(), "EN".into())]);
        assert_eq!(l.translate("fr", "k"), Some("EN"), "unknown locale -> en");
        assert_eq!(l.translate("fr", "missing"), None);
    }

    #[test]
    fn source_locale_never_falls_back_to_english() {
        let mut l = Localizer::new();
        l.add_table("en", vec![("撤销".into(), "Undo".into())]);
        // The source locale (zh-CN) renders keys verbatim: no en fallback.
        assert_eq!(l.translate("zh-CN", "撤销"), None, "source key stays verbatim");
        assert_eq!(l.translate("en", "撤销"), Some("Undo"));
        // An explicit zh-CN override still wins when a table exists.
        l.add_table("zh-CN", vec![("撤销".into(), "撤回".into())]);
        assert_eq!(l.translate("zh-CN", "撤销"), Some("撤回"));
    }

    #[test]
    fn missing_key_reports_false() {
        let l = loc();
        assert!(l.has("greeting"));
        assert!(!l.has("nope"));
        assert_eq!(l.locales(), vec!["en", "zh-CN"]);
    }

    #[test]
    fn later_registration_wins() {
        let mut l = loc();
        l.add_table("en", vec![("greeting".into(), "Hi".into())]);
        assert_eq!(l.translate("en", "greeting"), Some("Hi"));
    }
}
