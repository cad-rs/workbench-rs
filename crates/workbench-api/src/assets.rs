//! GUI 无关的资源加载契约（SVG 图标等静态资源）。
//!
//! Workbench/插件在 UI 元素上声明资源路径（如 `"icons/save.svg"`），
//! 产品通过 [`crate::WorkbenchAppBuilder::with_assets`]
//! （workbench-core）提供[`AssetSource`]；前端负责按路径取字节并渲染。
//! 路径只是稳定键，不保证对应真实文件——最常见的形态是 `include_bytes!`
//! 内嵌到二进制里的静态表。

use std::borrow::Cow;
use std::sync::Arc;

/// 静态资源源：按路径加载资源字节。
///
/// 返回 `None` 表示该源没有此资源（调用方会继续向下查找或回退到无图标的
/// 展示）；加载失败也应返回 `None` 而不是让 UI 中断。
pub trait AssetSource: Send + Sync {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>>;

    /// 枚举本源持有的全部资源路径（GUI 前端向 gpui `AssetRegistry`
    /// 登记路径用；registry 只解析登记过的路径）。默认空——纯按需查询的
    /// 源无法枚举自身，应改用可枚举形态（如 [`StaticAssets`]）。
    fn paths(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 链式资源源：按顺序查找，第一个命中者生效（后面的源提供回退）。
///
/// 典型用法：`ChainedAssets::new(vec![产品源, 平台标准源])`——产品可以用
/// 同名路径覆盖平台标准图标，也可以只补充自己的图标。
pub struct ChainedAssets {
    sources: Vec<Arc<dyn AssetSource>>,
}

impl ChainedAssets {
    pub fn new(sources: Vec<Arc<dyn AssetSource>>) -> Self {
        Self { sources }
    }
}

impl AssetSource for ChainedAssets {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.sources.iter().find_map(|s| s.load(path))
    }

    fn paths(&self) -> Vec<String> {
        self.sources.iter().flat_map(|s| s.paths()).collect()
    }
}

/// 便捷构造：用静态 `(路径, 字节)` 表创建资源源（配合 `include_bytes!`）。
#[derive(Clone, Copy)]
pub struct StaticAssets {
    entries: &'static [(&'static str, &'static [u8])],
}

impl StaticAssets {
    pub const fn new(entries: &'static [(&'static str, &'static [u8])]) -> Self {
        Self { entries }
    }
}

impl AssetSource for StaticAssets {
    fn load(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        self.entries
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes))
    }

    fn paths(&self) -> Vec<String> {
        self.entries.iter().map(|(p, _)| (*p).to_string()).collect()
    }
}
