//! gpui frontend for workbench-rs: renders the platform workspace model into
//! a gpui-ce window.
//!
//! This crate is the only place on the GUI side that touches gpui. Panels and
//! central views draw through `PaintBackend`; panels registered with a
//! `script` path are instead mounted as gpui-rhai script views (UiNode trees
//! authored in Rhai).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, quad, relative, rgb_to_hsla, size, AnyElement, App, Bounds, Context, Corners, Edges,
    ElementId, GlobalElementId, InspectorElementId, Hsla, KeyDownEvent, LayoutId,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, Rgba, ScrollDelta,
    ScrollWheelEvent, SharedString, Style, StyleRefinement, TextAlign, TextRun, TitlebarOptions,
    TransformationMatrix, Window, WindowBounds, WindowOptions,
};

use workbench_api as wb;
use workbench_core::AppRuntime;

pub mod gpu_view;

/// Built-in platform panels (diagnostics / plugins).
pub mod platform_panels;

pub(crate) mod dock_panel;

/// Ribbon 挂载桥（唯一 ribbon crate 耦合点，ribbon-bar.md §5.1）。
pub(crate) mod ribbon_bridge;

/// UI 主题（dark/light）：shell 配色 + 全窗口主题应用。
pub mod theme;

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
        // `app` is the builder's source, which already chains product assets
        // with the platform standard icons (wb-std/*) — see
        // WorkbenchAppBuilder::asset_source.
        if let Some(bytes) = self.app.load(path) {
            return Ok(Some(bytes));
        }
        self.component.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        // gpui 的 AssetRegistry 由 list("") 枚举建表（AssetSource::iter_ondemand）：
        // 只登记这里报出的路径，未登记的路径 load 永远 miss（图标静默不渲染）。
        // 因此根路径必须并集产品/平台标准路径与组件内置路径；同名路径（如
        // icons/delete.svg 两边都有）去重——registry 重复 insert 会报错。
        let mut paths: Vec<SharedString> = Vec::new();
        if path.is_empty() {
            let mut seen = std::collections::HashSet::new();
            for p in self
                .app
                .paths()
                .into_iter()
                .map(SharedString::from)
                .chain(self.component.list(path)?.into_iter())
            {
                if seen.insert(p.clone()) {
                    paths.push(p.into());
                }
            }
        }
        Ok(paths)
    }
}

// ---- 颜色转换 ----

pub fn col(c: wb::Color) -> Hsla {
    rgb_to_hsla(Rgba::new(c.r, c.g, c.b, c.a))
}

fn black() -> Hsla {
    rgb_to_hsla(Rgba::new(0.0, 0.0, 0.0, 1.0))
}

fn ui_font() -> gpui::Font {
    gpui::Font {
        // ".SystemUIFont" 由 gpui 按平台解析为系统字体（Windows 上为 Segoe UI）
        family: ".SystemUIFont".into(),
        ..Default::default()
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
    /// The dock area and its mirror of the model (built on the first frame:
    /// `RootView::new` has no window to build it with).
    dock: Option<dock_panel::DockShell>,
    /// The persisted dock layout to re-establish, applied on the first frame
    /// after the model-driven sync has built this session's panels.
    pending_dock: Option<gpui_component::dock::DockAreaState>,
    /// gpui-rhai script view domain (per window; created at the first script
    /// panel mount, shared with the panels that draw them).
    scripts: dock_panel::SharedScripts,
    /// Frontend-owned view instances, one per open slot.
    views: SharedViews,
    /// What the dock is showing, as last mirrored into the model.
    mirror: dock_panel::SharedMirror,
    /// Ribbon 挂载点（ribbon crate 实体；首帧懒构建，模式同 dock）。
    ribbon: ribbon_bridge::RibbonHost,
}

impl RootView {
    fn new(core: SharedCore, pending_dock: Option<gpui_component::dock::DockAreaState>) -> Self {
        Self {
            core,
            last_render: web_time::Instant::now(),
            dock: None,
            pending_dock,
            scripts: std::rc::Rc::new(std::cell::RefCell::new(
                dock_panel::ScriptRuntime::default(),
            )),
            views: std::rc::Rc::new(std::cell::RefCell::new(ViewRuntime::default())),
            mirror: std::rc::Rc::new(std::cell::RefCell::new(
                dock_panel::DockMirror::default(),
            )),
            ribbon: ribbon_bridge::RibbonHost::new(),
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
        col(theme::ui_theme().placeholder),
        Edges::all(px(0.)),
        gpui::transparent_black(),
        gpui::BorderStyle::default(),
    );
    window.paint_quad(q);
}

/// 面板/中央视图绘制到 canvas。
/// 实例存放在核心状态内，绘制需要 &mut 核心——先换出、绘制、再放回。
/// The element that renders one workspace slot (central tab or dock panel).
///
/// This replaces the former `canvas`-based slot. `canvas` is a one-shot
/// element — both of its closures are `FnOnce` — so a slot that lives as long
/// as its tab or panel needs a real `Element` whose state survives across
/// frames.
/// Frontend-owned view instances, keyed by slot.
///
/// The model keeps only slot metadata; the frontend creates each instance from
/// its view type's factory the first time it mounts the slot and keeps it
/// alive as long as the slot is open.
#[derive(Default)]
pub(crate) struct ViewRuntime {
    tabs: HashMap<u64, Rc<RefCell<Box<dyn wb::ViewInstance>>>>,
    panels: HashMap<String, Rc<RefCell<Box<dyn wb::ViewInstance>>>>,
}

pub(crate) type SharedViews = Rc<RefCell<ViewRuntime>>;

impl ViewRuntime {
    /// The instance for a central tab, creating it on first use.
    pub(crate) fn tab(
        &mut self,
        core: &SharedCore,
        instance_id: u64,
        type_id: &str,
    ) -> Option<Rc<RefCell<Box<dyn wb::ViewInstance>>>> {
        if let Some(v) = self.tabs.get(&instance_id) {
            return Some(v.clone());
        }
        let instance = {
            let guard = core.lock().ok()?;
            let def = guard.host.ws.view_def(&wb::ViewTypeId::new(type_id))?;
            (def.factory)()
        };
        let rc = Rc::new(RefCell::new(instance));
        self.tabs.insert(instance_id, rc.clone());
        Some(rc)
    }

    /// The instance for a dock panel, creating it on first use. Script panels
    /// have no Rust instance (their content is a gpui-rhai view).
    pub(crate) fn panel(
        &mut self,
        core: &SharedCore,
        panel_id: &str,
    ) -> Option<Rc<RefCell<Box<dyn wb::ViewInstance>>>> {
        if let Some(v) = self.panels.get(panel_id) {
            return Some(v.clone());
        }
        let instance = {
            let guard = core.lock().ok()?;
            let def = guard.host.ws.panel_def(&wb::PanelId::new(panel_id))?;
            (def.factory)()
        };
        let rc = Rc::new(RefCell::new(instance));
        self.panels.insert(panel_id.to_string(), rc.clone());
        Some(rc)
    }

    /// Drop the instances of slots the model no longer has.
    pub(crate) fn prune(&mut self, live_tabs: &[u64], live_panels: &[String]) {
        self.tabs.retain(|id, _| live_tabs.contains(id));
        self.panels.retain(|id, _| live_panels.contains(id));
    }
}

/// The view type id of a central tab, as the model stores it.
fn tab_type_id(core: &SharedCore, instance_id: u64) -> Option<String> {
    let guard = core.lock().ok()?;
    guard
        .host
        .ws
        .tabs
        .iter()
        .find(|t| t.instance_id.0 == instance_id)
        .map(|t| t.type_id.0.clone())
}

/// The script path of a central tab, as the model stores it (None for a Rust view).
fn tab_script(core: &SharedCore, instance_id: u64) -> Option<String> {
    let guard = core.lock().ok()?;
    guard
        .host
        .ws
        .tabs
        .iter()
        .find(|t| t.instance_id.0 == instance_id)
        .and_then(|t| t.script.clone())
}

/// The placeholder shown for a slot whose view produced no element.
fn slot_placeholder() -> AnyElement {
    div()
        .size_full()
        .bg(col(theme::ui_theme().placeholder))
        .into_any_element()
}

/// The element a dock panel mounts for one workspace slot.
///
/// The view builds its own content as a frontend element tree, reading live
/// state through the platform host. The handle is type-erased — on the gpui
/// frontend it is a `gpui::AnyElement`.
fn view_canvas(
    views: SharedViews,
    core: SharedCore,
    scripts: dock_panel::SharedScripts,
    key: SlotKey,
) -> AnyElement {
    // A script central tab has no Rust instance: the frontend mounts the script
    // view and draws it here, keyed by the tab instance (`wb.tab.{id}`).
    if let SlotKey::Central(id) = &key {
        if tab_script(&core, *id).is_some() {
            return scripts.borrow().element(&format!("wb.tab.{id}"));
        }
    }

    let instance = match &key {
        SlotKey::Central(id) => match tab_type_id(&core, *id) {
            Some(type_id) => views.borrow_mut().tab(&core, *id, &type_id),
            None => None,
        },
        SlotKey::Panel(id) => views.borrow_mut().panel(&core, id),
    };

    let Some(instance) = instance else {
        return slot_placeholder();
    };

    // The view reads the host while building its element tree, so the model
    // lock is held for the duration of `element`; the published dispatcher lets
    // its event handlers reach the same runtime. Published per frame and kept
    // until the next frame's publish (round-191: clearing it here made every
    // view-content button dead — gpui fires click handlers between frames,
    // after the clear, so wb::dispatch silently failed with
    // "视图元素不在渲染上下文中"). The handle points at the same live runtime
    // the view was built from; a new frame always replaces it before any
    // further handler runs.
    let element = {
        let Ok(mut guard) = core.lock() else {
            return slot_placeholder();
        };
        let dispatcher_core = core.clone();
        wb::publish_dispatcher(std::rc::Rc::new(move |command, args| {
            let mut guard = dispatcher_core.lock().unwrap_or_else(|p| p.into_inner());
            guard.execute_command(command, &args)
        }));
        let built = instance
            .borrow_mut()
            .element(&mut *guard as &mut dyn wb::AppServices);
        built
    };

    match element {
        Some(handle) => match handle.downcast::<AnyElement>() {
            Ok(element) => *element,
            Err(_) => slot_placeholder(),
        },
        None => slot_placeholder(),
    }
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
        let weak = cx.entity().downgrade();
        let core = self.core.clone();

        // ---- 核心状态读取（一次性） ----
        let (shell_snap, status) = {
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
            (shell_snap, status)
        };

        // ---- Dock 同步（模型 → dock；结构变化才重建） ----
        if self.dock.is_none() {
            self.dock = Some(dock_panel::DockShell::new(
                self.core.clone(),
                self.scripts.clone(),
                self.views.clone(),
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

        // ---- RibbonBar（ribbon crate 挂载；ribbon_bridge 每帧喂模型快照） ----
        self.ribbon.ensure_mounted(&core, weak.clone(), cx);
        self.ribbon.sync(&core, cx);
        let ribbon_element = self.ribbon.element();

        // ---- 组装 ----
        let mut root_col = div()
            .flex()
            .flex_col()
            .size_full()
            .font_family("Segoe UI")
            .bg(col(theme::ui_theme().bg))
            .text_color(col(wb::Color::LIGHT_GRAY))
            .child(ribbon_element)
            .child(dock_element);

        root_col = root_col.child(render_status_bar(&status, core.clone(), weak.clone()));

        // Keyboard shortcuts. Pointer input is not collected here: views are
        // element trees now, so a view registers its own handlers from its
        // `Element::paint` (gpui clears window listeners after every frame).
        root_col = root_col
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
                    .bg(col(theme::ui_theme().progress_track))
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
                        .bg(col(theme::ui_theme().cancel_rest))
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
        .bg(col(theme::ui_theme().toolbar))
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
    // The built-in platform panels are view implementations, so the frontend
    // registers them — before core's `platform_cmds::register` builds the
    // "窗口" toolbar group, which then picks them up with every other panel.
    builder = builder.with_plugin_stage(Box::new(platform_panels::register));
    let smoke = builder.smoke_seconds();
    let title: SharedString = window_title.to_string().into();
    let assets = builder.asset_source();
    // 产品声明的 GPU 设备需求：在窗口打开前交给前端（gpui 的平台层按
    // WgpuDeviceRequirements 合并）；未声明时用平台基线。
    let gpu_requirements = builder.take_gpu_requirements();
    let runtime = builder.build()?;
    let core: SharedCore = std::rc::Rc::new(Mutex::new(runtime));
    // Theme from settings (builder default already seeded): applied after
    // gpui_component::init in launch_app, before any window opens.
    let theme_name = {
        let core = core.lock().unwrap_or_else(|p| p.into_inner());
        core.host.settings.get_str("ui.theme", "dark")
    };
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
                app.run_embedded(launch_app(core, title, smoke, fonts, gpu_requirements, dock, theme_name));
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
            theme_name,
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
    theme_name: String,
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
        // 主题在组件初始化后、任何窗口打开前应用：shell 配色、ribbon tokens
        // 与 gpui-component 全局主题一次切齐。
        theme::apply_theme(&theme_name, cx);
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
        // 单窗口单工程是产品的既定形态（web 与桌面同构），所以一份布局 = 一个窗口。
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

    /// Regression (icons silently missing): gpui builds its `AssetRegistry`
    /// from `AssetSource::list("")`, and the registry resolves SVGs by table
    /// lookup — a path that `list` never reports can never render. WbAssets
    /// must therefore report the product and platform-standard paths alongside
    /// the component's built-in icons.
    #[test]
    fn wb_assets_lists_product_standard_and_component_paths() {
        use gpui::AssetSource as _;
        // Mirror the production chain: product assets + platform standard
        // icons (WorkbenchAppBuilder::asset_source).
        let app: std::sync::Arc<dyn wb::AssetSource> = std::sync::Arc::new(
            wb::ChainedAssets::new(vec![
                std::sync::Arc::new(wb_example::example_assets()),
                std::sync::Arc::new(*workbench_core::assets::standard_assets()),
            ]),
        );
        let assets = WbAssets {
            app,
            component: gpui_component_assets::Assets::new(""),
        };
        let listed = assets.list("").expect("list root");
        let has = |p: &str| listed.iter().any(|s| s.as_ref() == p);
        assert!(has("icons/scan.svg"), "product icon not listed");
        assert!(has("wb-std/new.svg"), "platform standard icon not listed");
        assert!(listed.len() >= 5, "component icons missing from the union");
        assert!(assets.load("icons/scan.svg").unwrap().is_some());
        assert!(assets.load("wb-std/new.svg").unwrap().is_some());
    }

    /// The model snapshot the dock mirrors is built from the live workspace:
    /// all three side areas and the central tabs reach it, and each area
    /// carries its visible panels, active panel and size. This is the
    /// regression the retired splitter tests stood for — "every region is
    /// really wired" — now stated against the dock's input instead of the
    /// hand-written layout.
    #[test]
    fn the_shell_snapshot_covers_every_region() {
        // Mirror `run()`: the platform panels are registered by the frontend.
        let runtime = workbench_core::WorkbenchAppBuilder::new("test.dock", "Dock snapshot")
            .with_workbench(wb_example::ExampleWorkbench)
            .with_plugin_stage(Box::new(platform_panels::register))
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

