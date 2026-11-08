//! Plugin manifest (`plugin.toml`) parsing and validation.
//!
//! The structure is language-neutral: `entry_point` points at a Rhai script file (e.g.
//! `plugin.rhai`), optionally naming the entry function after a `:` (default `register`).

use serde::Deserialize;

/// Host API version (follows this crate's release cycle).
pub const HOST_API_VERSION: &str = "1";

#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginInfo,
    pub compatibility: Option<Compatibility>,
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub entry_point: String,
    /// Compiled-in Workbench modules this plugin enables.
    ///
    /// A plugin is a runtime unit: it does not contain code, it turns on
    /// modules that were compiled into the product. A script-only plugin
    /// leaves this out.
    #[serde(default)]
    pub modules: Vec<String>,
    /// The feature this plugin needs, a name the product defines (see
    /// `workbench_api::Entitlements`); without it the plugin is not loaded.
    /// Omitted = no requirement.
    pub requires: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Compatibility {
    pub products: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct Permissions {
    pub document_read: Option<bool>,
    pub document_write: Option<bool>,
    pub filesystem_read: Option<bool>,
    pub network: Option<bool>,
}

/// Split an entry point: `plugin.rhai` or `plugin.rhai:register`.
pub fn split_entry_point(entry: &str) -> (String, String) {
    match entry.split_once(':') {
        Some((file, func)) => (file.to_string(), func.to_string()),
        None => (entry.to_string(), "register".to_string()),
    }
}

/// Parse and validate a manifest; on failure returns a readable error (the loader records it
/// as the rejection reason).
pub fn parse(text: &str) -> Result<PluginManifest, String> {
    let manifest: PluginManifest =
        toml::from_str(text).map_err(|e| format!("failed to parse manifest: {e}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &PluginManifest) -> Result<(), String> {
    let id = &manifest.plugin.id;
    if id.is_empty() {
        return Err("plugin ID is empty".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!("plugin ID contains invalid characters: {id}"));
    }
    if manifest.plugin.api_version != HOST_API_VERSION {
        return Err(format!(
            "API version mismatch: plugin {}, host {}",
            manifest.plugin.api_version, HOST_API_VERSION
        ));
    }
    if !manifest.plugin.entry_point.ends_with(".rhai") {
        return Err(format!(
            "entry_point must be a .rhai script: {}",
            manifest.plugin.entry_point
        ));
    }
    for module in &manifest.plugin.modules {
        if module.is_empty() {
            return Err("modules contains an empty module ID".to_string());
        }
        if !module
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err(format!("module ID contains invalid characters: {module}"));
        }
    }
    if manifest.plugin.requires.as_deref().is_some_and(str::is_empty) {
        return Err("requires names no feature".to_string());
    }
    Ok(())
}
