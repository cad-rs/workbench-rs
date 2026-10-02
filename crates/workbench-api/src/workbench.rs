//! Workbench 模块契约（design.md §8）。
//!
//! 模块在产品装配阶段静态链接（Cargo crate），通过 `init` 注册贡献；
//! 模块自身不持有运行期状态——领域状态放扩展服务或文档内容。

use crate::ids::WorkbenchId;
use crate::registry::Registry;

pub trait Workbench: 'static {
    /// 模块稳定 ID。
    fn id(&self) -> WorkbenchId;
    /// 注册贡献。允许 panic——核心装配时会捕获并隔离（验收 #19）。
    fn init(&self, registry: &mut Registry);
}

/// 便捷：为实现了 Workbench 的类型生成 Box 装配项。
pub fn boxed<W: Workbench>(wb: W) -> Box<dyn Workbench> {
    Box::new(wb)
}
