//! gpui frontend for workbench-rs: renders the platform workspace model into
//! a gpui-ce window.
//!
//! This crate is the only place on the GUI side that touches gpui. Panels and
//! central views draw through `PaintBackend`; panels registered with a
//! `script` path are instead mounted as gpui-rhai script views (UiNode trees
//! authored in Rhai).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{
    canvas, div, prelude::*, px, quad, relative, rgb_to_hsla, size, svg, App, Bounds,
    Context, Corners, Edges, Hsla, KeyDownEvent, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, ScrollDelta, ScrollWheelEvent,
    SharedString, TextAlign, TextRun, TitlebarOptions, TransformationMatrix, Window,
    WindowBounds, WindowOptions,
};

use workbench_api as wb;
use workbench_core::AppRuntime;

pub mod gpu_view;

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
    drag: Arc<Mutex<Option<Drag>>>,
    /// gpui-rhai 脚本视图域（每窗口一个；首个脚本面板挂载时创建）。
    script_host: Option<gpui_rhai::ScriptViewHost>,
    /// 已挂载的脚本面板（面板 id → 句柄）。
    script_views: HashMap<String, gpui_rhai::ScriptViewHandle>,
    /// 脚本面板挂载失败原因（面板 id → 诊断；失败不重试）。
    script_errors: HashMap<String, String>,
}

impl RootView {
    fn new(core: SharedCore) -> Self {
        Self {
            core,
            last_render: web_time::Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
            mouse_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drag: Arc::new(Mutex::new(None)),
            script_host: None,
            script_views: HashMap::new(),
            script_errors: HashMap::new(),
        }
    }
}

fn notify(weak: &gpui::WeakEntity<RootView>, cx: &mut App) {
    if let Some(entity) = weak.upgrade() {
        entity.update(cx, |_, cx| cx.notify());
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
        let frame = FrameInput {
            events: events_snapshot,
            mouse_down: self.mouse_down.clone(),
        };

        let weak = cx.entity().downgrade();
        let core = self.core.clone();

        // ---- 核心状态读取（一次性） ----
        /// 渲染期快照：面板区域内的一个面板标签。
        struct PanelUi {
            id: String,
            title: String,
            active: bool,
            icon: Option<String>,
            /// 脚本面板的 Rhai 文件路径（None = PaintBackend 视图）
            script: Option<String>,
        }
        struct AreaUi {
            visible: bool,
            size: f32,
            panels: Vec<PanelUi>,
        }
        struct CentralTabUi {
            id: u64,
            title: String,
            active: bool,
            icon: Option<String>,
        }
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
                        .map(|p| PanelUi {
                            id: p.id.0.clone(),
                            title: p.title.clone(),
                            active: active_id.as_ref() == Some(&p.id),
                            icon: p.icon.clone(),
                            script: p.script.clone(),
                        })
                        .collect(),
                }
            };
            let left = area_of(wb::DockArea::Left);
            let right = area_of(wb::DockArea::Right);
            let bottom = area_of(wb::DockArea::Bottom);
            let central_active = core.host.ws.active_tab().map(|t| t.instance_id.0);
            let central_tabs: Vec<CentralTabUi> = core
                .host
                .ws
                .tabs
                .iter()
                .map(|t| CentralTabUi {
                    id: t.instance_id.0,
                    title: t.title.clone(),
                    active: Some(t.instance_id.0) == central_active,
                    icon: t.icon.clone(),
                })
                .collect();
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
            (left, right, bottom, central_tabs, central_active, ribbon_tabs, ribbon_groups, status)
        };

        // ---- 脚本面板挂载（每个活动脚本面板一次；需要 window/cx）----
        for area_ui in [&left, &right, &bottom] {
            if !area_ui.visible {
                continue;
            }
            let Some(panel) = area_ui.panels.iter().find(|p| p.active) else {
                continue;
            };
            let Some(script) = &panel.script else { continue };
            if self.script_views.contains_key(&panel.id)
                || self.script_errors.contains_key(&panel.id)
            {
                continue;
            }
            if self.script_host.is_none() {
                match gpui_rhai::ScriptViewHost::new("wb-main", cx) {
                    Ok(host) => self.script_host = Some(host),
                    Err(e) => {
                        self.script_errors
                            .insert(panel.id.clone(), format!("ScriptViewHost: {e}"));
                        continue;
                    }
                }
            }
            let host = self.script_host.as_ref().expect("刚已创建").clone();
            // gpui-rhai 的 view_id 只允许 ASCII 字母数字;面板 id 可能含 . /
            let view_id: String = panel
                .id
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            let mounted = gpui_rhai::FileScriptView::new(std::path::PathBuf::from(script))
                .prepare()
                .and_then(|prepared| {
                    prepared.mount(
                        gpui_rhai::ScriptViewConfig::new(view_id),
                        host,
                        window,
                        cx,
                    )
                });
            match mounted {
                Ok(handle) => {
                    self.script_views.insert(panel.id.clone(), handle);
                }
                Err(e) => {
                    self.script_errors.insert(panel.id.clone(), e.to_string());
                }
            }
        }

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

        // ---- 面板列（区域内的标签页 + 活动面板内容） ----
        let script_views = &self.script_views;
        let script_errors = &self.script_errors;
        let script_host = &self.script_host;
        let panel_column = |area: &AreaUi, key_prefix: &'static str| -> gpui::AnyElement {
            let mut header = div().flex().flex_row().items_center().gap_1();
            for panel in &area.panels {
                let mut tab = div()
                    .id(SharedString::from(format!("{key_prefix}-{}", panel.id)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .text_size(px(11.5))
                    .cursor_pointer();
                if let Some(icon) = &panel.icon {
                    tab = tab.child(
                        svg()
                            .path(icon.clone())
                            .size(px(13.0))
                            .text_color(if panel.active {
                                col(wb::Color::WHITE)
                            } else {
                                col(wb::Color::LIGHT_GRAY)
                            }),
                    );
                }
                tab = tab.child(panel.title.clone());
                tab = if panel.active {
                    tab.bg(col(wb::Color::ACCENT))
                        .text_color(col(wb::Color::WHITE))
                } else {
                    tab.text_color(col(wb::Color::LIGHT_GRAY))
                        .hover(|s| s.bg(col(wb::Color::rgb(0.26, 0.27, 0.32))))
                };
                let core_c = core.clone();
                let pid_c = panel.id.clone();
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
            let active_panel = area.panels.iter().find(|p| p.active).or_else(|| {
                area.panels.first()
            });
            let content: gpui::AnyElement = match active_panel {
                // 脚本面板：gpui-rhai 挂载的 UiNode 树（宿主 container 提供
                // overlay/focus 域；错误以面板内诊断呈现）
                Some(panel) if panel.script.is_some() => {
                    let pid = &panel.id;
                    if let Some(err) = script_errors.get(pid) {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_3()
                            .text_color(col(wb::Color::GRAY))
                            .text_size(px(12.0))
                            .child(format!("脚本面板加载失败：{err}"))
                            .into_any_element()
                    } else {
                        match script_views.get(pid) {
                            Some(handle) => match handle.flex_item() {
                                Ok(item) => match script_host.as_ref() {
                                    Some(host) => host.container(item),
                                    None => item,
                                },
                                Err(e) => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(col(wb::Color::GRAY))
                                    .text_size(px(12.0))
                                    .child(format!("脚本视图错误：{e}"))
                                    .into_any_element(),
                            },
                            None => div().into_any_element(),
                        }
                    }
                }
                Some(panel) => view_canvas(
                    core.clone(),
                    frame.clone(),
                    SlotKey::Panel(panel.id.clone()),
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
        for tab_ui in &central_tabs {
            let mut tab = div()
                .id(SharedString::from(format!("tab-{}", tab_ui.id)))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_size(px(12.0))
                .cursor_pointer();
            if let Some(icon) = &tab_ui.icon {
                tab = tab.child(
                    svg()
                        .path(icon.clone())
                        .size(px(13.0))
                        .text_color(if tab_ui.active {
                            col(wb::Color::WHITE)
                        } else {
                            col(wb::Color::LIGHT_GRAY)
                        }),
                );
            }
            tab = tab.child(tab_ui.title.clone());
            tab = if tab_ui.active {
                tab.bg(col(wb::Color::rgb(0.30, 0.32, 0.38)))
                    .text_color(col(wb::Color::WHITE))
            } else {
                tab.text_color(col(wb::Color::LIGHT_GRAY))
                    .hover(|s| s.bg(col(wb::Color::rgb(0.24, 0.25, 0.30))))
            };
            let core_c = core.clone();
            let id_c = tab_ui.id;
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
                .id(SharedString::from(format!("tab-close-{}", tab_ui.id)))
                .px_1()
                .rounded_sm()
                .text_size(px(11.0))
                .text_color(col(wb::Color::GRAY))
                .cursor_pointer()
                .hover(|s| s.bg(col(wb::Color::rgb(0.4, 0.2, 0.2))))
                .child("×");
            let core_c = core.clone();
            let id_c = tab_ui.id;
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
        // 窗口级输入监听。
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
    let core: SharedCore = std::rc::Rc::new(Mutex::new(builder.build()?));

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
            let handle = app.run_embedded(launch_app(core, title, smoke, fonts, gpu_requirements));
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
        app.run(launch_app(core, title, smoke, Vec::new(), gpu_requirements));
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

