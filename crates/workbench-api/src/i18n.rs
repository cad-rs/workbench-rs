//! i18n: locale tables and the translation lookup.
//!
//! Products and plugins contribute `(locale, key, text)` entries at assembly
//! time (Registry::register_locales); the platform merges them into one
//! [`Localizer`] on the ServiceHost. Display sites call `host.tr(key)`:
//! the current locale comes from the `ui.locale` setting, resolution falls
//! back `de-AT -> de -> en -> the key itself`, so an untranslated key
//! degrades to its literal text instead of breaking the UI.
//!
//! The platform ships English only; the tables are for products and plugins
//! that add other languages.

use std::collections::HashMap;

/// Merged locale tables: locale -> (key -> text).
///
/// Convention (Qt tr() style): the source-language string is the key. The
/// [`Localizer::source_locale`] therefore needs no table — its keys *are*
/// the display text — and never falls back to `en`.
pub struct Localizer {
    tables: HashMap<String, HashMap<String, String>>,
    /// Locale whose strings double as keys (`en` for this platform).
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
            source_locale: "en".to_string(),
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
    /// language (`de-AT` -> `de`) -> `en` (unless the locale IS the source
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
        // The source locale renders keys verbatim.
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
        l.add_table("de", vec![("greeting".into(), "Hallo".into())]);
        l.add_table("fr", vec![("greeting".into(), "Bonjour".into())]);
        l
    }

    #[test]
    fn exact_locale_wins() {
        assert_eq!(loc().translate("de", "greeting"), Some("Hallo"));
        assert_eq!(loc().translate("fr", "greeting"), Some("Bonjour"));
    }

    #[test]
    fn base_language_fallback() {
        assert_eq!(loc().translate("de-AT", "greeting"), Some("Hallo"), "de-AT -> de");
    }

    #[test]
    fn english_fallback_then_none() {
        let mut l = Localizer::new();
        l.set_source_locale("de");
        l.add_table("en", vec![("k".into(), "EN".into())]);
        assert_eq!(l.translate("fr", "k"), Some("EN"), "unknown locale -> en");
        assert_eq!(l.translate("fr", "missing"), None);
    }

    #[test]
    fn source_locale_renders_keys_verbatim() {
        let mut l = Localizer::new();
        l.add_table("de", vec![("Undo".into(), "Rückgängig".into())]);
        assert_eq!(l.translate("en", "Undo"), None, "source key stays verbatim");
        assert_eq!(l.translate("de", "Undo"), Some("Rückgängig"));
        // An explicit table for the source locale still wins.
        l.add_table("en", vec![("Undo".into(), "Revert".into())]);
        assert_eq!(l.translate("en", "Undo"), Some("Revert"));
    }

    #[test]
    fn missing_key_reports_false() {
        let l = loc();
        assert!(l.has("greeting"));
        assert!(!l.has("nope"));
        assert_eq!(l.locales(), vec!["de", "fr"]);
    }

    #[test]
    fn later_registration_wins() {
        let mut l = loc();
        l.add_table("de", vec![("greeting".into(), "Servus".into())]);
        assert_eq!(l.translate("de", "greeting"), Some("Servus"));
    }
}
