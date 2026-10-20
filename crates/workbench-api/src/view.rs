//! 视图契约（design.md §9）：注册、生命周期与呈现。

use crate::ids::ViewTypeId;
use crate::services::AppServices;

/// 视图实例：面板与中央标签共用的内容单元。
///
/// 生命周期：工厂创建 → 每帧 `element` → 关闭时丢弃。
///
/// 视图内容是一棵**前端元素树**：前端在渲染期调用 [`ViewInstance::element`]，
/// 传入平台宿主让视图读取实时状态，视图返回类型擦除的元素句柄（gpui 前端下
/// 即 `gpui::AnyElement`）。视图不再自己画像素——绘制由前端元素系统负责，
/// 输入由前端的事件系统负责。
pub trait ViewInstance: 'static {
    /// 视图类型 ID（应与注册的 ViewTypeId 一致）。
    fn type_id(&self) -> ViewTypeId;
    /// 标题（中央标签 / 面板头显示）。
    fn title(&self) -> String;
    /// 视图内容的前端元素树。
    ///
    /// 返回 `None` 表示本帧不渲染内容。句柄的具体类型由前端与其视图扩展约定。
    fn element(&mut self, _host: &mut dyn AppServices) -> Option<Box<dyn std::any::Any>> {
        None
    }
}
