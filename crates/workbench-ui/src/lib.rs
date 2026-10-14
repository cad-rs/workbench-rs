//! gpui frontend for workbench-rs: renders the platform workspace model into
//! a gpui-ce window.
//!
//! This crate is the only place on the GUI side that touches gpui. Panels and
//! central views draw through `PaintBackend`; panels registered with a
//! `script` path are instead mounted as gpui-rhai script views (UiNode trees
//! authored in Rhai).

use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, quad, relative, rgb_to_hsla, size, svg, App, Bounds,
    Context, Corners, Edges, Hsla, KeyDownEvent, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, Rgba, ScrollDelta, ScrollWheelEvent, SharedString, TextAlign, TextRun,
    TitlebarOptions, TransformationMatrix, Window, WindowBounds, WindowOptions,
};

use workbench_api as wb;
use workbench_core::AppRuntime;

pub mod gpu_view;

pub(crate) mod dock_panel;

/// 平台运行时句柄：全部使用都在 UI 主线程（gpui 前台执行器），故用 `Rc`。
type SharedCore = std::rc::Rc<Mutex<AppRuntime>>;

/// 拉丁基础字体（IBM Plex Sans，OFL 许可，随 license.txt 一并提交）。
/// fork 的 gpui_web 是空字体库，Web 端必须在首帧前显式注入。
#[cfg(target_arch = "wasm32")]
const LATIN_FONT: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");

/// 组合资源源：产品资源优先，gpui-component 内置图标资源兜底。
/// （桌面为 RustEmbed 静态表；Web 端按需从同源/CDN 拉取 icons/*.svg）
struct WbAssets {
    app: Arc<dyn wb::AssetSource>,
    component: gpui_component_assets::Assets,
}

impl gpui::AssetSource for WbAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = self.app.load(path) {
            return Ok(Some(bytes));
        }
        self.component.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        self.component.list(path)
    }
}

// ---- 颜色转换 ----

fn col(c: wb::Color) -> Hsla {
    rgb_to_hsla(Rgba::new(c.r, c.g, c.b, c.a))
}

fn black() -> Hsla {
    rgb_to_hsla(Rgba::new(0.0, 0.0, 0.0, 1.0))
}

const COL_BG: wb::Color = wb::Color::rgb(0.16, 0.17, 0.20);
const COL_TOOLBAR: wb::Color = wb::Color::rgb(0.20, 0.21, 0.25);

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

    /// 绘制 SVG 图标（等比缩放，绘制区域取 rect 与图标宽高比的较小者）。
    /// 资源缺失时静默不绘制（与 gpui svg 元素行为一致）。
    fn draw_icon(&mut self, rect: wb::Rect, path: &str, color: wb::Color) {
        let _ = self.window.paint_svg(
            self.bounds_of(rect),
            path.into(),
            None,
            TransformationMatrix::unit(),
            col(color),
            self.cx,
        );
    }
}

// ---- 输入事件中转 ----

#[derive(Clone, Copy, Debug)]
struct InputEv {
    pos: (f32, f32), // 窗口坐标
    /// 事件时的 Shift 修饰键
    shift: bool,
    kind: EvKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum EvKind {
    Down,
    Up,
    Move,
    /// 滚轮（像素增量，向下为正）
    Scroll(f32),
}

/// 追加一条窗口级输入事件（四个输入处理器共享）。
fn push_input(events: &Arc<Mutex<Vec<InputEv>>>, pos: (f32, f32), shift: bool, kind: EvKind) {
    events
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(InputEv { pos, shift, kind });
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
        let shift = events.iter().next_back().map(|e| e.shift).unwrap_or(false);
        let scroll_total: f32 = events
            .iter()
            .filter(|e| {
                matches!(e.kind, EvKind::Scroll(_))
                    && rect.contains(wb::Point::new(e.pos.0, e.pos.1))
            })
            .map(|e| match e.kind {
                EvKind::Scroll(d) => d,
                _ => 0.0,
            })
            .sum();
        wb::ViewInput {
            mouse_pos,
            mouse_down: self.mouse_down.load(std::sync::atomic::Ordering::Relaxed),
            clicked,
            shift,
            scroll: (scroll_total != 0.0).then_some(scroll_total),
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
    last_render: web_time::Instant,
    events: Arc<Mutex<Vec<InputEv>>>,
    mouse_down: Arc<std::sync::atomic::AtomicBool>,
    /// The dock area and its mirror of the model (built on the first frame:
    /// `RootView::new` has no window to build it with).
    dock: Option<dock_panel::DockShell>,
    /// The persisted dock layout to re-establish, applied on the first frame
    /// after the model-driven sync has built this session's panels.
    pending_dock: Option<gpui_component::dock::DockAreaState>,
    /// Per-frame input for the paint closures, shared with the dock panels.
    frame: dock_panel::SharedFrame,
    /// gpui-rhai script view domain (per window; created at the first script
    /// panel mount, shared with the panels that draw them).
    scripts: dock_panel::SharedScripts,
    /// What the dock is showing, as last mirrored into the model.
    mirror: dock_panel::SharedMirror,
}

impl RootView {
    fn new(core: SharedCore, pending_dock: Option<gpui_component::dock::DockAreaState>) -> Self {
        Self {
            core,
            last_render: web_time::Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
            mouse_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dock: None,
            pending_dock,
            frame: std::rc::Rc::new(std::cell::RefCell::new(FrameInput {
                events: Arc::new(Mutex::new(Vec::new())),
                mouse_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })),
            scripts: std::rc::Rc::new(std::cell::RefCell::new(
                dock_panel::ScriptRuntime::default(),
            )),
            mirror: std::rc::Rc::new(std::cell::RefCell::new(
                dock_panel::DockMirror::default(),
            )),
        }
    }
}

/// 请求重绘：根视图 + 全部 dock 面板。
///
/// 面板现在是独立实体，而 dock 用 gpui 的**缓存视图**通路绘制面板内容
/// （面板实体被 notify 时其子树才重新渲染）。旧布局把面板画在根视图的子树
/// 里，notify 根视图就够；这层镜像保持"任何输入都驱动每个视口重绘"的语义。
fn notify(weak: &gpui::WeakEntity<RootView>, cx: &mut App) {
    let Some(entity) = weak.upgrade() else {
        return;
    };
    // Clone the panel handles out first: `cx` is borrowed by `read`.
    let panels = entity
        .read(cx)
        .dock
        .as_ref()
        .map(|dock| dock.panel_weaks())
        .unwrap_or_default();
    entity.update(cx, |_, cx| cx.notify());
    for panel in panels {
        if let Some(panel) = panel.upgrade() {
            panel.update(cx, |_, cx| cx.notify());
        }
    }
}

// ---- 面板/中央视图绘制 ----

/// GPU 视口未就绪 / 失败时的占位背景（深色，与视口底色一致）。
pub(crate) fn paint_placeholder(window: &mut Window, bounds: Bounds<Pixels>) {
    let q = quad(
        bounds,
        Corners::all(px(0.)),
        col(wb::Color::rgb(0.10, 0.11, 0.13)),
        Edges::all(px(0.)),
        gpui::transparent_black(),
        gpui::BorderStyle::default(),
    );
    window.paint_quad(q);
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
                // 原生视图优先（GPU 视口等）：命中则前端直接呈现，不走 2D 通道
                if !gpu_view::paint_native_view(inst.as_mut(), window, bounds, &input) {
                    let mut painter = GpuiPainter {
                        window,
                        cx,
                        origin: bounds.origin,
                    };
                    let mut ctx = wb::ViewCtx::new(&mut *guard as &mut dyn wb::AppServices, input);
                    inst.paint(&mut painter, area, &mut ctx);
                }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 帧驱动：任务泵 + 上下文快照
        {
            let mut core = self.core.lock().unwrap_or_else(|p| p.into_inner());
            core.frame_tick(0.016);
            let now = web_time::Instant::now();
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
        // Publish the frame's input to the dock panels: they are separate
        // entities now, so this shared cell is how a panel's paint closure
        // sees the events the root view just collected.
        *self.frame.borrow_mut() = FrameInput {
            events: events_snapshot,
            mouse_down: self.mouse_down.clone(),
        };

        let weak = cx.entity().downgrade();
        let core = self.core.clone();

        // ---- 核心状态读取（一次性） ----
        struct RibbonItemUi {
            cmd: String,
            label: String,
            enabled: bool,
            icon: Option<String>,
        }
        struct RibbonGroupUi {
            title: String,
            items: Vec<RibbonItemUi>,
        }
        struct RibbonTabUi {
            id: String,
            title: String,
            active: bool,
        }
        let (shell_snap, ribbon_tabs, ribbon_groups, status) = {
            let core = self.core.lock().unwrap_or_else(|p| p.into_inner());
            let area_of = |a: wb::DockArea| -> dock_panel::AreaSnapshot {
                let slots: Vec<&wb::PanelSlot> = core
                    .host
                    .ws
                    .panels_in_area(a)
                    .into_iter()
                    .filter(|p| p.visible)
                    .collect();
                let active_id = core.host.ws.area_active.get(&a).cloned();
                dock_panel::AreaSnapshot {
                    // The dock's size seeds from the panel it displays; the
                    // pre-dock front end sized the column the same way.
                    size: slots
                        .iter()
                        .find(|p| Some(&p.id) == active_id.as_ref())
                        .or_else(|| slots.first())
                        .map(|p| p.size)
                        .unwrap_or(0.0),
                    active: active_id.map(|id| id.0),
                    panels: slots
                        .iter()
                        .map(|p| dock_panel::PanelSnapshot {
                            id: p.id.0.clone(),
                            script: p.script.clone(),
                        })
                        .collect(),
                }
            };
            let shell_snap = dock_panel::ShellSnapshot {
                left: area_of(wb::DockArea::Left),
                right: area_of(wb::DockArea::Right),
                bottom: area_of(wb::DockArea::Bottom),
                tabs: core
                    .host
                    .ws
                    .tabs
                    .iter()
                    .map(|t| t.instance_id.0)
                    .collect(),
                active_tab: core.host.ws.active_tab().map(|t| t.instance_id.0),
            };
            let (ribbon_tabs, ribbon_groups): (Vec<RibbonTabUi>, Vec<RibbonGroupUi>) = {
                let ws = &core.host.ws;
                let tabs = ws
                    .ribbon
                    .iter()
                    .map(|t| RibbonTabUi {
                        id: t.id.0.clone(),
                        title: t.title.clone(),
                        active: t.id.0 == ws.active_ribbon_tab,
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
                                        RibbonItemUi {
                                            cmd: it.command.0.clone(),
                                            label: it.label.clone().unwrap_or(title),
                                            enabled: core.is_enabled(&it.command.0),
                                            icon: core.command_icon(&it.command.0),
                                        }
                                    })
                                    .collect();
                                RibbonGroupUi {
                                    title: g.title.clone(),
                                    items,
                                }
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
            (shell_snap, ribbon_tabs, ribbon_groups, status)
        };

        // ---- Dock 同步（模型 → dock；结构变化才重建） ----
        if self.dock.is_none() {
            self.dock = Some(dock_panel::DockShell::new(
                self.core.clone(),
                self.frame.clone(),
                self.scripts.clone(),
                self.mirror.clone(),
                window,
                cx,
            ));
        }
        {
            let dock = self.dock.as_mut().expect("built above");
            dock.sync(&shell_snap, window, cx);
            // The persisted physical layout goes on top of the model-driven
            // one, once: the panels it names exist by now, and the sync above
            // recorded the model's panel sets so it will not rebuild the
            // restored docks.
            if let Some(saved) = self.pending_dock.take() {
                dock.restore(saved, &shell_snap, window, cx);
            }
        }
        let dock_element = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(self.dock.as_ref().expect("built above").area());

        // ---- RibbonBar（design.md §7）：Tab 行 + 活动 Tab 的 Group 行 ----
        let mut tab_row = div().flex().flex_row().items_end().px_2().gap_1();
        for tab_def in &ribbon_tabs {
            let core_c = core.clone();
            let weak_c = weak.clone();
            let id_c = tab_def.id.clone();
            let mut tab = div()
                .id(SharedString::from(format!("rtab-{}", tab_def.id)))
                .px_2()
                .py_1()
                .text_size(px(12.5))
                .cursor_pointer()
                .child(tab_def.title.clone());
            tab = if tab_def.active {
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
        for (gi, group) in ribbon_groups.iter().enumerate() {
            if gi > 0 {
                groups_row = groups_row.child(
                    div()
                        .w(px(1.0))
                        .bg(col(wb::Color::rgb(0.30, 0.31, 0.36))),
                );
            }
            let mut btns = div().flex().flex_row().items_center().gap_1();
            for item in &group.items {
                let mut btn = div()
                    .id(SharedString::from(format!("tb-{}", item.cmd)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_size(px(12.5));
                if let Some(icon) = &item.icon {
                    // 左侧 15px 单色 SVG（禁用态灰色，与文本一致）
                    btn = btn.child(
                        svg()
                            .path(icon.clone())
                            .size(px(15.0))
                            .text_color(if item.enabled {
                                col(wb::Color::WHITE)
                            } else {
                                col(wb::Color::GRAY)
                            }),
                    );
                }
                btn = btn.child(item.label.clone());
                btn = if item.enabled {
                    btn.bg(col(wb::Color::rgb(0.27, 0.29, 0.34)))
                        .text_color(col(wb::Color::WHITE))
                        .cursor_pointer()
                        .hover(|s| s.bg(col(wb::Color::ACCENT)))
                        .on_click({
                            let core_c = core.clone();
                            let weak_c = weak.clone();
                            let cmd_c = item.cmd.clone();
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
                            .child(group.title.clone()),
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

        // ---- 组装 ----
        let mut root_col = div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Segoe UI")
            .bg(col(COL_BG))
            .text_color(col(wb::Color::LIGHT_GRAY))
            .child(ribbon)
            .child(dock_element);

        root_col = root_col.child(render_status_bar(&status, core.clone(), weak.clone()));

        // 窗口级输入监听（分隔条/标签拖拽已是 dock 自身的交互，这里只驱动
        // 视口输入管线与重绘）。
        // 每次输入都请求重绘：gpui 按需渲染，而视口拖拽/缩放的状态变化发生
        // 在绘制期（视图实例内），事件侧必须显式 notify 才能驱动下一帧，
        // 否则拖拽只在其他状态变化时零星刷新（滞后的手感）。
        root_col = root_col
            .on_mouse_down(
                gpui::MouseButton::Left,
                {
                    let ev_cell = self.events.clone();
                    let down_flag = self.mouse_down.clone();
                    let weak_d = weak.clone();
                    move |ev: &MouseDownEvent, _w: &mut Window, cx: &mut App| {
                        down_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        push_input(
                            &ev_cell,
                            (ev.position.x.as_f32(), ev.position.y.as_f32()),
                            ev.modifiers.shift,
                            EvKind::Down,
                        );
                        notify(&weak_d, cx);
                    }
                },
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                {
                    let ev_cell = self.events.clone();
                    let down_flag = self.mouse_down.clone();
                    let weak_u = weak.clone();
                    move |ev: &MouseUpEvent, _w: &mut Window, cx: &mut App| {
                        down_flag.store(false, std::sync::atomic::Ordering::Relaxed);
                        push_input(
                            &ev_cell,
                            (ev.position.x.as_f32(), ev.position.y.as_f32()),
                            ev.modifiers.shift,
                            EvKind::Up,
                        );
                        notify(&weak_u, cx);
                    }
                },
            )
            .on_mouse_move({
                let ev_cell = self.events.clone();
                let weak_m = weak.clone();
                move |ev: &MouseMoveEvent, _w: &mut Window, cx: &mut App| {
                    push_input(
                        &ev_cell,
                        (ev.position.x.as_f32(), ev.position.y.as_f32()),
                        ev.modifiers.shift,
                        EvKind::Move,
                    );
                    notify(&weak_m, cx);
                }
            })
            .on_scroll_wheel({
                let ev_cell = self.events.clone();
                let weak_s = weak.clone();
                move |ev: &ScrollWheelEvent, _w: &mut Window, cx: &mut App| {
                    let dy = match ev.delta {
                        ScrollDelta::Pixels(p) => p.y.as_f32(),
                        ScrollDelta::Lines(l) => l.y * 40.0,
                    };
                    push_input(
                        &ev_cell,
                        (ev.position.x.as_f32(), ev.position.y.as_f32()),
                        ev.modifiers.shift,
                        EvKind::Scroll(dy),
                    );
                    notify(&weak_s, cx);
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
///
/// 桌面：`Application::run` 阻塞直至退出。Web（wasm32）：`gpui_platform`
/// 自动切换到 `gpui_web` 的 WebPlatform（WebGPU 渲染），浏览器事件循环驱动；
/// 先注入字体（fork 的 gpui_web 是空字体库：内嵌拉丁 + 异步取同源 CJK，
/// 均在首帧前，避免排版缓存命中旧字形），并用 `run_embedded` 返回的句柄
/// 保持应用存活（页面生命周期 = 应用生命周期）。
pub fn run(builder: workbench_core::WorkbenchAppBuilder, window_title: &str) -> Result<(), String> {
    let mut builder = builder;
    let smoke = builder.smoke_seconds();
    let title: SharedString = window_title.to_string().into();
    let assets = builder.asset_source();
    // 产品声明的 GPU 设备需求：在窗口打开前交给前端（gpui 的平台层按
    // WgpuDeviceRequirements 合并）；未声明时用平台基线。
    let gpu_requirements = builder.take_gpu_requirements();
    let runtime = builder.build()?;
    let core: SharedCore = std::rc::Rc::new(Mutex::new(runtime));
    // 恢复上次的布局（模型 + 停靠区物理布局）：先落模型，窗口打开后首帧再把
    // dock 布局叠上去。无布局文件（首次启动）或解析失败时返回 None → 默认布局。
    let dock = {
        let mut core = core.lock().unwrap_or_else(|p| p.into_inner());
        core.load_layout()
            .and_then(|restored| restored.dock)
            .and_then(|dock| serde_json::from_value(dock).ok())
    };

    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(async move {
            // 字体在窗口打开前注入：cosmic-text 的回退解析与 gpui 的排版
            // 缓存都按内容哈希命中，启动后再加字体会一直命中旧字形。
            // 拉丁字体由本 crate 内嵌（fork gpui_web 是空字体库）。
            let mut fonts: Vec<std::borrow::Cow<'static, [u8]>> =
                vec![std::borrow::Cow::Borrowed(LATIN_FONT)];
            for name in ["cjk.ttf", "cjk.otf"] {
                if let Some(bytes) =
                    wb::asyncrt::fetch_bytes(&format!("assets/fonts/{name}")).await
                {
                    fonts.push(std::borrow::Cow::Owned(bytes));
                    break;
                }
            }
            let app = gpui_platform::application().with_assets(WbAssets {
                app: assets,
                component: gpui_component_assets::Assets::new(""),
            });
            let handle =
                app.run_embedded(launch_app(core, title, smoke, fonts, gpu_requirements, dock));
            // Web 平台的 run 立即返回，应用状态由句柄保持；
            // 与页面共存亡，故意泄漏句柄。
            std::mem::forget(handle);
        });
        return Ok(());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // 产品资源源 + gpui-component 内置资源兜底
        let app = gpui_platform::application().with_assets(WbAssets {
            app: assets,
            component: gpui_component_assets::Assets::new(""),
        });
        app.run(launch_app(
            core,
            title,
            smoke,
            Vec::new(),
            gpu_requirements,
            dock,
        ));
        Ok(())
    }
}

/// 构建应用启动闭包：注册组件、打开主窗口、挂任务泵与字体注入。
fn launch_app(
    core: SharedCore,
    title: SharedString,
    smoke: Option<f32>,
    fonts: Vec<std::borrow::Cow<'static, [u8]>>,
    gpu_requirements: Option<Box<dyn std::any::Any>>,
    dock: Option<gpui_component::dock::DockAreaState>,
) -> impl FnOnce(&mut App) {
    move |cx: &mut App| {
        if !fonts.is_empty() {
            let _ = cx.text_system().add_fonts(fonts);
        }
        // 产品声明的额外 GPU 设备需求必须在任何窗口（即任何设备）创建之前落位。
        if let Some(requirements) = gpu_requirements {
            cx.set_gpu_requirements(requirements);
        }
        gpui_component::init(cx);
        gpui_rhai::install(cx);
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
        let dock_window = dock.clone();
        let handle = cx
            .open_window(options, |_window, cx| {
                cx.new(|_cx| RootView::new(core_window, dock_window))
            })
            .expect("打开窗口失败");
        let _ = handle;
        cx.activate(true);

        // 有任务运行时以低频率刷新（进度动画）
        let root_entity = cx
            .active_window()
            .and_then(|w| w.downcast::<RootView>())
            .and_then(|h| h.entity(cx).ok());
        let weak_root = root_entity.as_ref().map(|e| e.downgrade());
        let core_loop = core.clone();

        // 关窗即存布局：核心部分（面板/标签/模式）+ 停靠区物理布局（dock dump）。
        // 多窗口（P1-4）之前，窗口只有一个，关闭即写当前布局。
        //
        // 这里持有根视图的**强**句柄：窗口关闭时它自己的句柄先被释放，弱引用
        // 会来不及 upgrade，布局就存不下来（实测过）。
        let core_save = core.clone();
        cx.on_window_closed(move |cx, _| {
            if let Some(root) = &root_entity {
                let dump = {
                    let view = root.read(cx);
                    view.dock.as_ref().and_then(|shell| shell.dump(cx))
                };
                let result = core_save
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .save_layout(serde_json::Value::Null, dump);
                // 存盘失败（只读目录、Web 无文件系统）只记日志，不阻断退出。
                let mut core = core_save.lock().unwrap_or_else(|p| p.into_inner());
                match result {
                    Ok(path) => core.host.log.info(format!("已保存布局 {}", path.display())),
                    Err(e) => core.host.log.warn(format!("保存布局失败: {e}")),
                }
            }
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

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
                    cx.update(|cx| notify(&weak, cx));
                }
            }
        })
        .detach();

        // 冒烟模式：N 秒后自动退出（仅桌面有效；Web 无退出语义）
        if let Some(secs) = smoke {
            if secs > 0.0 {
                cx.spawn(async move |cx: &mut gpui::AsyncApp| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs_f32(secs))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model snapshot the dock mirrors is built from the live workspace:
    /// all three side areas and the central tabs reach it, and each area
    /// carries its visible panels, active panel and size. This is the
    /// regression the retired splitter tests stood for — "every region is
    /// really wired" — now stated against the dock's input instead of the
    /// hand-written layout.
    #[test]
    fn the_shell_snapshot_covers_every_region() {
        let runtime = workbench_core::WorkbenchAppBuilder::new("test.dock", "Dock snapshot")
            .with_workbench(wb_example::ExampleWorkbench)
            .open_on_start("example.welcome", false)
            .build()
            .expect("runtime builds");
        let core: SharedCore = std::rc::Rc::new(Mutex::new(runtime));

        let snap = {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            let area_of = |a: wb::DockArea| -> dock_panel::AreaSnapshot {
                let slots: Vec<&wb::PanelSlot> = core
                    .host
                    .ws
                    .panels_in_area(a)
                    .into_iter()
                    .filter(|p| p.visible)
                    .collect();
                let active_id = core.host.ws.area_active.get(&a).cloned();
                dock_panel::AreaSnapshot {
                    size: slots
                        .iter()
                        .find(|p| Some(&p.id) == active_id.as_ref())
                        .or_else(|| slots.first())
                        .map(|p| p.size)
                        .unwrap_or(0.0),
                    active: active_id.map(|id| id.0),
                    panels: slots
                        .iter()
                        .map(|p| dock_panel::PanelSnapshot {
                            id: p.id.0.clone(),
                            script: p.script.clone(),
                        })
                        .collect(),
                }
            };
            dock_panel::ShellSnapshot {
                left: area_of(wb::DockArea::Left),
                right: area_of(wb::DockArea::Right),
                bottom: area_of(wb::DockArea::Bottom),
                tabs: core
                    .host
                    .ws
                    .tabs
                    .iter()
                    .map(|t| t.instance_id.0)
                    .collect(),
                active_tab: core.host.ws.active_tab().map(|t| t.instance_id.0),
            }
        };

        // 每条区域都带上了模型里的面板（示例 workbench 注册了 资源管理器/任务/属性/输出，
        // 平台另有 诊断/插件 落在右区）。
        assert_eq!(snap.left.panels[0].id, "example.panel.explorer");
        assert_eq!(snap.left.active.as_deref(), Some("example.panel.explorer"));
        assert_eq!(snap.left.size, 240.0);
        let right: Vec<&str> = snap.right.panels.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            right,
            vec![
                "example.panel.tasks",
                "example.panel.properties",
                "app.panel.diagnostics",
                "app.panel.plugins"
            ]
        );
        assert_eq!(snap.right.active.as_deref(), Some("example.panel.tasks"));
        assert_eq!(snap.bottom.panels[0].id, "example.panel.output");
        assert_eq!(snap.bottom.size, 170.0);

        // 中央标签与活动标签。
        assert_eq!(snap.tabs.len(), 1, "the welcome tab is open");
        assert_eq!(snap.active_tab, Some(snap.tabs[0]));
    }
}

