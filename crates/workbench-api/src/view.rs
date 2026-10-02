//! 视图契约（design.md §9）：注册、生命周期与 GUI 无关的呈现。

use crate::command::CtxSnapshot;
use crate::geo::Rect;
use crate::ids::ViewTypeId;
use crate::paint::PaintBackend;
use crate::services::AppServices;

/// 前端传给视图的本帧输入（坐标为视图本地区域内的点）。
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewInput {
    /// 本帧鼠标位置；None 表示指针不在视图内。
    pub mouse_pos: Option<crate::geo::Point>,
    /// 鼠标按下状态。
    pub mouse_down: bool,
    /// 本帧内发生了点击（按下并抬起）。
    pub clicked: bool,
}

/// 视图实例：面板与中央标签共用的内容单元。
///
/// 生命周期：工厂创建 → 每帧 `paint`（可先经 `update`）→ 关闭时丢弃。
/// 视图通过 [`ViewCtx`] 访问平台服务，通过 [`PaintBackend`] 绘制，
/// 不接触任何 GUI 框架类型（design.md 验收 #21）。
pub trait ViewInstance: 'static {
    /// 视图类型 ID（应与注册的 ViewTypeId 一致）。
    fn type_id(&self) -> ViewTypeId;
    /// 标题（中央标签 / 面板头显示）。
    fn title(&self) -> String;
    /// 每帧逻辑更新（动画、轮询等）；先于 paint 调用。
    fn update(&mut self, _ctx: &mut ViewCtx, _dt_secs: f32) {}
    /// 绘制视图内容到 rect 区域。
    fn paint(&mut self, painter: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx);
}

/// 视图运行上下文。
pub struct ViewCtx<'a> {
    app: &'a mut dyn AppServices,
    /// 本帧输入。
    pub input: ViewInput,
}

impl<'a> ViewCtx<'a> {
    pub fn new(app: &'a mut dyn AppServices, input: ViewInput) -> Self {
        Self { app, input }
    }
    pub fn host(&mut self) -> &mut crate::services::ServiceHost {
        self.app.host()
    }
    pub fn snapshot(&self) -> CtxSnapshot {
        self.app.snapshot()
    }
    /// 触发一条命令（例如画布点击添加图形）。
    pub fn run(&mut self, command: &str) -> crate::command::CommandResult {
        self.app
            .dispatch(command, &crate::command::CommandArgs::default())
    }
    pub fn run_with(
        &mut self,
        command: &str,
        args: crate::command::CommandArgs,
    ) -> crate::command::CommandResult {
        self.app.dispatch(command, &args)
    }
}
