//! egui 前端适配器：把平台工作区模型渲染为 egui/eframe 窗口。
//!
//! 本 crate 是 GUI 侧唯一接触 egui 的地方；Workbench 模块与平台核心不依赖 egui。

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Align2, Color32, CornerRadius, FontId, Frame, Sense, Stroke};
use egui_tiles::{Tile, TileId, Tree, UiResponse};

use workbench_api as wb;
use workbench_core::AppRuntime;

/// 树中的叶子：面板或中央标签。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum PaneKind {
    Panel(String),
    Central(u64),
}

/// PaintBackend 的 egui 实现。
struct EguiPainter<'a> {
    painter: &'a egui::Painter,
    origin: egui::Pos2,
}

impl<'a> EguiPainter<'a> {
    fn new(painter: &'a egui::Painter, origin: egui::Pos2) -> Self {
        Self { painter, origin }
    }
    fn pt(&self, p: wb::Point) -> egui::Pos2 {
        egui::pos2(self.origin.x + p.x, self.origin.y + p.y)
    }
    fn rect(&self, r: wb::Rect) -> egui::Rect {
        egui::Rect::from_min_size(self.pt(r.pos), egui::vec2(r.size.w, r.size.h))
    }
    fn color(c: wb::Color) -> Color32 {
        Color32::from_rgba_unmultiplied(
            (c.r * 255.0) as u8,
            (c.g * 255.0) as u8,
            (c.b * 255.0) as u8,
            (c.a * 255.0) as u8,
        )
    }
}

impl wb::PaintBackend for EguiPainter<'_> {
    fn fill_rect(&mut self, rect: wb::Rect, color: wb::Color, corner_radius: f32) {
        self.painter.rect_filled(
            self.rect(rect),
            CornerRadius::same(corner_radius as u8),
            Self::color(color),
        );
    }
    fn stroke_rect(&mut self, rect: wb::Rect, color: wb::Color, width: f32, corner_radius: f32) {
        self.painter.rect_stroke(
            self.rect(rect),
            CornerRadius::same(corner_radius as u8),
            Stroke::new(width, Self::color(color)),
            egui::StrokeKind::Inside,
        );
    }
    fn line(&mut self, a: wb::Point, b: wb::Point, color: wb::Color, width: f32) {
        self.painter
            .line_segment([self.pt(a), self.pt(b)], Stroke::new(width, Self::color(color)));
    }
    fn fill_circle(&mut self, center: wb::Point, radius: f32, color: wb::Color) {
        self.painter
            .circle_filled(self.pt(center), radius, Self::color(color));
    }
    fn text(&mut self, pos: wb::Point, font_size: f32, color: wb::Color, text: &str) {
        self.painter.text(
            self.pt(pos),
            Align2::LEFT_TOP,
            text,
            FontId::proportional(font_size),
            Self::color(color),
        );
    }
    fn text_size(&mut self, font_size: f32, text: &str) -> wb::Size {
        let galley = self.painter.layout_no_wrap(
            text.to_string(),
            FontId::proportional(font_size),
            Color32::WHITE,
        );
        let s = galley.size();
        wb::Size::new(s.x, s.y)
    }
}

/// 前端宿主。
struct EguiHost {
    core: AppRuntime,
    tree: Tree<PaneKind>,
    /// 结构签名：变更时重建树并触发保存。
    signature: u64,
    last_save: Instant,
    start: Instant,
    smoke_seconds: Option<f32>,
    /// 已镜像到 stdout 的日志游标。
    log_cursor: u64,
}

impl EguiHost {
    fn new(mut core: AppRuntime) -> Self {
        let saved_ui = core.load_layout();
        let mut host = Self {
            core,
            tree: Tree::empty("workbench"),
            signature: 0,
            last_save: Instant::now(),
            start: Instant::now(),
            smoke_seconds: None,
            log_cursor: 0,
        };
        let restored = matches!(&saved_ui, Some(ui) if host.try_restore_tree(ui));
        if !restored {
            host.rebuild_tree();
        }
        host.signature = host.compute_signature();
        host
    }

    // ---- 树 ----

    fn compute_signature(&self) -> u64 {
        let mut sig = 0u64;
        for p in &self.core.host.ws.panels {
            sig = sig.wrapping_mul(31).wrapping_add(hash_str(&p.id.0)).wrapping_add((p.visible as u64) * 7);
        }
        for t in &self.core.host.ws.tabs {
            sig = sig.wrapping_mul(31).wrapping_add(t.instance_id.0.wrapping_mul(13));
        }
        sig
    }

    /// 从核心状态重建树：左 | 中 | 右 横排，底部独立行。
    fn rebuild_tree(&mut self) {
        let mut tree: Tree<PaneKind> = Tree::empty("workbench");

        fn collect(tree: &mut Tree<PaneKind>, core: &AppRuntime, area: wb::DockArea) -> Option<TileId> {
            let panes: Vec<PaneKind> = core
                .host
                .ws
                .panels_in_area(area)
                .into_iter()
                .filter(|p| p.visible)
                .map(|p| PaneKind::Panel(p.id.0.clone()))
                .collect();
            if panes.is_empty() {
                None
            } else {
                let ids: Vec<TileId> = panes.into_iter().map(|p| tree.tiles.insert_pane(p)).collect();
                Some(tree.tiles.insert_tab_tile(ids))
            }
        }

        let left = collect(&mut tree, &self.core, wb::DockArea::Left);
        let right = collect(&mut tree, &self.core, wb::DockArea::Right);
        let bottom = collect(&mut tree, &self.core, wb::DockArea::Bottom);

        let central = if self.core.host.ws.tabs.is_empty() {
            tree.tiles.insert_pane(PaneKind::Central(u64::MAX)) // 占位空内容
        } else {
            let ids: Vec<TileId> = self
                .core
                .host
                .ws
                .tabs
                .iter()
                .map(|t| tree.tiles.insert_pane(PaneKind::Central(t.instance_id.0)))
                .collect();
            tree.tiles.insert_tab_tile(ids)
        };

        let mut main_children = Vec::new();
        if let Some(l) = left {
            main_children.push(l);
        }
        main_children.push(central);
        if let Some(r) = right {
            main_children.push(r);
        }
        let main = tree.tiles.insert_horizontal_tile(main_children);

        let root = match bottom {
            Some(b) => tree.tiles.insert_vertical_tile(vec![main, b]),
            None => main,
        };
        tree.root = Some(root);
        self.tree = tree;
    }

    /// 尝试从布局 JSON 恢复树；包含不可用面板/标签时返回 false（重建默认树）。
    fn try_restore_tree(&mut self, ui: &serde_json::Value) -> bool {
        let tree: Tree<PaneKind> = match serde_json::from_value(ui.clone()) {
            Ok(t) => t,
            Err(_) => return false,
        };
        let mut tree = tree;
        // 校验：所有面板引用存在且可见；所有中央标签存在
        let visible_panels: Vec<String> = self
            .core
            .host
            .ws
            .panels
            .iter()
            .filter(|p| p.visible)
            .map(|p| p.id.0.clone())
            .collect();
        let tab_ids: Vec<u64> = self.core.host.ws.tabs.iter().map(|t| t.instance_id.0).collect();
        for (_tid, tile) in tree.tiles.iter_mut() {
            if let Tile::Pane(kind) = tile {
                match kind {
                    PaneKind::Panel(id) => {
                        if !visible_panels.contains(id) {
                            return false;
                        }
                    }
                    PaneKind::Central(i) => {
                        if i != &u64::MAX && !tab_ids.contains(i) {
                            return false;
                        }
                    }
                }
            }
        }
        // 活动中央标签同步到核心
        for tile_id in tree.active_tiles() {
            if let Some(Tile::Pane(PaneKind::Central(i))) = tree.tiles.get(tile_id) {
                self.core
                    .host
                    .ws
                    .activate_tab_by_instance(wb::TabInstanceId(*i));
            }
        }
        self.tree = tree;
        true
    }

    fn save_layout_now(&self) {
        let ui = serde_json::json!({ "tree": &self.tree });
        let _ = self.core.save_layout(ui);
    }

    // ---- 快捷键 ----

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let hotkeys = self.core.hotkeys();
        let mut commands = Vec::new();
        ctx.input(|i| {
            for ev in &i.events {
                let egui::Event::Key { key, pressed, repeat, modifiers, .. } = ev else {
                    continue;
                };
                if !pressed || *repeat {
                    continue;
                }
                let Some(code) = map_key(*key) else { continue };
                let hk = wb::Hotkey {
                    ctrl: modifiers.ctrl,
                    alt: modifiers.alt,
                    shift: modifiers.shift,
                    key: code,
                };
                for (registered, cmd) in &hotkeys {
                    if *registered == hk {
                        commands.push(cmd.clone());
                    }
                }
            }
        });
        for cmd in commands {
            let _ = self.core.execute_command(&cmd, &wb::CommandArgs::default());
        }
    }
}

fn hash_str(s: &str) -> u64 {
    s.bytes().fold(14695981039346656037u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    })
}

fn map_key(key: egui::Key) -> Option<wb::KeyCode> {
    use egui::Key as K;
    use wb::KeyCode as C;
    Some(match key {
        K::A => C::A, K::B => C::B, K::C => C::C, K::D => C::D, K::E => C::E,
        K::F => C::F, K::G => C::G, K::H => C::H, K::I => C::I, K::J => C::J,
        K::K => C::K, K::L => C::L, K::M => C::M, K::N => C::N, K::O => C::O,
        K::P => C::P, K::Q => C::Q, K::R => C::R, K::S => C::S, K::T => C::T,
        K::U => C::U, K::V => C::V, K::W => C::W, K::X => C::X, K::Y => C::Y,
        K::Z => C::Z,
        K::Num0 => C::Num0, K::Num1 => C::Num1, K::Num2 => C::Num2, K::Num3 => C::Num3,
        K::Num4 => C::Num4, K::Num5 => C::Num5, K::Num6 => C::Num6, K::Num7 => C::Num7,
        K::Num8 => C::Num8, K::Num9 => C::Num9,
        K::F1 => C::F1, K::F2 => C::F2, K::F3 => C::F3, K::F4 => C::F4, K::F5 => C::F5,
        K::F6 => C::F6, K::F7 => C::F7, K::F8 => C::F8, K::F9 => C::F9, K::F10 => C::F10,
        K::F11 => C::F11, K::F12 => C::F12,
        K::Enter => C::Enter, K::Escape => C::Escape, K::Tab => C::Tab,
        K::Backspace => C::Backspace, K::Delete => C::Delete, K::Space => C::Space,
        K::ArrowLeft => C::Left, K::ArrowRight => C::Right,
        K::ArrowUp => C::Up, K::ArrowDown => C::Down,
        _ => return None,
    })
}

// ---- 行为适配 ----

struct WbBehavior<'a> {
    core: &'a mut AppRuntime,
    /// 中央标签点击 → 激活
    activated: Option<u64>,
}

/// 视图槽位定位。
enum SlotKey {
    Panel(String),
    Central(u64),
}

impl egui_tiles::Behavior<PaneKind> for WbBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PaneKind) -> UiResponse {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, ui.id().with("pane_body"), Sense::click_and_drag());
        match pane {
            PaneKind::Panel(id) => {
                paint_view(&mut *self.core, ui.painter(), rect, &resp, SlotKey::Panel(id.clone()));
            }
            PaneKind::Central(instance) => {
                paint_view(&mut *self.core, ui.painter(), rect, &resp, SlotKey::Central(*instance));
                if resp.clicked() {
                    self.activated = Some(*instance);
                }
            }
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &PaneKind) -> egui::WidgetText {
        match pane {
            PaneKind::Panel(id) => self
                .core
                .host
                .ws
                .panels
                .iter()
                .find(|p| &p.id.0 == id)
                .map(|p| p.title.clone())
                .unwrap_or_else(|| id.clone())
                .into(),
            PaneKind::Central(i) => self
                .core
                .host
                .ws
                .tabs
                .iter()
                .find(|t| t.instance_id.0 == *i)
                .map(|t| t.title.clone())
                .unwrap_or_else(|| "视图".to_string())
                .into(),
        }
    }

    fn is_tab_closable(&self, _tiles: &egui_tiles::Tiles<PaneKind>, _tile_id: TileId) -> bool {
        true
    }

    fn on_tab_close(&mut self, tiles: &mut egui_tiles::Tiles<PaneKind>, tile_id: TileId) -> bool {
        if let Some(Tile::Pane(PaneKind::Central(i))) = tiles.get(tile_id).cloned() {
            if i != u64::MAX {
                self.core
                    .host
                    .ws
                    .close_tab_by_instance(wb::TabInstanceId(i));
            }
            return true;
        }
        false
    }
}

/// 绘制一个视图实例（面板或中央标签）。
/// 实例存放在核心状态内，而绘制需要 &mut 核心——先换出、绘制、再放回。
fn paint_view(
    core: &mut AppRuntime,
    painter: &egui::Painter,
    rect: egui::Rect,
    resp: &egui::Response,
    key: SlotKey,
) {
    let area = wb::Rect::new(0.0, 0.0, rect.width(), rect.height());
    let input = wb::ViewInput {
        mouse_pos: resp
            .hover_pos()
            .map(|p| wb::Point::new(p.x - rect.left(), p.y - rect.top())),
        mouse_down: resp.is_pointer_button_down_on(),
        clicked: resp.clicked(),
    };
    // 换出实例（take 得到所有权，避免与核心的可变借用冲突）
    let mut instance: Option<Box<dyn wb::ViewInstance>> = match &key {
        SlotKey::Central(id) => core
            .host
            .ws
            .tabs
            .iter_mut()
            .find(|t| t.instance_id.0 == *id)
            .and_then(|t| t.instance.take()),
        SlotKey::Panel(id) => core
            .host
            .ws
            .panels
            .iter_mut()
            .find(|p| &p.id.0 == id)
            .and_then(|p| p.instance.take()),
    };
    {
        let Some(inst) = instance.as_mut() else { return };
        let mut painter_impl = EguiPainter::new(painter, rect.min);
        let mut ctx = wb::ViewCtx::new(&mut *core as &mut dyn wb::AppServices, input);
        inst.paint(&mut painter_impl, area, &mut ctx);
    }
    // 放回实例
    if let Some(inst) = instance.take() {
        match &key {
            SlotKey::Central(id) => {
                if let Some(slot) = core
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
                if let Some(slot) = core.host.ws.panels.iter_mut().find(|p| &p.id.0 == id) {
                    slot.instance = Some(inst);
                }
            }
        }
    }
}

impl eframe::App for EguiHost {
    /// 无 UI 的逻辑帧：任务泵、快捷键、结构同步与布局保存。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dt = ctx.input(|i| i.stable_dt);
        self.core.frame_tick(dt);
        self.core.record_frame(dt * 1000.0);
        // 输入事件探针（临时）
        {
            let hits: Vec<String> = ctx.input(|i| {
                i.events.iter().filter_map(|e| match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        Some(format!("{button:?} pressed={pressed} at {pos:?}"))
                    }
                    _ => None,
                }).collect()
            });
            if !hits.is_empty() {
                eprintln!("wb-input: {}", hits.join(" | "));
            }
        }
        // 平台日志镜像到 stderr（终端运行与自动化验证用）
        {
            let host = &self.core.host;
            for e in host.log.since(self.log_cursor) {
                eprintln!("wb-log [{}] {} {}", e.level.as_str().trim(), e.time, e.message);
            }
            self.log_cursor = host.log.last().map(|e| e.seq).unwrap_or(self.log_cursor);
        }
        self.handle_hotkeys(ctx);

        // 结构变更检测
        let sig = self.compute_signature();
        if sig != self.signature {
            self.signature = sig;
            self.rebuild_tree();
            self.save_layout_now();
        }

        // 布局节流保存
        if self.last_save.elapsed() > Duration::from_secs(3) {
            self.last_save = Instant::now();
            self.save_layout_now();
        }

        // 有运行中的任务时保持刷新（进度动画）
        let (running, _) = self.core.host.tasks.summary();
        if running > 0 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // 冒烟模式自动退出；期间保持重绘（egui 空闲时不产帧，logic 不会被调）
        if let Some(secs) = self.smoke_seconds {
            if self.start.elapsed() > Duration::from_secs_f32(secs) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // RibbonBar（design.md §7）：Tab 行 + 活动 Tab 的 Group 行
        egui::Panel::top("wb_ribbon")
            .default_size(88.0)
            .resizable(false)
            .frame(Frame::default().inner_margin(egui::Margin::symmetric(6, 0)))
            .show(ui, |ui| {
                // 快照：Tab 列表 + 活动 Tab 的组与命令状态
                struct GroupUi {
                    title: String,
                    items: Vec<(String, Option<String>, String, bool, Option<String>)>,
                }
                struct TabUi {
                    id: String,
                    title: String,
                    active: bool,
                }
                let (tabs, groups): (Vec<TabUi>, Vec<GroupUi>) = {
                    let ws = &self.core.host.ws;
                    let active_id = ws.active_ribbon_tab.clone();
                    let tabs = ws
                        .ribbon
                        .iter()
                        .map(|t| TabUi {
                            id: t.id.0.clone(),
                            title: t.title.clone(),
                            active: t.id.0 == active_id,
                        })
                        .collect();
                    let groups = ws
                        .active_ribbon()
                        .map(|tab| {
                            tab.groups
                                .iter()
                                .map(|g| GroupUi {
                                    title: g.title.clone(),
                                    items: g
                                        .items
                                        .iter()
                                        .map(|it| {
                                            let title = self
                                                .core
                                                .command_title(&it.command.0)
                                                .unwrap_or_else(|| it.command.0.clone());
                                            let enabled = self.core.is_enabled(&it.command.0);
                                            let hotkey =
                                                self.core.command_hotkey_label(&it.command.0);
                                            (
                                                it.command.0.clone(),
                                                it.label.clone(),
                                                title,
                                                enabled,
                                                hotkey,
                                            )
                                        })
                                        .collect(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (tabs, groups)
                };

                ui.vertical(|ui| {
                    // Tab 行
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        for tab in &tabs {
                            let mut text =
                                egui::RichText::new(&tab.title).size(13.0);
                            if tab.active {
                                text = text.strong();
                            }
                            let btn = egui::Button::new(text)
                                .fill(if tab.active {
                                    Color32::from_rgb(0x33, 0x36, 0x3e)
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(if tab.active {
                                    Stroke::new(1.5, Color32::from_rgb(0x33, 0x8d, 0xf2))
                                } else {
                                    Stroke::NONE
                                });
                            if ui.add(btn).clicked() {
                                self.core.host.ws.set_active_ribbon_tab(&tab.id);
                            }
                        }
                    });
                    ui.separator();
                    // Group 行：命令按钮在上、组名（与按钮行等宽、居中）在下
                    ui.horizontal(|ui| {
                        for (i, group) in groups.iter().enumerate() {
                            if i > 0 {
                                ui.separator();
                            }
                            ui.vertical(|ui| {
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    for (cmd, label, title, enabled, hotkey) in &group.items {
                                        let text =
                                            label.clone().unwrap_or_else(|| title.clone());
                                        let tip = match hotkey {
                                            Some(hk) => format!("{title}（{hk}）"),
                                            None => title.clone(),
                                        };
                                        let resp = ui
                                            .add_enabled(
                                                *enabled,
                                                egui::Button::new(
                                                    egui::RichText::new(text).size(13.0),
                                                ),
                                            )
                                            .on_hover_text(tip);
                                        if resp.clicked() && *enabled {
                                            let _ = self.core.execute_command(
                                                cmd,
                                                &wb::CommandArgs::default(),
                                            );
                                        }
                                    }
                                });
                                // 组名与按钮行等宽并居中（不撑开水平布局）
                                let w = ui.min_rect().width();
                                let title = egui::RichText::new(&group.title)
                                    .size(10.0)
                                    .weak();
                                let label =
                                    egui::Label::new(title).halign(egui::Align::Center);
                                ui.add_sized([w, 14.0], label);
                                ui.add_space(2.0);
                            });
                        }
                    });
                });
            });

        // 状态栏
        egui::Panel::bottom("wb_status")
            .default_size(28.0)
            .resizable(false)
            .frame(Frame::default().inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let snap = self.core.snapshot().clone();
                    let doc_text = match &snap.active_document {
                        Some(d) => format!(
                            "文档：{}{} · 修订 r{}",
                            d.title,
                            if d.dirty { " •" } else { "" },
                            d.revision
                        ),
                        None => "文档：（无）".to_string(),
                    };
                    ui.label(egui::RichText::new(doc_text).size(12.0));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("模式：{}", snap.work_mode)).size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(last) = self.core.host.log.last() {
                            let color = match last.level {
                                wb::LogLevel::Error => Color32::from_rgb(255, 120, 120),
                                wb::LogLevel::Warn => Color32::from_rgb(240, 210, 110),
                                _ => Color32::from_rgb(160, 165, 175),
                            };
                            ui.label(
                                egui::RichText::new(format!("{} {}", last.time, last.message))
                                    .size(11.0)
                                    .color(color),
                            );
                        }
                        let (running, last_task) = self.core.host.tasks.summary();
                        if running > 0 {
                            if let Some(t) = last_task {
                                ui.label(
                                    egui::RichText::new(format!("#{} {}", t.id.0, t.title))
                                        .size(12.0),
                                );
                                ui.add(
                                    egui::ProgressBar::new(t.progress.unwrap_or(0.0))
                                        .show_percentage()
                                        .desired_width(120.0),
                                );
                                if let Some(stage) = &t.stage {
                                    ui.label(egui::RichText::new(stage).size(11.0));
                                }
                                if t.cancellable {
                                    let tid = t.id;
                                    if ui
                                        .small_button(egui::RichText::new("取消").size(11.0))
                                        .clicked()
                                    {
                                        self.core.host.tasks.request_cancel(tid);
                                    }
                                }
                            }
                        }
                    });
                });
            });

        // 中央树
        egui::CentralPanel::no_frame().show(ui, |ui| {
            let core = &mut self.core;
            let tree = &mut self.tree;
            let mut behavior = WbBehavior {
                core,
                activated: None,
            };
            tree.ui(&mut behavior, ui);
            if let Some(i) = behavior.activated {
                core.host.ws.activate_tab_by_instance(wb::TabInstanceId(i));
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_layout_now();
    }
}

/// 安装中文字体：egui 内置字体缺 CJK 字形，加载系统字体（Microsoft YaHei 等）
/// 作为回退。在候选路径中取第一个存在的文件。
fn install_cjk_fonts(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\msyhbd.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    let Some(path) = CANDIDATES.iter().map(std::path::Path::new).find(|p| p.exists()) else {
        return;
    };
    let Ok(data) = std::fs::read(path) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".into(),
        std::sync::Arc::new(egui::FontData::from_owned(data)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk".into());
    }
    ctx.set_fonts(fonts);
}

/// egui 入口：构建产品并运行主循环。
pub fn run(
    builder: workbench_core::WorkbenchAppBuilder,
    window_title: &str,
) -> Result<(), eframe::Error> {
    let smoke = builder.smoke_seconds();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_title(window_title),
        ..Default::default()
    };
    eframe::run_native(
        window_title,
        options,
        Box::new(move |cc| {
            install_cjk_fonts(&cc.egui_ctx);
            let core = builder.build().expect("产品装配失败");
            let mut host = EguiHost::new(core);
            host.smoke_seconds = smoke;
            Ok(Box::new(host))
        }),
    )
}
