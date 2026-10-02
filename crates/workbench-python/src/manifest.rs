//! 插件清单（design.md §11.2）：解析、校验与 API 版本检查。

use std::path::Path;

use serde::Deserialize;

/// 宿主提供的 Python Host API 版本。不兼容的插件被拒绝加载（验收 #20）。
pub const HOST_API_VERSION: &str = "1";

/// 插件清单。
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginInfo,
    #[serde(default)]
    pub compatibility: Option<Compatibility>,
    #[serde(default)]
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginInfo {
    /// 稳定且唯一的插件 ID。
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    /// 形如 `plugin.py:register`。
    pub entry_point: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Compatibility {
    #[serde(default)]
    pub products: Option<Vec<String>>,
    #[serde(default)]
    pub platform: Option<String>,
}

/// 权限声明用于能力管理、用户提示与审计，不构成进程内 Python 的安全沙箱（§11.2）。
#[derive(Debug, Clone, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub document_read: Option<bool>,
    #[serde(default)]
    pub document_write: Option<bool>,
    #[serde(default)]
    pub filesystem_read: Option<bool>,
    #[serde(default)]
    pub network: Option<bool>,
}

/// 解析清单文件。
pub fn parse(path: &Path) -> Result<PluginManifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {}: {e}", path.display()))?;
    let manifest: PluginManifest =
        toml::from_str(&text).map_err(|e| format!("清单解析失败: {e}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(m: &PluginManifest) -> Result<(), String> {
    if m.plugin.id.trim().is_empty() {
        return Err("插件 id 不能为空".to_string());
    }
    if !m
        .plugin
        .id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return Err(format!("插件 id 含非法字符: `{}`", m.plugin.id));
    }
    if m.plugin.api_version != HOST_API_VERSION {
        return Err(format!(
            "API 版本不兼容：插件需要 api_version={}，宿主提供 {}",
            m.plugin.api_version, HOST_API_VERSION
        ));
    }
    let (file, func) = split_entry_point(&m.plugin.entry_point)?;
    if file.is_empty() || !file.ends_with(".py") {
        return Err(format!("entry_point 文件名非法: `{file}`"));
    }
    if func.is_empty() {
        return Err(format!("entry_point 缺少入口函数: `{}`", m.plugin.entry_point));
    }
    Ok(())
}

/// 拆分 `file.py:function`。
pub fn split_entry_point(entry: &str) -> Result<(String, String), String> {
    let (file, func) = entry
        .rsplit_once(':')
        .ok_or_else(|| format!("entry_point 缺少 `:` 分隔: `{entry}`"))?;
    Ok((file.trim().to_string(), func.trim().to_string()))
}
