//! Product identity and branding (design.md §13.3).
//!
//! A product ships a `product.toml` next to its executable; the loader lives
//! in workbench-core (this crate stays toml-free). The profile is the single
//! source of truth for the product id, display name, version, logo asset
//! path, brand accent and About-dialog content — the ribbon tab strip and
//! the About dialog read it through the runtime.

use serde::Deserialize;

/// Identity + branding of one product build, loaded from `product.toml`.
///
/// Only `id` and `name` are required; every other field falls back to a
/// platform default (missing version → the framework version, missing logo →
/// no ribbon logo, missing accent → the theme accent).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProductProfile {
    /// Reverse-DNS product id (`com.rcad.creator`); namespaces the user
    /// config directory and plugin filters.
    pub id: String,
    /// Product display name: window title, ribbon tab strip label, About
    /// dialog title.
    pub name: String,
    /// Marketing version shown in the About dialog and update checks.
    /// Falls back to the framework version when empty.
    pub version: String,
    /// Logo icon resource path (svg), drawn in the ribbon tab strip and the
    /// About dialog. Resolved through the product [`crate::AssetSource`].
    pub logo: Option<String>,
    /// Brand accent color (`#RRGGBB`); consumed by the theme (Phase 1 keeps
    /// the current hardcoded accent until the theme API takes this over).
    pub accent: Option<String>,
    /// Product homepage shown as a link in the About dialog.
    pub about_url: Option<String>,
    /// Support page shown as a link in the About dialog.
    pub support_url: Option<String>,
    /// Copyright line for the About dialog.
    pub copyright: Option<String>,
    /// License text (short) for the About dialog.
    pub license_text: Option<String>,
    /// Default UI locale from `[i18n]` (e.g. `"zh-CN"` / `"en"`); seeds the
    /// `ui.locale` setting when the user has not chosen one.
    pub default_locale: Option<String>,
}

impl Default for ProductProfile {
    fn default() -> Self {
        Self {
            id: "com.example.workbench".to_string(),
            name: "Workbench".to_string(),
            version: String::new(),
            logo: None,
            accent: None,
            about_url: None,
            support_url: None,
            copyright: None,
            license_text: None,
            default_locale: None,
        }
    }
}

impl ProductProfile {
    /// The version to display: the profile's own version, falling back to the
    /// framework version when the product did not pin one.
    pub fn display_version(&self) -> &str {
        if self.version.is_empty() {
            crate::VERSION
        } else {
            &self.version
        }
    }
}

/// Raw `product.toml` shape: `[product]` for identity, `[branding]` for the
/// visual brand, `[about]` for the About dialog. All sections optional
/// except `[product]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ProductToml {
    pub product: ProductSection,
    pub branding: BrandingSection,
    pub about: AboutSection,
    pub i18n: I18nSection,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ProductSection {
    pub id: String,
    pub name: String,
    pub version: String,
    pub logo: Option<String>,
}

impl Default for ProductSection {
    fn default() -> Self {
        let d = ProductProfile::default();
        Self {
            id: d.id,
            name: d.name,
            version: d.version,
            logo: d.logo,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BrandingSection {
    pub accent: Option<String>,
    pub about_url: Option<String>,
    pub support_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AboutSection {
    pub copyright: Option<String>,
    pub license_text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct I18nSection {
    /// Default UI locale (`"zh-CN"` / `"en"` / …); seeds `ui.locale`.
    pub default_locale: Option<String>,
}

impl From<ProductToml> for ProductProfile {
    fn from(raw: ProductToml) -> Self {
        Self {
            id: raw.product.id,
            name: raw.product.name,
            version: raw.product.version,
            logo: raw.product.logo,
            accent: raw.branding.accent,
            about_url: raw.branding.about_url,
            support_url: raw.branding.support_url,
            copyright: raw.about.copyright,
            license_text: raw.about.license_text,
            default_locale: raw.i18n.default_locale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sections_to_profile() {
        let raw = ProductToml {
            product: ProductSection {
                id: "com.acme.cad".into(),
                name: "Acme CAD".into(),
                version: "2.1.0".into(),
                logo: Some("icons/acme-logo.svg".into()),
            },
            branding: BrandingSection {
                accent: Some("#FF6600".into()),
                about_url: Some("https://acme.example/cad".into()),
                support_url: Some("https://acme.example/support".into()),
            },
            about: AboutSection {
                copyright: Some("(c) 2026 Acme".into()),
                license_text: Some("Licensed to Acme Corp".into()),
            },
            i18n: I18nSection {
                default_locale: Some("en".into()),
            },
        };
        let profile: ProductProfile = raw.into();
        assert_eq!(profile.id, "com.acme.cad");
        assert_eq!(profile.name, "Acme CAD");
        assert_eq!(profile.version, "2.1.0");
        assert_eq!(profile.logo.as_deref(), Some("icons/acme-logo.svg"));
        assert_eq!(profile.accent.as_deref(), Some("#FF6600"));
        assert_eq!(profile.about_url.as_deref(), Some("https://acme.example/cad"));
        assert_eq!(profile.support_url.as_deref(), Some("https://acme.example/support"));
        assert_eq!(profile.copyright.as_deref(), Some("(c) 2026 Acme"));
        assert_eq!(profile.license_text.as_deref(), Some("Licensed to Acme Corp"));
        assert_eq!(profile.default_locale.as_deref(), Some("en"));
        assert_eq!(profile.display_version(), "2.1.0");
    }

    #[test]
    fn empty_version_falls_back_to_framework_version() {
        let mut profile = ProductProfile::default();
        assert_eq!(profile.display_version(), crate::VERSION);
        profile.version = "9.9".into();
        assert_eq!(profile.display_version(), "9.9");
    }
}
