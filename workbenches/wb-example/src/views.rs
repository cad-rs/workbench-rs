//! Example Workbench views and panels.
//!
//! Every view builds its content as a gpui element tree in
//! [`wb::ViewInstance::element`], reading live state through the host it is
//! handed. A view that needs pointer input registers its own handlers from the
//! element's `paint` phase (gpui clears window listeners after every frame).

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AbsoluteLength, AnyElement, App, Bounds, Corners,
    Div, Edges, Element, ElementId, GlobalElementId, Hsla, InspectorElementId, InteractiveElement,
    IntoElement, LayoutId, Pixels, Point, Style, Window,
};
use workbench_api as wb;

use crate::shapes::ShapeDoc;

/// Example extension service: the selection shared by panels and views.
#[derive(Default)]
pub struct ExampleState {
    /// (document id, selected shape index)
    pub selected: Option<(wb::DocumentId, usize)>,
}

const CANVAS_VIEW: &str = "example.canvas";

// ---- shared element helpers ----

/// 0xRRGGBB to the Hsla that gpui's text and background styles take.
fn rgb(v: u32) -> Hsla {
    gpui::rgb_to_hsla(gpui::Rgba::new(
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
        1.0,
    ))
}

fn bg_panel() -> Hsla {
    rgb(0x292B33)
}
fn bg_canvas() -> Hsla {
    rgb(0x2B2E38)
}
fn bg_row() -> Hsla {
    rgb(0x333640)
}
fn bg_output() -> Hsla {
    rgb(0x1F2026)
}

fn line(text: String, color: Hsla, size: f32) -> Div {
    div().text_size(px(size)).text_color(color).child(text)
}

fn heading(text: &str) -> Div {
    div()
        .mt_2()
        .text_size(px(13.))
        .text_color(rgb(0x3D8BEC))
        .child(text.to_string())
}

// ---------------------------------------------------------------- 欢迎 ----

pub struct WelcomeView;

impl wb::ViewInstance for WelcomeView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.welcome")
    }
    fn title(&self) -> String {
        "欢迎".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let snap = host.snapshot();
        let doc_line = match &snap.active_document {
            Some(doc) => format!(
                "活动文档：{}{}（修订 r{}）",
                doc.title,
                if doc.dirty { " •未保存" } else { "" },
                doc.revision
            ),
            None => "活动文档：（无）".to_string(),
        };

        let rows: Vec<Div> = vec![
            div()
                .text_size(px(24.))
                .text_color(gpui::white())
                .child("workbench-rs 演示产品".to_string()),
            div().h(px(16.)),
            line(format!("工作模式：{}", snap.work_mode), rgb(0xC7C7CC), 14.0),
            line(doc_line, rgb(0xC7C7CC), 14.0),
            line(
                format!(
                    "活动视图：{}",
                    snap.active_view_type.as_deref().unwrap_or("（无）")
                ),
                rgb(0xC7C7CC),
                14.0,
            ),
            div().h(px(12.)),
            line(
                "· 工具栏「图形」→ 添加图形：在画布中添加并撤销/重做".to_string(),
                rgb(0xC7C7CC),
                14.0,
            ),
            line(
                "· 工具栏「演示」→ 批量生成：后台任务，可取消，完成后经文档版本校验提交".to_string(),
                rgb(0xC7C7CC),
                14.0,
            ),
            line(
                "· 工具栏「演示」→ 慢任务：观察状态栏进度与协作式取消".to_string(),
                rgb(0xC7C7CC),
                14.0,
            ),
            line(
                "· 画布中点击图形可选中，右侧属性/任务/输出面板联动".to_string(),
                rgb(0xC7C7CC),
                14.0,
            ),
            line(
                "· Ctrl+S 保存 / Ctrl+Z 撤销 / Ctrl+W 关闭标签；布局自动保存恢复".to_string(),
                rgb(0xC7C7CC),
                14.0,
            ),
        ];
        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .p_8()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

// ---------------------------------------------------------------- 画布 ----

#[derive(Default)]
pub struct CanvasView;

/// One shape as the canvas needs it.
#[derive(Clone)]
struct ShapeRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    round: bool,
    hue: f32,
}

/// The example canvas: draws the grid and the document's shapes, and hit-tests
/// pointer events against them.
struct CanvasElement {
    shapes: Vec<ShapeRect>,
    selected: Option<usize>,
    bounds: Rc<RefCell<Bounds<Pixels>>>,
}

impl CanvasElement {
    /// Register the click handler that hit-tests the shapes.
    fn register_input(&self, window: &mut Window) {
        let shapes = self.shapes.clone();
        let bounds = self.bounds.clone();
        window.on_mouse_event(move |ev: &gpui::MouseDownEvent, phase, _window, _cx| {
            if !phase.bubble() || ev.button != gpui::MouseButton::Left {
                return;
            }
            let b = *bounds.borrow();
            let origin = b.origin;
            let p = Point::new(ev.position.x - origin.x, ev.position.y - origin.y);
            let hit = shapes
                .iter()
                .rposition(|s| {
                    p.x >= px(s.x) && p.x <= px(s.x + s.w) && p.y >= px(s.y) && p.y <= px(s.y + s.h)
                })
                .map(|i| i.to_string())
                .unwrap_or_default();
            wb::dispatch(
                "example.select_shape",
                wb::CommandArgs::new().with("index", hit),
            );
        });
    }

    fn paint_scene(&self, window: &mut Window, bounds: Bounds<Pixels>) {
        let b = *self.bounds.borrow();
        let _ = b;
        let origin = bounds.origin;

        // Grid.
        let step = 32.0;
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        let mut x = step;
        while x < w {
            paint_quad(
                window,
                Bounds {
                    origin: Point::new(origin.x + px(x), origin.y),
                    size: gpui::size(px(1.), px(h)),
                },
                grid_line(),
            );
            x += step;
        }
        let mut y = step;
        while y < h {
            paint_quad(
                window,
                Bounds {
                    origin: Point::new(origin.x, origin.y + px(y)),
                    size: gpui::size(px(w), px(1.)),
                },
                grid_line(),
            );
            y += step;
        }

        // Shapes.
        for (i, s) in self.shapes.iter().enumerate() {
            let r = Bounds {
                origin: Point::new(origin.x + px(s.x), origin.y + px(s.y)),
                size: gpui::size(px(s.w), px(s.h)),
            };
            let radius = if s.round { s.h * 0.5 } else { 4.0 };
            paint_quad(window, r, hue_color(s.hue).into());
            if self.selected == Some(i) {
                let outer = Bounds {
                    origin: Point::new(r.origin.x - px(2.), r.origin.y - px(2.)),
                    size: gpui::size(r.size.width + px(4.), r.size.height + px(4.)),
                };
                let stroke = if s.round { (s.h + 8.0) * 0.5 } else { 6.0 };
                paint_quad_border(window, outer, stroke, gpui::white().into());
            }
        }
    }
}

impl IntoElement for CanvasElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CanvasElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        *self.bounds.borrow_mut() = bounds;
        self.register_input(window);
        self.paint_scene(window, bounds);
    }
}

impl wb::ViewInstance for CanvasView {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new(CANVAS_VIEW)
    }
    fn title(&self) -> String {
        "图形画布".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let snap = host.snapshot();
        let host = host.host();
        let doc_id = snap.active_document.as_ref().map(|d| d.id);
        let selected = doc_id.and_then(|id| {
            host.service::<ExampleState>()
                .and_then(|s| s.selected)
                .filter(|(d, _)| *d == id)
                .map(|(_, i)| i)
        });
        let shapes: Vec<ShapeRect> = doc_id
            .and_then(|doc| {
                host.docs.read::<ShapeDoc, Vec<ShapeRect>>(doc, |s| {
                    s.shapes
                        .iter()
                        .map(|sh| ShapeRect {
                            x: sh.x,
                            y: sh.y,
                            w: sh.w,
                            h: sh.h,
                            round: sh.round,
                            hue: sh.hue,
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let add_enabled = snap.active_document.is_some() && snap.active_view_is(CANVAS_VIEW);

        let canvas = CanvasElement {
            shapes,
            selected,
            bounds: Rc::new(RefCell::new(Bounds::default())),
        };

        let element: AnyElement = div()
            .size_full()
            .relative()
            .child(canvas)
            .child(add_button(add_enabled))
            .into_any_element();
        Some(Box::new(element))
    }
}

fn add_button(enabled: bool) -> Div {
    let button = div()
        .absolute()
        .left(px(10.))
        .top(px(10.))
        .px_3()
        .py_1()
        .rounded(px(4.))
        .bg(rgb(if enabled { 0x3D8BEC } else { 0x4A4D55 }))
        .text_size(px(13.))
        .text_color(gpui::white())
        .child("添加".to_string());
    if enabled {
        button.on_mouse_down(gpui::MouseButton::Left, |_ev, _window, _cx| {
            wb::dispatch("example.add_shape", wb::CommandArgs::default());
        })
    } else {
        button
    }
}

// ---------------------------------------------------------------- 面板 ----

/// Left: the open documents and their shape counts.
pub struct ExplorerPanel;

impl wb::ViewInstance for ExplorerPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.explorer")
    }
    fn title(&self) -> String {
        "资源管理器".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let snap = host.snapshot();
        let host = host.host();
        let docs: Vec<(String, bool, u64, usize)> = host
            .docs
            .docs()
            .map(|d| {
                let count = host
                    .docs
                    .read::<ShapeDoc, usize>(d.id, |s| s.shapes.len())
                    .unwrap_or(0);
                (d.title.clone(), d.dirty, d.revision, count)
            })
            .collect();

        let mut rows: Vec<Div> = vec![heading("打开的文档")];
        for (title, dirty, rev, count) in docs {
            let color = if dirty { rgb(0xE6BF33) } else { rgb(0xC7C7CC) };
            rows.push(line(
                format!("{title}{}", if dirty { " •" } else { "" }),
                color,
                13.0,
            ));
            rows.push(line(
                format!("图形 ×{count} · 修订 r{rev}"),
                rgb(0x808088),
                11.0,
            ));
        }
        rows.push(heading("上下文"));
        rows.push(line(
            format!("工作模式：{}", snap.work_mode),
            rgb(0xC7C7CC),
            12.0,
        ));
        rows.push(line(
            format!(
                "活动视图：{}",
                snap.active_view_type.as_deref().unwrap_or("（无）")
            ),
            rgb(0xC7C7CC),
            12.0,
        ));

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

/// Right: the background task list, with a progress bar and a cancel button.
pub struct TasksPanel;

impl wb::ViewInstance for TasksPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.tasks")
    }
    fn title(&self) -> String {
        "任务".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let tasks: Vec<wb::TaskInfo> = host.host().tasks.all().cloned().collect();
        let rows: Vec<Div> = if tasks.is_empty() {
            vec![line(
                "暂无后台任务。试试工具栏「演示」。".to_string(),
                rgb(0x808088),
                13.0,
            )]
        } else {
            tasks.iter().map(task_row).collect()
        };

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

fn task_row(task: &wb::TaskInfo) -> Div {
    let title_color = match task.status {
        wb::TaskStatus::Failed => rgb(0xD94D4D),
        wb::TaskStatus::Cancelled => rgb(0xE6BF33),
        wb::TaskStatus::Succeeded => rgb(0x40B359),
        _ => gpui::white(),
    };
    let status_line = format!(
        "[{}] {}{}",
        task.kind.as_str(),
        task.status.as_str(),
        task.stage
            .as_deref()
            .map(|s| format!(" · {s}"))
            .or_else(|| task.detail.clone().map(|d| format!(" · {d}")))
            .unwrap_or_default()
    );

    let mut row = div()
        .w_full()
        .p_2()
        .rounded(px(4.))
        .bg(bg_row())
        .flex()
        .flex_col()
        .gap_1()
        .child(line(
            format!("#{} {}", task.id.0, task.title),
            title_color,
            13.0,
        ))
        .child(line(status_line, rgb(0x808088), 11.0));

    if !task.status.is_terminal() {
        row = row.child(progress(task.progress)).child(cancel_button(task.id));
    }
    row
}

fn progress(fraction: Option<f32>) -> Div {
    let ratio = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    div()
        .h(px(8.))
        .w_full()
        .rounded(px(4.))
        .bg(rgb(0x1F1F26))
        .child(
            div()
                .h_full()
                .bg(rgb(0x3D8BEC))
                .w(AbsoluteLength::from(px(ratio * 320.0))),
        )
}

fn cancel_button(task_id: wb::TaskId) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded(px(3.))
        .bg(rgb(0x4A4D55))
        .text_size(px(11.))
        .child("取消".to_string())
        .on_mouse_down(gpui::MouseButton::Left, move |_ev, _window, _cx| {
            wb::dispatch(
                "app.tasks.cancel",
                wb::CommandArgs::new().with("id", task_id.0.to_string()),
            );
        })
}

/// Right: the selected shape's properties.
pub struct PropertiesPanel;

impl wb::ViewInstance for PropertiesPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.properties")
    }
    fn title(&self) -> String {
        "属性".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let snap = host.snapshot();
        let host = host.host();
        let Some(doc) = snap.active_document.as_ref().map(|d| d.id) else {
            return Some(simple_panel(
                "没有活动文档".to_string(),
                rgb(0x808088),
            ));
        };
        let selected = host
            .service::<ExampleState>()
            .and_then(|s| s.selected)
            .filter(|(d, _)| *d == doc)
            .map(|(_, i)| i);
        let shape = selected.and_then(|i| {
            host.docs.read::<ShapeDoc, (f32, f32, f32, f32, bool, f32)>(doc, |s| {
                s.shapes
                    .get(i)
                    .map(|sh| (sh.x, sh.y, sh.w, sh.h, sh.round, sh.hue))
                    .unwrap_or_default()
            })
        });

        let rows: Vec<Div> = match shape {
            Some((x, y, w, h, round, hue)) => {
                let mut rows = vec![heading(&format!("图形 #{}", selected.unwrap_or(0)))];
                for (k, v) in [
                    ("X", format!("{x:.0}")),
                    ("Y", format!("{y:.0}")),
                    ("宽", format!("{w:.0}")),
                    ("高", format!("{h:.0}")),
                    ("圆角", if round { "是" } else { "否" }.to_string()),
                    ("色相", format!("{hue:.0}°")),
                ] {
                    rows.push(kv(k, v));
                }
                rows.push(
                    div()
                        .w(px(48.))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(hue_color(hue)),
                );
                rows
            }
            None => vec![line(
                "在画布中点击图形以查看属性".to_string(),
                rgb(0x808088),
                13.0,
            )],
        };

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

/// Bottom: the platform log.
pub struct OutputPanel;

impl wb::ViewInstance for OutputPanel {
    fn type_id(&self) -> wb::ViewTypeId {
        wb::ViewTypeId::new("example.panel.output")
    }
    fn title(&self) -> String {
        "输出".to_string()
    }
    fn element(&mut self, host: &mut dyn wb::AppServices) -> Option<Box<dyn std::any::Any>> {
        let entries = host.host().log.tail(64);
        let rows: Vec<Div> = entries
            .iter()
            .map(|e| {
                let color = match e.level {
                    wb::LogLevel::Debug => rgb(0x808088),
                    wb::LogLevel::Info => rgb(0xC7C7CC),
                    wb::LogLevel::Warn => rgb(0xE6BF33),
                    wb::LogLevel::Error => rgb(0xD94D4D),
                };
                div()
                    .flex()
                    .gap_2()
                    .text_size(px(12.))
                    .child(
                        div()
                            .w(px(56.))
                            .text_color(rgb(0x808088))
                            .child(e.time.clone()),
                    )
                    .child(div().text_color(color).child(e.message.clone()))
            })
            .collect();

        let element: AnyElement = div()
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .children(rows)
            .into_any_element();
        Some(Box::new(element))
    }
}

// ---- helpers ----

fn simple_panel(message: String, color: Hsla) -> Box<dyn std::any::Any> {
    let element: AnyElement = div()
        .size_full()
        .flex()
        .flex_col()
        .p_3()
        .child(line(message, color, 13.0))
        .into_any_element();
    Box::new(element)
}

fn kv(key: &str, value: String) -> Div {
    div()
        .flex()
        .gap_2()
        .text_size(px(12.))
        .child(
            div()
                .w(px(56.))
                .text_color(rgb(0x808088))
                .child(key.to_string()),
        )
        .child(div().text_color(rgb(0xC7C7CC)).child(value))
}

/// A barely-visible white, for the canvas grid lines.
fn grid_line() -> Hsla {
    gpui::rgb_to_hsla(gpui::Rgba::new(1.0, 1.0, 1.0, 0.045))
}

/// A shape colour from a hue in degrees.
fn hue_color(hue: f32) -> Hsla {
    let (r, g, b) = hsl_to_rgb(hue, 0.65, 0.55);
    gpui::rgb_to_hsla(gpui::Rgba::new(r, g, b, 1.0))
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r1 + m, g1 + m, b1 + m)
}

fn paint_quad(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(gpui::quad(
        bounds,
        Corners::all(px(0.)),
        color,
        Edges::all(px(0.)),
        gpui::transparent_black(),
        gpui::BorderStyle::default(),
    ));
}

fn paint_quad_border(window: &mut Window, bounds: Bounds<Pixels>, width: f32, color: Hsla) {
    window.paint_quad(gpui::quad(
        bounds,
        Corners::all(px(0.)),
        gpui::transparent_black(),
        Edges::all(px(width)),
        color,
        gpui::BorderStyle::default(),
    ));
}
