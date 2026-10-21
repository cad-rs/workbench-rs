//! product.toml discovery and loading (design.md §13.3).
//!
//! A product ships a `product.toml` describing its identity and brand. The
//! standard search order is: next to the executable, then the working
//! directory. Products with a different layout (or dev builds running from
//! `target/debug`) pass their own directories to [`load_from_dirs`].
//!
//! A missing or malformed product.toml is never fatal: the caller keeps the
//! builder's hardcoded defaults (parse problems are reported on stderr so a
//! misconfigured OEM build is visible without a debugger).

use std::path::PathBuf;

use workbench_api::{ProductProfile, ProductToml};

/// Conventional product descriptor file name.
pub const PRODUCT_TOML: &str = "product.toml";

/// Standard search locations: the executable's directory, then the working
/// directory.
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    dirs
}

/// Load the first valid `product.toml` found in `dirs` (search order =
/// directory order). Returns the profile together with the file it came from.
pub fn load_from_dirs(dirs: &[PathBuf]) -> Option<(ProductProfile, PathBuf)> {
    for dir in dirs {
        let path = dir.join(PRODUCT_TOML);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str::<ProductToml>(&text) {
            Ok(raw) => return Some((raw.into(), path)),
            Err(e) => eprintln!("product: failed to parse {}: {e}", path.display()),
        }
    }
    None
}

/// Convenience: [`default_search_dirs`] + [`load_from_dirs`].
pub fn discover() -> Option<(ProductProfile, PathBuf)> {
    load_from_dirs(&default_search_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_product_toml() {
        let dir = std::env::temp_dir().join(format!("wb-product-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(PRODUCT_TOML);
        std::fs::write(
            &path,
            r##"
                [product]
                id = "com.acme.cad"
                name = "Acme CAD"
                version = "2.1.0"
                logo = "icons/acme-logo.svg"

                [branding]
                accent = "#FF6600"
                about_url = "https://acme.example/cad"
                support_url = "https://acme.example/support"

                [about]
                copyright = "(c) 2026 Acme"
                license_text = "Licensed to Acme Corp"
            "##,
        )
        .unwrap();

        let (profile, found) = load_from_dirs(&[dir.clone()]).expect("profile loads");
        assert_eq!(found, path);
        assert_eq!(profile.id, "com.acme.cad");
        assert_eq!(profile.name, "Acme CAD");
        assert_eq!(profile.version, "2.1.0");
        assert_eq!(profile.logo.as_deref(), Some("icons/acme-logo.svg"));
        assert_eq!(profile.accent.as_deref(), Some("#FF6600"));
        assert_eq!(profile.about_url.as_deref(), Some("https://acme.example/cad"));
        assert_eq!(profile.support_url.as_deref(), Some("https://acme.example/support"));
        assert_eq!(profile.copyright.as_deref(), Some("(c) 2026 Acme"));
        assert_eq!(profile.license_text.as_deref(), Some("Licensed to Acme Corp"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_file_falls_through_to_next_dir() {
        let bad = std::env::temp_dir().join(format!("wb-product-bad-{}", std::process::id()));
        let good = std::env::temp_dir().join(format!("wb-product-good-{}", std::process::id()));
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(bad.join(PRODUCT_TOML), "not [ valid toml =").unwrap();
        std::fs::write(
            good.join(PRODUCT_TOML),
            "[product]\nid = \"com.acme.ok\"\nname = \"Ok\"\n",
        )
        .unwrap();

        let (profile, found) = load_from_dirs(&[bad.clone(), good.clone()]).expect("recovers");
        assert_eq!(profile.id, "com.acme.ok");
        assert_eq!(found, good.join(PRODUCT_TOML));

        let _ = std::fs::remove_dir_all(&bad);
        let _ = std::fs::remove_dir_all(&good);
    }

    #[test]
    fn missing_everywhere_returns_none() {
        let empty = std::env::temp_dir().join(format!("wb-product-none-{}", std::process::id()));
        assert!(load_from_dirs(&[empty]).is_none());
    }
}
