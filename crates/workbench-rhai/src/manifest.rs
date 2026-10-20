//! 插件清单（`plugin.toml`）解析与校验。
//!
//! 结构与语言无关：`entry_point` 指向 Rhai 脚本文件（如 `plugin.rhai`），
//! 可选地用 `:` 指定入口函数名（缺省为 `register`）。

use serde::Deserialize;

/// Host API 版本（与本 crate 发布周期一致）。
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

/// 拆分入口点：`plugin.rhai` 或 `plugin.rhai:register`。
pub fn split_entry_point(entry: &str) -> (String, String) {
    match entry.split_once(':') {
        Some((file, func)) => (file.to_string(), func.to_string()),
        None => (entry.to_string(), "register".to_string()),
    }
}

/// 解析并校验清单；失败返回可读错误（装载方记录为拒绝原因）。
pub fn parse(text: &str) -> Result<PluginManifest, String> {
    let manifest: PluginManifest =
        toml::from_str(text).map_err(|e| format!("清单解析失败: {e}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &PluginManifest) -> Result<(), String> {
    let id = &manifest.plugin.id;
    if id.is_empty() {
        return Err("插件 ID 为空".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!("插件 ID 含非法字符: {id}"));
    }
    if manifest.plugin.api_version != HOST_API_VERSION {
        return Err(format!(
            "API 版本不匹配: 插件 {}，宿主 {}",
            manifest.plugin.api_version, HOST_API_VERSION
        ));
    }
    if !manifest.plugin.entry_point.ends_with(".rhai") {
        return Err(format!(
            "entry_point 必须是 .rhai 脚本: {}",
            manifest.plugin.entry_point
        ));
    }
    for module in &manifest.plugin.modules {
        if module.is_empty() {
            return Err("modules 含空模块 ID".to_string());
        }
        if !module
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err(format!("模块 ID 含非法字符: {module}"));
        }
    }
    Ok(())
}
