//! 平台标准资源：平台内置命令的 SVG 图标（`include_bytes!` 内嵌）。
//!
//! 图标路径以 `wb-std/` 为前缀，由 [`standard_assets`] 提供。产品的资源源在
//! [`workbench_api::ChainedAssets`] 链中排在标准源之前，可用同名路径覆盖。
//!
//! 语义：图标按 alpha 蒙版渲染并由前端着色（单色图标），SVG 自身颜色不生效，
//! 笔画/填充的形状决定蒙版。

use workbench_api::StaticAssets;

/// 平台标准图标路径。
pub mod icons {
    pub const NEW: &str = "wb-std/new.svg";
    pub const OPEN: &str = "wb-std/open.svg";
    pub const SAVE: &str = "wb-std/save.svg";
    pub const UNDO: &str = "wb-std/undo.svg";
    pub const REDO: &str = "wb-std/redo.svg";
}

static STANDARD_ENTRIES: &[(&str, &[u8])] = &[
    (icons::NEW, include_bytes!("../assets/std/new.svg")),
    (icons::OPEN, include_bytes!("../assets/std/open.svg")),
    (icons::SAVE, include_bytes!("../assets/std/save.svg")),
    (icons::UNDO, include_bytes!("../assets/std/undo.svg")),
    (icons::REDO, include_bytes!("../assets/std/redo.svg")),
];

/// 平台标准图标资源源（全局静态，零分配查找）。
pub fn standard_assets() -> &'static StaticAssets {
    static ASSETS: StaticAssets = StaticAssets::new(STANDARD_ENTRIES);
    &ASSETS
}
