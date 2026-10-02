//! gpui 前端适配器：把平台工作区模型渲染为 gpui-ce 窗口。
//!
//! 本 crate 是 GUI 侧唯一接触 gpui 的地方。布局采用与 egui 端等价的
//! 固定四区 + 中央标签结构（两端的物理实现独立，design.md §6.6）。

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, quad, relative, rgb_to_hsla, size, App, Bounds,
    Context, Corners, Edges, Hsla, KeyDownEvent, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, SharedString, TextAlign, TextRun,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use gpui_component_assets::Assets;
use workbench_api as wb;
use workbench_core::AppRuntime;

type SharedCore = Arc<Mutex<AppRuntime>>;

// ---- 颜色转换 ----

fn col(c: wb::Color) -> Hsla {
    rgb_to_hsla(Rgba::new(c.r, c.g, c.b, c.a))
}

fn black() -> Hsla {
    rgb_to_hsla(Rgba::new(0.0, 0.0, 0.0, 1.0))
}

const COL_BG: wb::Color = wb::Color::rgb(0.16, 0.17, 0.20);
const COL_SPLITTER: wb::Color = wb::Color::rgb(0.10, 0.10, 0.12);
const COL_TOOLBAR: wb::Color = wb::Color::rgb(0.20, 0.21, 0.25);
const COL_TABBAR: wb::Color = wb::Color::rgb(0.14, 0.15, 0.18);

fn ui_font() -> gpui::Font {
    gpui::Font {
        // ".SystemUIFont" 由 gpui 按平台解析为系统字体（Windows 上为 Segoe UI）
        family: ".SystemUIFont".into(),
        ..Default::default()
    }
}

// ---- PaintBackend 的 gpui 实现 ----

struct GpuiPainter<'a> {
    window: &'a mut Window,
    cx: &'a mut App,
    origin: Point<Pixels>,
}

impl GpuiPainter<'_> {
    fn bounds_of(&self, r: wb::Rect) -> Bounds<Pixels> {
        Bounds {
            origin: Point::new(self.origin.x + px(r.pos.x), self.origin.y + px(r.pos.y)),
            size: size(px(r.size.w), px(r.size.h)),
        }
    }

    fn paint_quad_simple(&mut self, r: wb::Rect, color: wb::Color, radius: f32) {
        let q = quad(
            self.bounds_of(r),
            Corners::all(px(radius)),
            col(color),
            Edges::all(px(0.)),
            gpui::transparent_black(),
            gpui::BorderStyle::default(),
        );
        self.window.paint_quad(q);
    }
}

impl wb::PaintBackend for GpuiPainter<'_> {
    fn fill_rect(&mut self, rect: wb::Rect, color: wb::Color, corner_radius: f32) {
        self.paint_quad_simple(rect, color, corner_radius);
    }

    fn stroke_rect(&mut self, rect: wb::Rect, color: wb::Color, width: f32, corner_radius: f32) {
        let q = quad(
            self.bounds_of(rect),
            Corners::all(px(corner_radius)),
            gpui::transparent_black(),
            Edges::all(px(width)),
            col(color),
            gpui::BorderStyle::default(),
        );
        self.window.paint_quad(q);
    }

    fn line(&mut self, a: wb::Point, b: wb::Point, color: wb::Color, width: f32) {
        // 用细四边形近似线段（垂直/水平精确，斜线为包围盒近似）。
        let x0 = a.x.min(b.x);
        let y0 = a.y.min(b.y);
        let x1 = a.x.max(b.x);
        let y1 = a.y.max(b.y);
        let r = if x1 - x0 < 0.5 {
            wb::Rect::new(a.x - width * 0.5, y0, width, (y1 - y0).max(width))
        } else if y1 - y0 < 0.5 {
            wb::Rect::new(x0, a.y - width * 0.5, (x1 - x0).max(width), width)
        } else {
            wb::Rect::new(x0, y0, x1 - x0, y1 - y0)
        };
        self.paint_quad_simple(r, color, 0.0);
    }

    fn fill_circle(&mut self, center: wb::Point, radius: f32, color: wb::Color) {
        let r = wb::Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        );
        self.paint_quad_simple(r, color, radius);
    }

    fn text(&mut self, pos: wb::Point, font_size: f32, color: wb::Color, text: &str) {
        let text: SharedString = text.to_string().into();
        let run = TextRun {
            len: text.len(),
            font: ui_font(),
            color: col(color),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: None,
        };
        let line = self
            .window
            .text_system()
            .shape_line(text, px(font_size), &[run], None);
        let _ = line.paint(
            Point::new(self.origin.x + px(pos.x), self.origin.y + px(pos.y)),
            px(font_size * 1.3),
            TextAlign::Left,
            None,
            self.window,
            self.cx,
        );
    }

    fn text_size(&mut self, font_size: f32, text: &str) -> wb::Size {
        let text: SharedString = text.to_string().into();
        let run = TextRun {
            len: text.len(),
            font: ui_font(),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: None,
        };
        let line = self
            .window
            .text_system()
            .shape_line(text, px(font_size), &[run], None);
        wb::Size::new(line.width().as_f32(), font_size * 1.3)
    }
}

// ---- 输入事件中转 ----

#[derive(Clone, Copy, Debug)]
struct InputEv {
    pos: (f32, f32), // 窗口坐标
    kind: EvKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EvKind {
    Down,
    Up,
    Move,
}

#[derive(Clone, Copy, Debug)]
enum DragTarget {
    Left,
    Right,
    Bottom,
}

struct Drag {
    target: DragTarget,
    last: (f32, f32),
}

#[derive(Clone, Copy)]
struct DragSnapshot {
    target: DragTarget,
    last: (f32, f32),
}

/// 渲染期共享给绘制闭包的输入快照。
#[derive(Clone)]
struct FrameInput {
    events: Arc<Mutex<Vec<InputEv>>>,
    mouse_down: Arc<std::sync::atomic::AtomicBool>,
}

impl FrameInput {
    fn input_for(&self, bounds: &Bounds<Pixels>) -> wb::ViewInput {
        let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
        let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
        let rect = wb::Rect::new(ox, oy, w, h);
        let events = self
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let clicked = events
            .iter()
            .any(|e| e.kind == EvKind::Up && rect.contains(wb::Point::new(e.pos.0, e.pos.1)));
        let mouse_pos = events
            .iter()
            .next_back()
            .map(|e| wb::Point::new(e.pos.0, e.pos.1))
            .filter(|p| rect.contains(*p))
            .map(|p| wb::Point::new(p.x - ox, p.y - oy));
        wb::ViewInput {
            mouse_pos,
            mouse_down: self.mouse_down.load(std::sync::atomic::Ordering::Relaxed),
            clicked,
        }
    }
}

#[derive(Clone, Debug)]
enum SlotKey {
    Central(u64),
    Panel(String),
}

// ---- 根视图 ----

struct RootView {
    core: SharedCore,
    last_render: std::time::Instant,
    events: Arc<Mutex<Vec<InputEv>>>,
    mouse_down: Arc<std::sync::atomic::AtomicBool>,
    drag: Arc<Mutex<Option<Drag>>>,
}

impl RootView {
    fn new(core: SharedCore) -> Self {
        Self {
            core,
            last_render: std::time::Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
            mouse_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drag: Arc::new(Mutex::new(None)),
        }
    }
}

fn notify(weak: &gpui::WeakEntity<RootView>, cx: &mut App) {
    if let Some(entity) = weak.upgrade() {
        entity.update(cx, |_, cx| cx.notify());
    }
}

/// 面板/中央视图绘制到 canvas。
/// 实例存放在核心状态内，绘制需要 &mut 核心——先换出、绘制、再放回。
fn view_canvas(core: SharedCore, frame: FrameInput, key: SlotKey) -> impl IntoElement {
    canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, _state, window, cx| {
            let input = frame.input_for(&bounds);
            let area = wb::Rect::new(
                0.0,
                0.0,
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
            );
            let Ok(mut guard) = core.lock() else {
                return;
            };
            let mut instance: Option<Box<dyn wb::ViewInstance>> = match &key {
                SlotKey::Central(id) => guard
                    .host
                    .ws
                    .tabs
                    .iter_mut()
                    .find(|t| t.instance_id.0 == *id)
                    .and_then(|t| t.instance.take()),
                SlotKey::Panel(id) => guard
                    .host
                    .ws
                    .panels
                    .iter_mut()
                    .find(|p| &p.id.0 == id)
                    .and_then(|p| p.instance.take()),
            };
            {
                let Some(inst) = instance.as_mut() else {
                    return;
                };
                let mut painter = GpuiPainter {
                    window,
                    cx,
                    origin: bounds.origin,
                };
                let mut ctx = wb::ViewCtx::new(&mut *guard as &mut dyn wb::AppServices, input);
                inst.paint(&mut painter, area, &mut ctx);
            }
            if let Some(inst) = instance.take() {
                match &key {
                    SlotKey::Central(id) => {
                        if let Some(slot) = guard
                            .host
                            .ws
                            .tabs
                            .iter_mut()
                            .find(|t| t.instance_id.0 == *id)
                        {
                            slot.instance = Some(inst);
                        }
                    }
                    SlotKey::Panel(id) => {
                        if let Some(slot) = guard.host.ws.panels.iter_mut().find(|p| &p.id.0 == id)
                        {
                            slot.instance = Some(inst);
                        }
                    }
                }
            }
        },
    )
    .size_full()
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 帧驱动：任务泵 + 上下文快照
        {
            let mut core = self.core.lock().unwrap_or_else(|p| p.into_inner());
            core.frame_tick(0.016);
            let now = std::time::Instant::now();
            let dt_ms = (now - self.last_render).as_secs_f32() * 1000.0;
            self.last_render = now;
            core.record_frame(dt_ms);
        }
        // 本帧输入快照（事件为窗口坐标；绘制闭包在渲染结束后才执行）
        let events_snapshot: Arc<Mutex<Vec<InputEv>>> = {
            let mut cell = self.events.lock().unwrap_or_else(|p| p.into_inner());
            if cell.len() > 64 {
                { let len = cell.len(); cell.drain(0..len - 64); }
            }
            Arc::new(Mutex::new(std::mem::take(&mut *cell)))
        };
        let frame = FrameInput {
            events: events_snapshot,
            mouse_down: self.mouse_down.clone(),
        };

        let weak = cx.entity().downgrade();
        let core = self.core.clone();

        // ---- 核心状态读取（一次性） ----
        struct AreaUi {
            visible: bool,
            size: f32,
            panels: Vec<(String, String, bool)>, // (id, title, active)
        }
        let (left, right, bottom, central_tabs, central_active, ribbon_tabs, ribbon_groups, status) = {
            let core = self.core.lock().unwrap_or_else(|p| p.into_inner());
            let area_of = |a: wb::DockArea| {
                let slots: Vec<&wb::PanelSlot> = core
                    .host
                    .ws
                    .panels_in_area(a)
                    .into_iter()
                    .filter(|p| p.visible)
                    .collect();
                let active_id = core.host.ws.area_active.get(&a).cloned();
                let visible = !slots.is_empty();
                let size = slots
                    .iter()
                    .find(|p| Some(&p.id) == active_id.as_ref())
                    .or_else(|| slots.first())
                    .map(|p| p.size)
                    .unwrap_or(0.0);
                AreaUi {
                    visible,
                    size,
                    panels: slots
                        .iter()
                        .map(|p| {
                            let active = active_id.as_ref() == Some(&p.id);
                            (p.id.0.clone(), p.title.clone(), active)
                        })
                        .collect(),
                }
            };
            let left = area_of(wb::DockArea::Left);
            let right = area_of(wb::DockArea::Right);
            let bottom = area_of(wb::DockArea::Bottom);
            let central_active = core.host.ws.active_tab().map(|t| t.instance_id.0);
            let central_tabs: Vec<(u64, String, bool)> = core
                .host
                .ws
                .tabs
                .iter()
                .map(|t| (t.instance_id.0, t.title.clone(), Some(t.instance_id.0) == central_active))
                .collect();
            let (ribbon_tabs, ribbon_groups): (
                Vec<(String, String, bool)>,
                Vec<(String, Vec<(String, String, bool)>)>,
            ) = {
                let ws = &core.host.ws;
                let tabs = ws
                    .ribbon
                    .iter()
                    .map(|t| {
                        (
                            t.id.0.clone(),
                            t.title.clone(),
                            t.id.0 == ws.active_ribbon_tab,
                        )
                    })
                    .collect();
                let groups = ws
                    .active_ribbon()
                    .map(|tab| {
                        tab.groups
                            .iter()
                            .map(|g| {
                                let items = g
                                    .items
                                    .iter()
                                    .map(|it| {
                                        let title = core
                                            .command_title(&it.command.0)
                                            .unwrap_or_else(|| it.command.0.clone());
                                        let enabled = core.is_enabled(&it.command.0);
                                        (
                                            it.command.0.clone(),
                                            it.label.clone().unwrap_or_else(|| title.clone()),
                                            enabled,
                                        )
                                    })
                                    .collect();
                                (g.title.clone(), items)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (tabs, groups)
            };
            let snap = core.snapshot().clone();
            let (running, last_task) = core.host.tasks.summary();
            let status = StatusData {
                doc: snap
                    .active_document
                    .as_ref()
                    .map(|d| {
                        format!(
                            "文档：{}{} · 修订 r{}",
                            d.title,
                            if d.dirty { " •" } else { "" },
                            d.revision
                        )
                    })
                    .unwrap_or_else(|| "文档：（无）".to_string()),
                mode: snap.work_mode.clone(),
                running,
                task: last_task.cloned(),
                last_log: core
                    .host
                    .log
                    .last()
                    .map(|e| (e.level, format!("{} {}", e.time, e.message))),
            };
            (left, right, bottom, central_tabs, central_active, ribbon_tabs, ribbon_groups, status)
        };

        // ---- RibbonBar（design.md §7）：Tab 行 + 活动 Tab 的 Group 行 ----
        let mut tab_row = div().flex().flex_row().items_end().px_2().gap_1();
        for (id, title, active) in &ribbon_tabs {
            let core_c = core.clone();
            let weak_c = weak.clone();
            let id_c = id.clone();
            let mut tab = div()
                .id(SharedString::from(format!("rtab-{id}")))
                .px_2()
                .py_1()
                .text_size(px(12.5))
                .cursor_pointer()
                .child(title.clone());
            tab = if *active {
                tab.text_color(col(wb::Color::WHITE))
                    .border_b_2()
                    .border_color(col(wb::Color::ACCENT))
                    .bg(col(wb::Color::rgb(0.26, 0.27, 0.32)))
            } else {
                tab.text_color(col(wb::Color::LIGHT_GRAY))
                    .hover(|s| s.bg(col(wb::Color::rgb(0.24, 0.25, 0.30))))
            };
            tab = tab.on_click(move |_: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                core_c
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .host
                    .ws
                    .set_active_ribbon_tab(&id_c);
                notify(&weak_c, cx);
            });
            tab_row = tab_row.child(tab);
        }

        let mut groups_row = div().flex().flex_row().items_stretch().px_2().gap_2();
        for (gi, (gtitle, items)) in ribbon_groups.iter().enumerate() {
            if gi > 0 {
                groups_row = groups_row.child(
                    div()
                        .w(px(1.0))
                        .bg(col(wb::Color::rgb(0.30, 0.31, 0.36))),
                );
            }
            let mut btns = div().flex().flex_row().items_center().gap_1();
            for (cmd, label, enabled) in items {
                let mut btn = div()
                    .id(SharedString::from(format!("tb-{cmd}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_size(px(12.5))
                    .child(label.clone());
                btn = if *enabled {
                    btn.bg(col(wb::Color::rgb(0.27, 0.29, 0.34)))
                        .text_color(col(wb::Color::WHITE))
                        .cursor_pointer()
                        .hover(|s| s.bg(col(wb::Color::ACCENT)))
                        .on_click({
                            let core_c = core.clone();
                            let weak_c = weak.clone();
                            let cmd_c = cmd.clone();
                            move |_: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                                let r = core_c
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .execute_command(&cmd_c, &wb::CommandArgs::default());
                                if let wb::CommandResult::Failed { error } = r {
                                    core_c
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner())
                                        .host
                                        .log
                                        .error(error);
                                }
                                notify(&weak_c, cx);
                            }
                        })
                } else {
                    btn.bg(col(wb::Color::rgb(0.22, 0.23, 0.27)))
                        .text_color(col(wb::Color::GRAY))
                };
                btns = btns.child(btn);
            }
            groups_row = groups_row.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_0p5()
                    .px_1()
                    .py_1()
                    .child(btns)
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(col(wb::Color::GRAY))
                            .child(gtitle.clone()),
                    ),
            );
        }
        let ribbon = div()
            .flex()
            .flex_col()
            .bg(col(COL_TOOLBAR))
            .border_b_1()
            .border_color(col(wb::Color::rgb(0.10, 0.10, 0.12)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .child(tab_row),
            )
            .child(groups_row);

        // ---- 面板列（区域内的标签页 + 活动面板内容） ----
        let panel_column = |area: &AreaUi, key_prefix: &'static str| -> gpui::AnyElement {
            let mut header = div().flex().flex_row().items_center().gap_1();
            for (pid, title, active) in &area.panels {
                let mut tab = div()
                    .id(SharedString::from(format!("{key_prefix}-{pid}")))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(11.5))
                    .cursor_pointer()
                    .child(title.clone());
                tab = if *active {
                    tab.bg(col(wb::Color::ACCENT))
                        .text_color(col(wb::Color::WHITE))
                } else {
                    tab.text_color(col(wb::Color::LIGHT_GRAY))
                        .hover(|s| s.bg(col(wb::Color::rgb(0.26, 0.27, 0.32))))
                };
                let core_c = core.clone();
                let pid_c = pid.clone();
                let weak_c = weak.clone();
                tab = tab.on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                    let mut core = core_c.lock().unwrap_or_else(|p| p.into_inner());
                    let area_kind = core
                        .host
                        .ws
                        .panels
                        .iter()
                        .find(|p| p.id.0 == pid_c)
                        .map(|p| p.area);
                    if let Some(a) = area_kind {
                        core.host
                            .ws
                            .area_active
                            .insert(a, wb::PanelId::new(pid_c.clone()));
                    }
                    drop(core);
                    notify(&weak_c, cx);
                });
                header = header.child(tab);
            }
            let active_panel = area
                .panels
                .iter()
                .find(|(_, _, active)| *active)
                .or_else(|| area.panels.first());
            let content = match active_panel {
                Some((pid, _, _)) => view_canvas(
                    core.clone(),
                    frame.clone(),
                    SlotKey::Panel(pid.clone()),
                )
                .into_any_element(),
                None => div().into_any_element(),
            };
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(col(COL_BG))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_1()
                        .h(px(24.0))
                        .bg(col(COL_TABBAR))
                        .child(header),
                )
                .child(div().flex_1().min_h(px(0.0)).child(content))
                .into_any_element()
        };

        // ---- 中央标签区 ----
        let mut tabbar = div().flex().flex_row().items_center().px_1().gap_1();
        for (id, title, active) in &central_tabs {
            let mut tab = div()
                .id(SharedString::from(format!("tab-{id}")))
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_size(px(12.0))
                .cursor_pointer()
                .child(title.clone());
            tab = if *active {
                tab.bg(col(wb::Color::rgb(0.30, 0.32, 0.38)))
                    .text_color(col(wb::Color::WHITE))
            } else {
                tab.text_color(col(wb::Color::LIGHT_GRAY))
                    .hover(|s| s.bg(col(wb::Color::rgb(0.24, 0.25, 0.30))))
            };
            let core_c = core.clone();
            let id_c = *id;
            let weak_c = weak.clone();
            tab = tab.on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                core_c
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .host
                    .ws
                    .activate_tab_by_instance(wb::TabInstanceId(id_c));
                notify(&weak_c, cx);
            });
            tabbar = tabbar.child(tab);

            let close = div()
                .id(SharedString::from(format!("tab-close-{id}")))
                .px_1()
                .rounded_sm()
                .text_size(px(11.0))
                .text_color(col(wb::Color::GRAY))
                .cursor_pointer()
                .hover(|s| s.bg(col(wb::Color::rgb(0.4, 0.2, 0.2))))
                .child("×");
            let core_c = core.clone();
            let id_c = *id;
            let weak_c = weak.clone();
            let close = close.on_click(
                move |_ev: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                    core_c
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .host
                        .ws
                        .close_tab_by_instance(wb::TabInstanceId(id_c));
                    notify(&weak_c, cx);
                },
            );
            tabbar = tabbar.child(close);
        }
        let central_content = match central_active {
            Some(id) => view_canvas(core.clone(), frame.clone(), SlotKey::Central(id))
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(col(wb::Color::GRAY))
                .child("没有打开的视图")
                .into_any_element(),
        };
        let central_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .bg(col(COL_BG))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(26.0))
                    .px_1()
                    .bg(col(COL_TABBAR))
                    .child(tabbar),
            )
            .child(div().flex_1().min_h(px(0.0)).child(central_content));

        // ---- 分隔条（拖拽调整区域尺寸） ----
        let mk_splitter = |target: DragTarget, vertical: bool| -> gpui::AnyElement {
            let drag_cell = self.drag.clone();
            let base = if vertical {
                div()
                    .id(match target {
                        DragTarget::Left => "split-left",
                        DragTarget::Right => "split-right",
                        DragTarget::Bottom => "split-bottom-v",
                    })
                    .w(px(6.0))
                    .h_full()
                    .cursor_col_resize()
            } else {
                div()
                    .id("split-bottom")
                    .h(px(6.0))
                    .w_full()
                    .cursor_row_resize()
            };
            base.bg(col(COL_SPLITTER))
                .hover(|s| s.bg(col(wb::Color::ACCENT)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    {
                        let drag_cell = drag_cell.clone();
                        move |ev: &MouseDownEvent, _w: &mut Window, _cx: &mut App| {
                            *drag_cell.lock().unwrap_or_else(|p| p.into_inner()) = Some(Drag {
                                target,
                                last: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                            });
                        }
                    },
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    {
                        let drag_cell = drag_cell.clone();
                        move |_ev: &MouseUpEvent, _w: &mut Window, _cx: &mut App| {
                            *drag_cell.lock().unwrap_or_else(|p| p.into_inner()) = None;
                        }
                    },
                )
                .on_mouse_move({
                    let core_m = core.clone();
                    let drag_cell = drag_cell.clone();
                    move |ev: &MouseMoveEvent, _w: &mut Window, _cx: &mut App| {
                        let d_snapshot = {
                            let guard = drag_cell.lock().unwrap_or_else(|p| p.into_inner());
                            guard.as_ref().map(|d| DragSnapshot {
                                target: d.target,
                                last: d.last,
                            })
                        };
                        let Some(d) = d_snapshot else {
                            return;
                        };
                        let (x, y) = (ev.position.x.as_f32(), ev.position.y.as_f32());
                        let dx = x - d.last.0;
                        let dy = y - d.last.1;
                        if let Some(d) = drag_cell.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
                        {
                            d.last = (x, y);
                        }
                        let mut core = core_m.lock().unwrap_or_else(|p| p.into_inner());
                        let mut adjust = |area: wb::DockArea, delta: f32| {
                            let Some(active) = core.host.ws.area_active.get(&area).cloned() else {
                                return;
                            };
                            if let Some(slot) = core.host.ws.panels.iter_mut().find(|p| p.id == active)
                            {
                                slot.size = (slot.size + delta).max(120.0);
                            }
                        };
                        match d.target {
                            DragTarget::Left => adjust(wb::DockArea::Left, dx),
                            DragTarget::Right => adjust(wb::DockArea::Right, -dx),
                            DragTarget::Bottom => adjust(wb::DockArea::Bottom, -dy),
                        }
                    }
                })
                .into_any_element()
        };

        // ---- 组装 ----
        let mut main_row = div().flex().flex_row().flex_1().min_h(px(0.0));
        if left.visible {
            main_row = main_row
                .child(
                    div()
                        .w(px(left.size))
                        .min_w(px(120.0))
                        .child(panel_column(&left, "pl")),
                )
                .child(mk_splitter(DragTarget::Left, true));
        }
        main_row = main_row.child(central_area);
        if right.visible {
            main_row = main_row
                .child(mk_splitter(DragTarget::Right, true))
                .child(
                    div()
                        .w(px(right.size))
                        .min_w(px(120.0))
                        .child(panel_column(&right, "pr")),
                );
        }

        let mut root_col = div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Segoe UI")
            .bg(col(COL_BG))
            .text_color(col(wb::Color::LIGHT_GRAY))
.child(ribbon)
            .child(main_row);
        if bottom.visible {
            root_col = root_col
                .child(mk_splitter(DragTarget::Bottom, false))
                .child(
                    div()
                        .h(px(bottom.size))
                        .min_h(px(80.0))
                        .child(panel_column(&bottom, "pb")),
                );
        }

        root_col = root_col.child(render_status_bar(&status, core.clone(), weak.clone()));

        // 窗口级输入监听
        root_col = root_col
            .on_mouse_down(
                gpui::MouseButton::Left,
                {
                    let ev_cell = self.events.clone();
                    let down_flag = self.mouse_down.clone();
                    move |ev: &MouseDownEvent, _w: &mut Window, _cx: &mut App| {
                        down_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        ev_cell
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(InputEv {
                                pos: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                                kind: EvKind::Down,
                            });
                    }
                },
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                {
                    let ev_cell = self.events.clone();
                    let down_flag = self.mouse_down.clone();
                    move |ev: &MouseUpEvent, _w: &mut Window, _cx: &mut App| {
                        down_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                        ev_cell
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .push(InputEv {
                                pos: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                                kind: EvKind::Up,
                            });
                    }
                },
            )
            .on_mouse_move({
                let ev_cell = self.events.clone();
                move |ev: &MouseMoveEvent, _w: &mut Window, _cx: &mut App| {
                    ev_cell
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(InputEv {
                            pos: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                            kind: EvKind::Move,
                        });
                }
            })
            .on_key_down({
                let core_k = core.clone();
                let weak_k = weak.clone();
                move |ev: &KeyDownEvent, _w: &mut Window, cx: &mut App| {
                    let ks = &ev.keystroke;
                    let Some(code) = wb::KeyCode::from_name(&ks.key) else {
                        return;
                    };
                    let hk = wb::Hotkey {
                        ctrl: ks.modifiers.control,
                        alt: ks.modifiers.alt,
                        shift: ks.modifiers.shift,
                        key: code,
                    };
                    let matched: Vec<String> = core_k
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .hotkeys()
                        .into_iter()
                        .filter(|(registered, _)| *registered == hk)
                        .map(|(_, cmd)| cmd)
                        .collect();
                    for cmd in matched {
                        let _ = core_k
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .execute_command(&cmd, &wb::CommandArgs::default());
                    }
                    notify(&weak_k, cx);
                }
            });

        root_col
    }
}

#[derive(Clone)]
struct StatusData {
    doc: String,
    mode: String,
    running: usize,
    task: Option<wb::TaskInfo>,
    last_log: Option<(wb::LogLevel, String)>,
}

fn render_status_bar(
    data: &StatusData,
    core: SharedCore,
    weak: gpui::WeakEntity<RootView>,
) -> gpui::AnyElement {
    let mut right_side = div().flex().flex_row().items_center().gap_2();
    if let Some((level, msg)) = &data.last_log {
        let color = match level {
            wb::LogLevel::Error => wb::Color::RED,
            wb::LogLevel::Warn => wb::Color::YELLOW,
            _ => wb::Color::GRAY,
        };
        right_side = right_side.child(
            div()
                .max_w(px(420.0))
                .text_size(px(11.0))
                .text_color(col(color))
                .child(msg.clone()),
        );
    }
    if data.running > 0 {
        if let Some(task) = &data.task {
            right_side = right_side.child(
                div()
                    .text_size(px(11.5))
                    .child(format!("#{} {}", task.id.0, task.title)),
            );
            let frac = task.progress.unwrap_or(0.0);
            right_side = right_side.child(
                div()
                    .w(px(120.0))
                    .h(px(8.0))
                    .rounded_sm()
                    .overflow_hidden()
                    .bg(col(wb::Color::rgb(0.12, 0.12, 0.15)))
                    .child(
                        div()
                            .h_full()
                            .w(relative(frac.clamp(0.0, 1.0)))
                            .bg(col(wb::Color::ACCENT)),
                    ),
            );
            if let Some(stage) = &task.stage {
                right_side = right_side.child(div().text_size(px(11.0)).child(stage.clone()));
            }
            if task.cancellable {
                let core_c = core.clone();
                let weak_c = weak.clone();
                let tid = task.id;
                right_side = right_side.child(
                    div()
                        .id("status-cancel")
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .text_size(px(11.0))
                        .cursor_pointer()
                        .bg(col(wb::Color::rgb(0.35, 0.22, 0.22)))
                        .hover(|s| s.bg(col(wb::Color::RED)))
                        .child("取消")
                        .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                            core_c
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .host
                                .tasks
                                .request_cancel(tid);
                            notify(&weak_c, cx);
                        }),
                );
            }
        }
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(26.0))
        .px_2()
        .gap_2()
        .bg(col(COL_TOOLBAR))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(col(wb::Color::LIGHT_GRAY))
                        .child(data.doc.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(col(wb::Color::GRAY))
                        .child(format!("模式：{}", data.mode)),
                ),
        )
        .child(right_side)
        .into_any_element()
}

/// gpui 入口：构建产品并运行主循环。
pub fn run(builder: workbench_core::WorkbenchAppBuilder, window_title: &str) -> Result<(), String> {
    let smoke = builder.smoke_seconds();
    let title: SharedString = window_title.to_string().into();
    let core: SharedCore = Arc::new(Mutex::new(builder.build()?));

    let app = gpui_platform::application().with_assets(Assets);
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone()),
                appears_transparent: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        let core_window = core.clone();
        let handle = cx
            .open_window(options, |_window, cx| {
                cx.new(|_cx| RootView::new(core_window))
            })
            .expect("打开窗口失败");
        let _ = handle;
        cx.activate(true);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // 有任务运行时以低频率刷新（进度动画）
        let weak_root = cx
            .active_window()
            .and_then(|w| w.downcast::<RootView>())
            .and_then(|h| h.entity(cx).ok())
            .map(|e| e.downgrade());
        let core_loop = core.clone();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            let running = core_loop
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .tasks
                .summary()
                .0;
            if running > 0 {
                if let Some(weak) = weak_root.clone() {
                    let _ = cx.update(|cx| notify(&weak, cx));
                }
            }
        })
        .detach();

        // 冒烟模式：N 秒后自动退出
        if let Some(secs) = smoke {
            if secs > 0.0 {
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs_f32(secs))
                        .await;
                    let _ = cx.update(|cx| cx.quit());
                })
                .detach();
            }
        }
    });
    Ok(())
}
