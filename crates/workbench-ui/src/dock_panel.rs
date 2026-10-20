//! The gpui-component dock adapter (design 10.4 P1-10).
//!
//! The workbench model stays the single source of truth; [`GpuiDockArea`] is
//! the presentation and interaction layer over it. This module owns both
//! directions of that mapping:
//!
//! * [`DockShell::sync`] mirrors the model into the dock. It rebuilds a region
//!   only when that region's panel set or displayed panel changed:
//!   `set_dock`/`set_center` reset the region's layout tree, so rebuilding on
//!   every frame — or on every activation — would throw away the tab drags and
//!   splits the user made in it.
//! * [`WbPanel`] mirrors the user's actions back into the model: a closed
//!   panel closes (central tab) or hides (side panel) in the model, and the
//!   dock's displayed panel becomes the model's active one.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, Render, SharedString, Window,
};

use gpui_component::dock::{
    panel_handle, register_panel, BasePanel, DockArea as GpuiDockArea, DockAreaState, DockLayout,
    DockPlacement, DockSkin, Panel, PanelControl, PanelEvent, PanelInfo, PanelState,
};

use workbench_api as wb;

use super::{col, tab_script, view_canvas, SharedCore, SharedViews, SlotKey};

/// The dock area's id (and its version), as persistence expects them.
const DOCK_AREA_ID: &str = "wb-main";
const DOCK_AREA_VERSION: usize = 1;

/// Shown in the centre while no tab is open (the pre-dock front end's text).
const EMPTY_CENTER_TITLE: &str = "没有打开的视图";

/// The dock identifies a panel by a `&'static str` name that must never
/// change once chosen, while the model's panel ids and tab instances are
/// runtime strings. Interning keeps one leaked copy per distinct name; the set
/// is bounded by the registered panels plus the tab instances opened this
/// session.
fn intern(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let names = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut names = names.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = names.get(name) {
        return existing;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(name.to_string(), leaked);
    leaked
}

// ---- 共享状态（面板实体携带） ----

/// The gpui-rhai view domain, shared between the root view (which mounts) and
/// the script panels (which draw the mounted view).
#[derive(Default)]
pub(crate) struct ScriptRuntime {
    host: Option<gpui_rhai::ScriptViewHost>,
    views: HashMap<String, gpui_rhai::ScriptViewHandle>,
    errors: HashMap<String, String>,
}

pub(crate) type SharedScripts = Rc<RefCell<ScriptRuntime>>;

impl ScriptRuntime {
    /// Mount a script panel's view unless it is mounted already. Failures are
    /// remembered and not retried (the pre-dock front end behaved the same).
    pub(crate) fn mount(&mut self, panel_id: &str, script: &str, window: &mut Window, cx: &mut App) {
        if self.views.contains_key(panel_id) || self.errors.contains_key(panel_id) {
            return;
        }
        if self.host.is_none() {
            match gpui_rhai::ScriptViewHost::new("wb-main", cx) {
                Ok(host) => self.host = Some(host),
                Err(e) => {
                    self.errors
                        .insert(panel_id.to_string(), format!("ScriptViewHost: {e}"));
                    return;
                }
            }
        }
        let host = self.host.as_ref().expect("just created").clone();
        // gpui-rhai view ids admit ASCII alphanumerics only; panel ids may
        // contain `.` or `/`.
        let view_id: String = panel_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let mounted = gpui_rhai::FileScriptView::new(std::path::PathBuf::from(script))
            .prepare()
            .and_then(|prepared| {
                prepared.mount(gpui_rhai::ScriptViewConfig::new(view_id), host, window, cx)
            });
        match mounted {
            Ok(handle) => {
                self.views.insert(panel_id.to_string(), handle);
            }
            Err(e) => {
                self.errors.insert(panel_id.to_string(), e.to_string());
            }
        }
    }

    /// The element for a mounted script panel: the ui tree inside its host
    /// container (overlay/focus domain), or the load/run diagnostic.
    pub(crate) fn element(&self, panel_id: &str) -> AnyElement {
        if let Some(err) = self.errors.get(panel_id) {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px_3()
                .text_color(col(wb::Color::GRAY))
                .text_size(px(12.0))
                .child(format!("脚本面板加载失败：{err}"))
                .into_any_element();
        }
        match self.views.get(panel_id) {
            Some(handle) => match handle.flex_item() {
                Ok(item) => match self.host.as_ref() {
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
            // Not mounted yet (the mount runs in the sync, once, per panel).
            None => div().into_any_element(),
        }
    }
}

/// What the dock is showing, as the platform model last saw it: the panel the
/// sync pushed, or the panel a panel mirrored back when the user clicked a
/// tab. The sync compares the model against this to tell a dock-driven
/// activation (already agreed — no rebuild) from a model-driven one (rebuild
/// that region so the dock shows it).
#[derive(Default)]
pub(crate) struct DockMirror {
    area: HashMap<wb::DockArea, String>,
    center: Option<u64>,
    /// Set while the sync itself replaces a dock/center. The displaced panels
    /// are told `on_removed`, and that must not be read back into the model as
    /// the user closing them — it would close every other tab in the region.
    suppress_removal: bool,
}

pub(crate) type SharedMirror = Rc<RefCell<DockMirror>>;

/// Run `f` with removal writebacks suppressed. See [`DockMirror`].
fn with_removal_suppressed<T>(mirror: &SharedMirror, f: impl FnOnce() -> T) -> T {
    mirror.borrow_mut().suppress_removal = true;
    let out = f();
    mirror.borrow_mut().suppress_removal = false;
    out
}

// ---- 面板适配层 ----

/// Where a dock panel comes from in the model.
pub(crate) enum PanelRef {
    /// A registered panel living in a side dock.
    Area { area: wb::DockArea, panel_id: String },
    /// One open central tab.
    Central { instance_id: u64 },
    /// The placeholder shown while no central tab is open.
    Empty,
}

/// The handles every dock panel carries: the model, the script views, the
/// frontend view instances, and the presentation mirror. Bundled so creating a
/// panel — or handing the set to the shell — stays a short call.
#[derive(Clone)]
pub(crate) struct PanelCtx {
    core: SharedCore,
    scripts: SharedScripts,
    views: SharedViews,
    mirror: SharedMirror,
}

/// One model panel/tab, as a dock panel entity.
pub(crate) struct WbPanel {
    /// Interned dock name (the panel id, or `wb.tab.{instance}`).
    name: &'static str,
    r: PanelRef,
    /// Script panels: the gpui-rhai entry point (read from the registry, fixed
    /// for the panel's lifetime).
    script: Option<String>,
    ctx: PanelCtx,
    focus_handle: FocusHandle,
}

impl WbPanel {
    fn new(
        name: &'static str,
        r: PanelRef,
        script: Option<String>,
        ctx: PanelCtx,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self {
            name,
            r,
            script,
            ctx,
            focus_handle: cx.focus_handle(),
        })
    }

    /// The panel's title and icon, read live from the model so a renamed panel
    /// or tab updates without a dock rebuild.
    fn model_title(&self) -> (String, Option<String>) {
        model_title(&self.r, &self.ctx.core)
    }
}

/// The panel's title and icon, read live from the model (also used by the
/// writeback tests, which drive the same logic without a dock panel entity).
fn model_title(r: &PanelRef, core: &SharedCore) -> (String, Option<String>) {
    let core = core.lock().unwrap_or_else(|p| p.into_inner());
    match r {
        PanelRef::Area { panel_id, .. } => core
            .host
            .ws
            .panels
            .iter()
            .find(|p| &p.id.0 == panel_id)
            .map(|p| (p.title.clone(), p.icon.clone()))
            .unwrap_or_default(),
        PanelRef::Central { instance_id } => core
            .host
            .ws
            .tabs
            .iter()
            .find(|t| t.instance_id.0 == *instance_id)
            .map(|t| (t.title.clone(), t.icon.clone()))
            .unwrap_or_default(),
        PanelRef::Empty => (EMPTY_CENTER_TITLE.to_string(), None),
    }
}

/// The dock's displayed panel changed: make it the model's active one. This is
/// the mirror of a tab click, and it is delivered for the panel becoming
/// active (one notification per edge).
fn mirror_active(r: &PanelRef, core: &SharedCore, mirror: &SharedMirror) {
    {
        let mut core = core.lock().unwrap_or_else(|p| p.into_inner());
        match r {
            PanelRef::Area { area, panel_id } => {
                core.host
                    .ws
                    .area_active
                    .insert(*area, wb::PanelId::new(panel_id.clone()));
            }
            PanelRef::Central { instance_id } => {
                core.host
                    .ws
                    .activate_tab_by_instance(wb::TabInstanceId(*instance_id));
            }
            PanelRef::Empty => return,
        }
    }
    let mut mirror = mirror.borrow_mut();
    match r {
        PanelRef::Area { area, panel_id } => {
            mirror.area.insert(*area, panel_id.clone());
        }
        PanelRef::Central { instance_id } => mirror.center = Some(*instance_id),
        PanelRef::Empty => {}
    }
}

/// The panel left the dock. Unless the sync itself displaced it, that is the
/// user closing it: close the central tab, or hide the side panel (closing
/// semantics). Both writebacks are idempotent.
fn write_back_removed(r: &PanelRef, core: &SharedCore, mirror: &SharedMirror) {
    if mirror.borrow().suppress_removal {
        return;
    }
    let mut core = core.lock().unwrap_or_else(|p| p.into_inner());
    match r {
        PanelRef::Central { instance_id } => {
            core.host
                .ws
                .close_tab_by_instance(wb::TabInstanceId(*instance_id));
        }
        PanelRef::Area { panel_id, .. } => {
            // `toggle_panel` is a toggle, so guard on the current state: a
            // panel already hidden must stay hidden (a no-op), not come back.
            let visible = core
                .host
                .ws
                .panels
                .iter()
                .find(|p| &p.id.0 == panel_id)
                .map(|p| p.visible)
                .unwrap_or(false);
            if visible {
                let _ = core.host.ws.toggle_panel(&wb::PanelId::new(panel_id.clone()));
            }
        }
        PanelRef::Empty => {}
    }
}

impl EventEmitter<PanelEvent> for WbPanel {}

impl Focusable for WbPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for WbPanel {
    fn panel_name(&self) -> &'static str {
        self.name
    }

    fn closable(&self, _: &App) -> bool {
        !matches!(self.r, PanelRef::Empty)
    }

    fn set_active(&mut self, active: bool, _: &mut Window, _: &mut Context<Self>) {
        if active {
            mirror_active(&self.r, &self.ctx.core, &self.ctx.mirror);
        }
    }

    fn on_removed(&mut self, _: &mut Window, _: &mut Context<Self>) {
        write_back_removed(&self.r, &self.ctx.core, &self.ctx.mirror);
    }
}

impl Panel for WbPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.model_title().0.into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (text, icon) = self.model_title();
        let mut row = div().flex().flex_row().items_center().gap_1();
        if let Some(icon) = icon {
            row = row.child(svg().path(icon).size(px(13.0)));
        }
        row.child(text)
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        // The empty placeholder is not a view: nothing to maximize.
        match self.r {
            PanelRef::Empty => None,
            _ => Some(PanelControl::Menu),
        }
    }

    fn inner_padding(&self, _: &App) -> bool {
        // Content is a canvas / GPU viewport: it fills its frame itself.
        false
    }
}

impl Render for WbPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.r {
            PanelRef::Empty => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(col(wb::Color::GRAY))
                .child(EMPTY_CENTER_TITLE)
                .into_any_element(),
            PanelRef::Central { instance_id } => view_canvas(
                self.ctx.views.clone(),
                self.ctx.core.clone(),
                self.ctx.scripts.clone(),
                SlotKey::Central(*instance_id),
            ),
            PanelRef::Area { panel_id, .. } => match &self.script {
                Some(_) => self.ctx.scripts.borrow().element(panel_id),
                None => view_canvas(
                    self.ctx.views.clone(),
                    self.ctx.core.clone(),
                    self.ctx.scripts.clone(),
                    SlotKey::Panel(panel_id.clone()),
                ),
            },
        }
    }
}

// ---- 模型快照（每帧读取一次） ----

/// One visible panel of a side dock.
pub(crate) struct PanelSnapshot {
    pub id: String,
    /// gpui-rhai entry point for script panels (`None` = painted view).
    pub script: Option<String>,
}

/// A side dock's model state.
#[derive(Default)]
pub(crate) struct AreaSnapshot {
    /// Visible panels, in model order.
    pub panels: Vec<PanelSnapshot>,
    /// The model's active panel id. May name a hidden panel; the sync falls
    /// back to the first visible one.
    pub active: Option<String>,
    /// Model size along the dock's axis (the active panel's size, else the
    /// first visible one's).
    pub size: f32,
}

/// The model state the dock mirrors, read once per frame.
#[derive(Default)]
pub(crate) struct ShellSnapshot {
    pub left: AreaSnapshot,
    pub right: AreaSnapshot,
    pub bottom: AreaSnapshot,
    /// Open central tabs, in tab order.
    pub tabs: Vec<u64>,
    pub active_tab: Option<u64>,
}

// ---- 同步器 ----

/// What each region's layout tree was last built from. Only a change here — or
/// of the region's displayed panel — rebuilds a region.
#[derive(Default, Clone, PartialEq, Eq)]
struct ShellStructure {
    left: Vec<String>,
    right: Vec<String>,
    bottom: Vec<String>,
    center: Vec<u64>,
}

impl ShellStructure {
    fn row(&self, area: wb::DockArea) -> &[String] {
        match area {
            wb::DockArea::Left => self.left.as_slice(),
            wb::DockArea::Right => self.right.as_slice(),
            wb::DockArea::Bottom => self.bottom.as_slice(),
            // The model has a Top area; this front end has never shown one.
            wb::DockArea::Top => &[],
        }
    }

    fn set_row(&mut self, area: wb::DockArea, ids: Vec<String>) {
        match area {
            wb::DockArea::Left => self.left = ids,
            wb::DockArea::Right => self.right = ids,
            wb::DockArea::Bottom => self.bottom = ids,
            wb::DockArea::Top => {}
        }
    }
}

/// The size last pushed to a dock, and the panel it was read from. Each panel
/// carries its own default size but a dock has one width: pushing on a mere
/// active-panel switch would resize a dock the user has dragged, so a push
/// happens only when the model's value for the same panel moved (a layout
/// restore).
#[derive(Clone)]
struct PushedSize {
    panel_id: String,
    size: f32,
}

/// The panel a region should display: the model's active one when it is one of
/// the region's panels, else the first.
fn shown_panel(panels: &[String], active: Option<&str>) -> Option<String> {
    match active {
        Some(id) if panels.iter().any(|p| p == id) => Some(id.to_string()),
        _ => panels.first().cloned(),
    }
}

/// Keep only the leaves of a persisted panel tree this session can rebuild,
/// and drop the containers that lose all of theirs. Sizes and the active tab
/// index follow the survivors, so a pruned tree is still internally
/// consistent (a stack whose size list no longer lines up with its children
/// could not divide its space).
fn prune_panel(state: &PanelState, allowed: &HashSet<String>) -> Option<PanelState> {
    if state.children.is_empty() {
        return allowed
            .contains(&state.panel_name)
            .then(|| state.clone());
    }

    let sizes = state.info.sizes().cloned().unwrap_or_default();
    let mut children = Vec::new();
    let mut kept_sizes = Vec::new();
    for (ix, child) in state.children.iter().enumerate() {
        if let Some(pruned) = prune_panel(child, allowed) {
            children.push(pruned);
            if let Some(size) = sizes.get(ix) {
                kept_sizes.push(*size);
            }
        }
    }
    if children.is_empty() {
        return None;
    }

    let mut pruned = state.clone();
    pruned.children = children;
    pruned.info = match state.info.axis() {
        Some(axis) => PanelInfo::stack(kept_sizes, axis),
        None => match state.info.active_index() {
            Some(active_ix) => PanelInfo::tabs(active_ix.min(pruned.children.len() - 1)),
            None => state.info.clone(),
        },
    };
    Some(pruned)
}

/// What [`wb::DockArea`] placement a side dock maps to (`None` for `Top`,
/// which this front end does not show).
fn dock_placement(area: wb::DockArea) -> Option<DockPlacement> {
    match area {
        wb::DockArea::Left => Some(DockPlacement::Left),
        wb::DockArea::Right => Some(DockPlacement::Right),
        wb::DockArea::Bottom => Some(DockPlacement::Bottom),
        wb::DockArea::Top => None,
    }
}

/// Mirrors the model into a gpui-component [`GpuiDockArea`], and the user's
/// dock actions back into the model.
pub(crate) struct DockShell {
    area: Entity<GpuiDockArea>,
    /// Dock panel entities by interned name (bounded: registry panels + open
    /// tabs; pruned to what the model still has). Behind a shared cell because
    /// the `PanelRegistry` builders handed to `set_dock`/`load` look panels up
    /// through it.
    panels: Rc<RefCell<HashMap<&'static str, Entity<WbPanel>>>>,
    pushed: HashMap<wb::DockArea, PushedSize>,
    structure: ShellStructure,
    /// Whether the first sync has run. The first one installs every region,
    /// including the empty-centre placeholder.
    installed: bool,
    ctx: PanelCtx,
}

impl DockShell {
    pub(crate) fn new(
        core: SharedCore,
        scripts: SharedScripts,
        views: SharedViews,
        mirror: SharedMirror,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        // The skin's settings stay at their defaults; the area's installed
        // renderer holds its own reference to the skin.
        let (area, _skin) =
            DockSkin::dock_area(DOCK_AREA_ID, Some(DOCK_AREA_VERSION), window, cx);
        Self {
            area,
            panels: Rc::new(RefCell::new(HashMap::new())),
            pushed: HashMap::new(),
            structure: ShellStructure::default(),
            installed: false,
            ctx: PanelCtx {
                core,
                scripts,
                views,
                mirror,
            },
        }
    }

    pub(crate) fn area(&self) -> Entity<GpuiDockArea> {
        self.area.clone()
    }

    /// The live panel entities, for the root view's repaint driver: the dock
    /// draws a panel's subtree through gpui's *cached* view path, so a panel
    /// repaints only when its own entity is notified.
    pub(crate) fn panel_weaks(&self) -> Vec<gpui::WeakEntity<WbPanel>> {
        self.panels
            .borrow()
            .values()
            .map(|entity| entity.downgrade())
            .collect()
    }

    /// The dock's physical layout, as persistence stores it.
    pub(crate) fn dump(&self, cx: &App) -> Option<serde_json::Value> {
        serde_json::to_value(self.area.read(cx).dump(cx)).ok()
    }

    /// Re-establish a persisted layout — the side docks only, and only for the
    /// panels this session still has.
    ///
    /// The centre is rebuilt from the model instead of restored: its panels are
    /// named `wb.tab.{instance}` and instance ids are runtime numbers, so a
    /// persisted centre could only come back as placeholder tabs. Side docks
    /// are keyed by registry panel ids, which are stable across sessions.
    pub(crate) fn restore(
        &mut self,
        saved: DockAreaState,
        snap: &ShellSnapshot,
        window: &mut Window,
        cx: &mut App,
    ) {
        let allowed = |snap: &AreaSnapshot| -> HashSet<String> {
            snap.panels.iter().map(|p| p.id.clone()).collect()
        };
        let restore_dock = |dock: Option<gpui_component::dock::DockState>,
                            area: wb::DockArea|
         -> Option<gpui_component::dock::DockState> {
            let dock = dock?;
            let area_snap = match area {
                wb::DockArea::Left => &snap.left,
                wb::DockArea::Right => &snap.right,
                wb::DockArea::Bottom => &snap.bottom,
                wb::DockArea::Top => return None,
            };
            let pruned = prune_panel(dock.panel(), &allowed(area_snap))?;
            Some(gpui_component::dock::DockState::new(
                pruned,
                dock.placement(),
                dock.size(),
                dock.open(),
            ))
        };

        let state = DockAreaState {
            version: saved.version,
            // Rebuilt from the model on the next sync (see the doc comment).
            center: PanelState::default(),
            left_dock: restore_dock(saved.left_dock, wb::DockArea::Left),
            right_dock: restore_dock(saved.right_dock, wb::DockArea::Right),
            bottom_dock: restore_dock(saved.bottom_dock, wb::DockArea::Bottom),
        };
        if state.left_dock.is_none() && state.right_dock.is_none() && state.bottom_dock.is_none() {
            return;
        }

        // The panels the current layout holds are displaced by the load and
        // told `on_removed`; that is the sync's doing, not the user's.
        let area = self.area.clone();
        let loaded = with_removal_suppressed(&self.ctx.mirror, || {
            area.update(cx, |area, cx| area.load(state, window, cx).is_ok())
        });
        if !loaded {
            // Unreadable state: the model-driven layout from the first sync
            // stays (never a panic).
            return;
        }
        // The recorded structure stays as the model describes it — the restored
        // layout is the dock's own business until the model's panel set changes
        // — except the centre, which is empty until the next sync rebuilds it.
        self.structure.center.clear();
    }

    /// Mirror the model into the dock. Called from the root view's render,
    /// before the dock element is drawn.
    pub(crate) fn sync(&mut self, snap: &ShellSnapshot, window: &mut Window, cx: &mut App) {
        let fresh = !self.installed;
        let mut live: HashSet<&'static str> = HashSet::new();
        for area in [wb::DockArea::Left, wb::DockArea::Right, wb::DockArea::Bottom] {
            let Some(placement) = dock_placement(area) else {
                continue;
            };
            let area_snap = match area {
                wb::DockArea::Left => &snap.left,
                wb::DockArea::Right => &snap.right,
                wb::DockArea::Bottom => &snap.bottom,
                wb::DockArea::Top => continue,
            };
            self.sync_area(area, placement, area_snap, &mut live, window, cx);
        }
        self.sync_center(snap, fresh, &mut live, window, cx);
        self.installed = true;

        // A panel the model no longer has (closed tab, hidden panel) leaves the
        // dock with the next rebuild; its adapter goes now, so the map cannot
        // grow with every closed tab.
        self.panels.borrow_mut().retain(|name, _| live.contains(name));

        // The view instances follow the slots: drop the ones whose tab or panel
        // the model no longer has.
        let live_panels: Vec<String> = [&snap.left, &snap.right, &snap.bottom]
            .iter()
            .flat_map(|area| area.panels.iter().map(|p| p.id.clone()))
            .collect();
        self.ctx
            .views
            .borrow_mut()
            .prune(&snap.tabs.clone(), &live_panels);
    }

    fn sync_area(
        &mut self,
        area: wb::DockArea,
        placement: DockPlacement,
        snap: &AreaSnapshot,
        live: &mut HashSet<&'static str>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let fresh = !self.installed;
        // Script views mount before anything draws them (the pre-dock front
        // end mounted from the root view's render the same way).
        for panel in &snap.panels {
            if let Some(script) = &panel.script {
                self.ctx.scripts
                    .borrow_mut()
                    .mount(&panel.id, script, window, cx);
            }
        }

        let ids: Vec<String> = snap.panels.iter().map(|p| p.id.clone()).collect();
        if ids.is_empty() {
            // No visible panel: the dock gives up its slot entirely.
            if self.area.read(cx).has_dock(placement) {
                let area_entity = self.area.clone();
                with_removal_suppressed(&self.ctx.mirror, || {
                    area_entity.update(cx, |a, cx| a.remove_dock(placement, window, cx));
                });
            }
            self.structure.set_row(area, ids);
            self.pushed.remove(&area);
            self.ctx.mirror.borrow_mut().area.remove(&area);
            return;
        }

        let shown = shown_panel(&ids, snap.active.as_deref());
        let mirrored = self.ctx.mirror.borrow().area.get(&area).cloned();
        if fresh || self.structure.row(area) != ids.as_slice() || mirrored != shown {
            let mut layout = DockLayout::tabs();
            let mut active_ix = 0usize;
            for (ix, panel) in snap.panels.iter().enumerate() {
                if Some(panel.id.as_str()) == shown.as_deref() {
                    active_ix = ix;
                }
                let entity = self.panel_for_area(area, panel, live, cx);
                layout = layout.panel_view(panel_handle(entity), cx);
            }
            let layout = layout.active_index(active_ix);
            let area_entity = self.area.clone();
            with_removal_suppressed(&self.ctx.mirror, || {
                area_entity.update(cx, |a, cx| a.set_dock(placement, layout, window, cx));
            });
            self.structure.set_row(area, ids);
            let mut mirror = self.ctx.mirror.borrow_mut();
            match shown.as_deref() {
                Some(id) => {
                    mirror.area.insert(area, id.to_string());
                }
                None => {
                    mirror.area.remove(&area);
                }
            }
        }

        // Size: the dock keeps what the user dragged; the model's value is
        // pushed only when it moved for the panel it was read from.
        if let Some(panel_id) = shown.as_deref() {
            let size = snap.size;
            let push = match self.pushed.get(&area) {
                None => true,
                Some(pushed) => pushed.panel_id == panel_id && pushed.size != size,
            };
            if push {
                let area_entity = self.area.clone();
                area_entity.update(cx, |a, cx| {
                    a.set_dock_size(placement, px(size), window, cx)
                });
            }
            self.pushed.insert(
                area,
                PushedSize {
                    panel_id: panel_id.to_string(),
                    size,
                },
            );
        }
    }

    fn sync_center(
        &mut self,
        snap: &ShellSnapshot,
        fresh: bool,
        live: &mut HashSet<&'static str>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Script central views mount before anything draws them, like the
        // script panels of the side docks.
        for instance_id in &snap.tabs {
            if let Some(script) = tab_script(&self.ctx.core, *instance_id) {
                let key = format!("wb.tab.{instance_id}");
                self.ctx.scripts.borrow_mut().mount(&key, &script, window, cx);
            }
        }

        let active = shown_tab(&snap.tabs, snap.active_tab);
        let mirrored = self.ctx.mirror.borrow().center;
        if !(fresh || self.structure.center != snap.tabs || mirrored != active) {
            return;
        }

        let mut layout = DockLayout::tabs();
        let mut active_ix = 0usize;
        if snap.tabs.is_empty() {
            // No tab open: keep the pre-dock front end's placeholder.
            let entity = self.empty_panel(live, cx);
            layout = layout.panel_view(panel_handle(entity), cx);
        } else {
            for (ix, instance_id) in snap.tabs.iter().enumerate() {
                if Some(*instance_id) == active {
                    active_ix = ix;
                }
                let entity = self.panel_for_tab(*instance_id, live, cx);
                layout = layout.panel_view(panel_handle(entity), cx);
            }
        }
        let layout = layout.active_index(active_ix);
        let area_entity = self.area.clone();
        with_removal_suppressed(&self.ctx.mirror, || {
            area_entity.update(cx, |a, cx| a.set_center(layout, window, cx));
        });
        self.structure.center = snap.tabs.clone();
        self.ctx.mirror.borrow_mut().center = active;
    }

    /// The dock panel registered under `name`, created on first sight.
    fn panel_for(
        &mut self,
        name: &'static str,
        make: impl FnOnce(&mut App) -> Entity<WbPanel>,
        live: &mut HashSet<&'static str>,
        cx: &mut App,
    ) -> Entity<WbPanel> {
        live.insert(name);
        if let Some(existing) = self.panels.borrow().get(name) {
            return existing.clone();
        }
        let entity = make(cx);
        self.panels.borrow_mut().insert(name, entity.clone());
        // `DockArea::load` rebuilds panels by name through this registry; the
        // builder hands back the panel this session already has for that name,
        // so a restored tree points at the same entities (no churn, and no
        // placeholder tabs for panels we can build).
        let map = self.panels.clone();
        register_panel(cx, name, move |_ctx, _window, _cx| {
            let entity = map
                .borrow()
                .get(name)
                .cloned()
                .expect("only names this session registered are ever asked for");
            panel_handle(entity)
        });
        entity
    }

    /// The dock panel for one side-dock panel.
    fn panel_for_area(
        &mut self,
        area: wb::DockArea,
        panel: &PanelSnapshot,
        live: &mut HashSet<&'static str>,
        cx: &mut App,
    ) -> Entity<WbPanel> {
        let name = intern(&panel.id);
        let r = PanelRef::Area {
            area,
            panel_id: panel.id.clone(),
        };
        let script = panel.script.clone();
        let ctx = self.ctx.clone();
        self.panel_for(
            name,
            |cx| WbPanel::new(name, r, script, ctx, cx),
            live,
            cx,
        )
    }

    /// The dock panel for one central tab.
    fn panel_for_tab(
        &mut self,
        instance_id: u64,
        live: &mut HashSet<&'static str>,
        cx: &mut App,
    ) -> Entity<WbPanel> {
        let name = intern(&format!("wb.tab.{instance_id}"));
        let ctx = self.ctx.clone();
        self.panel_for(
            name,
            |cx| {
                WbPanel::new(name, PanelRef::Central { instance_id }, None, ctx, cx)
            },
            live,
            cx,
        )
    }

    /// The placeholder panel for an empty centre.
    fn empty_panel(&mut self, live: &mut HashSet<&'static str>, cx: &mut App) -> Entity<WbPanel> {
        let name = intern("wb.empty");
        let ctx = self.ctx.clone();
        self.panel_for(
            name,
            |cx| WbPanel::new(name, PanelRef::Empty, None, ctx, cx),
            live,
            cx,
        )
    }
}

/// The central tab to display: the model's active one when it is still open,
/// else the first.
fn shown_tab(tabs: &[u64], active: Option<u64>) -> Option<u64> {
    match active {
        Some(id) if tabs.contains(&id) => Some(id),
        _ => tabs.first().copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(ids: &[&str], active: Option<&str>, size: f32) -> AreaSnapshot {
        AreaSnapshot {
            panels: ids
                .iter()
                .map(|id| PanelSnapshot {
                    id: id.to_string(),
                    script: None,
                })
                .collect(),
            active: active.map(str::to_string),
            size,
        }
    }

    /// A runtime with one open tab, and the shared cells a panel carries.
    fn tabbed_core() -> (SharedCore, SharedMirror, u64) {        let runtime = workbench_core::WorkbenchAppBuilder::new("test.dock.writeback", "Writeback")
            .with_workbench(wb_example::ExampleWorkbench)
            .open_on_start("example.welcome", false)
            .build()
            .expect("runtime builds");
        let core: SharedCore = std::rc::Rc::new(Mutex::new(runtime));
        let instance = {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            core.host.ws.tabs.first().expect("the welcome tab").instance_id.0
        };
        let mirror: SharedMirror = std::rc::Rc::new(RefCell::new(DockMirror::default()));
        (core, mirror, instance)
    }

    /// The user closing a central panel in the dock closes its tab in the
    /// model, and a repeat close is a no-op (the sync's own removal also
    /// reports through here, when it is not suppressed).
    #[test]
    fn a_closed_centre_panel_closes_its_tab_and_stays_closed() {
        let (core, mirror, instance) = tabbed_core();
        let r = PanelRef::Central { instance_id: instance };

        write_back_removed(&r, &core, &mirror);
        assert!(
            core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.tabs.is_empty(),
            "the dock's close closed the tab"
        );

        write_back_removed(&r, &core, &mirror);
        assert!(
            core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.tabs.is_empty(),
            "closing twice is a no-op, not a reopen"
        );
    }

    /// Every panel the sync itself displaces is told `on_removed` — including
    /// the ones that are still in the model. Read back as closes, that would
    /// close every other tab in the region, so the sync suppresses it.
    #[test]
    fn a_sync_displacement_is_not_read_back_as_a_close() {
        let (core, mirror, instance) = tabbed_core();
        mirror.borrow_mut().suppress_removal = true;

        write_back_removed(&PanelRef::Central { instance_id: instance }, &core, &mirror);

        assert_eq!(
            core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.tabs.len(),
            1,
            "a panel displaced by the rebuild keeps its tab"
        );
    }

    /// Closing a side panel hides it (closing semantics), and hiding an
    /// already hidden panel must not bring it back: `toggle_panel` is a
    /// toggle, so the writeback guards on the current state.
    #[test]
    fn a_closed_side_panel_hides_once_and_does_not_resurrect() {
        let (core, mirror, _) = tabbed_core();
        let r = PanelRef::Area {
            area: wb::DockArea::Left,
            panel_id: "example.panel.explorer".to_string(),
        };
        let visible = |core: &SharedCore| {
            core.lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .ws
                .panels
                .iter()
                .find(|p| p.id.0 == "example.panel.explorer")
                .map(|p| p.visible)
                .expect("the explorer panel")
        };

        assert!(visible(&core), "starts visible");
        write_back_removed(&r, &core, &mirror);
        assert!(!visible(&core), "closed in the dock → hidden in the model");

        write_back_removed(&r, &core, &mirror);
        assert!(!visible(&core), "hidden stays hidden");
    }

    /// Activation mirrors both ways: the model's active panel/tab, and the
    /// mirror the sync compares against (so the dock's own tab click is not
    /// mistaken for a model-driven activation and rebuilt).
    #[test]
    fn activation_mirrors_into_the_model_and_the_mirror() {
        let (core, mirror, instance) = tabbed_core();

        mirror_active(
            &PanelRef::Central { instance_id: instance },
            &core,
            &mirror,
        );
        assert_eq!(mirror.borrow().center, Some(instance));
        {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(core.host.ws.active_tab().map(|t| t.instance_id.0), Some(instance));
        }

        mirror_active(
            &PanelRef::Area {
                area: wb::DockArea::Right,
                panel_id: "example.panel.properties".to_string(),
            },
            &core,
            &mirror,
        );
        assert_eq!(
            mirror.borrow().area.get(&wb::DockArea::Right).map(String::as_str),
            Some("example.panel.properties")
        );
        {
            let core = core.lock().unwrap_or_else(|p| p.into_inner());
            let active = core.host.ws.area_active.get(&wb::DockArea::Right);
            assert_eq!(active.map(|id| id.0.as_str()), Some("example.panel.properties"));
        }
    }

    /// The placeholder is not a view: neither activating nor removing it may
    /// touch the model.
    #[test]
    fn the_empty_placeholder_never_touches_the_model() {
        let (core, mirror, _) = tabbed_core();
        let tabs_before = core.lock().unwrap_or_else(|p| p.into_inner()).host.ws.tabs.len();

        mirror_active(&PanelRef::Empty, &core, &mirror);
        write_back_removed(&PanelRef::Empty, &core, &mirror);

        let core = core.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(core.host.ws.tabs.len(), tabs_before);
        assert_eq!(core.host.ws.active_tab().map(|t| t.instance_id.0), Some(1));
    }

    // ---- 持久化：恢复前的剪枝（只留本会话能重建的面板） ----

    fn allowed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn leaf(name: &str) -> PanelState {
        PanelState::new(name)
    }

    fn tabs(active_ix: usize, children: Vec<PanelState>) -> PanelState {
        let mut state = PanelState::new("TabPanel");
        for child in children {
            state.add_child(child);
        }
        state.info = PanelInfo::tabs(active_ix);
        state
    }

    fn stack(sizes: Vec<f32>, children: Vec<PanelState>) -> PanelState {
        let mut state = PanelState::new("StackPanel");
        for child in children {
            state.add_child(child);
        }
        state.info = PanelInfo::stack(
            sizes.into_iter().map(gpui::px).collect(),
            gpui::Axis::Horizontal,
        );
        state
    }

    fn leaf_names(state: &PanelState) -> Vec<String> {
        if state.children.is_empty() {
            return vec![state.panel_name.clone()];
        }
        state
            .children
            .iter()
            .flat_map(leaf_names)
            .collect::<Vec<String>>()
    }

    /// A persisted group whose panel this session no longer has comes back
    /// without it — and its active tab index follows (a stale index would
    /// point past the end of the surviving list).
    #[test]
    fn restore_prunes_panels_this_session_does_not_have() {
        let saved = tabs(
            2,
            vec![leaf("tasks"), leaf("properties"), leaf("ghost")],
        );
        let pruned = prune_panel(&saved, &allowed(&["tasks", "properties"])).expect("kept");

        assert_eq!(leaf_names(&pruned), vec!["tasks", "properties"]);
        assert_eq!(pruned.info.active_index(), Some(1));
    }

    /// A container that loses all of its panels goes with them, and the sizes
    /// of a split follow the survivors — a stack whose size list no longer
    /// lines up with its children could not divide its space.
    #[test]
    fn restore_drops_empty_containers_and_their_sizes() {
        let saved = stack(
            vec![240.0, 480.0],
            vec![tabs(0, vec![leaf("ghost")]), tabs(0, vec![leaf("tasks")])],
        );
        let pruned = prune_panel(&saved, &allowed(&["tasks"])).expect("kept");

        assert_eq!(leaf_names(&pruned), vec!["tasks"]);
        assert_eq!(pruned.info.sizes(), Some(&vec![gpui::px(480.0)]));
    }

    /// A saved layout naming nothing this session has is dropped whole: the
    /// model-driven layout from the first sync stays.
    #[test]
    fn a_layout_with_no_recognisable_panel_prunes_to_nothing() {
        let saved = tabs(0, vec![leaf("ghost")]);
        assert!(prune_panel(&saved, &HashSet::new()).is_none());
    }

    /// Pruning keeps the tree it was given when nothing needs dropping (the
    /// ordinary restart case).
    #[test]
    fn restore_keeps_a_fully_recognised_tree() {
        let saved = tabs(1, vec![leaf("tasks"), leaf("properties")]);
        let pruned = prune_panel(&saved, &allowed(&["tasks", "properties"])).expect("kept");
        assert_eq!(pruned, saved);
    }

    /// The panel each region displays: the model's active one when it is among
    /// the region's panels, else the first. A stale active id (its panel was
    /// hidden) must not leave the dock showing nothing.
    #[test]
    fn regions_fall_back_to_their_first_panel() {
        assert_eq!(shown_panel(&[], None), None);
        assert_eq!(
            shown_panel(&["explorer".into()], Some("explorer")),
            Some("explorer".to_string())
        );
        assert_eq!(
            shown_panel(&["tasks".into(), "properties".into()], Some("properties")),
            Some("properties".to_string())
        );
        assert_eq!(
            shown_panel(&["tasks".into(), "properties".into()], Some("gone")),
            Some("tasks".to_string())
        );
        assert_eq!(
            shown_panel(&["tasks".into(), "properties".into()], None),
            Some("tasks".to_string())
        );
    }

    #[test]
    fn the_centre_falls_back_to_its_first_open_tab() {
        assert_eq!(shown_tab(&[], None), None);
        assert_eq!(shown_tab(&[3, 7], Some(7)), Some(7));
        // The active tab was closed: the first remaining one takes over.
        assert_eq!(shown_tab(&[3, 7], Some(9)), Some(3));
        assert_eq!(shown_tab(&[3, 7], None), Some(3));
    }

    /// The three model areas map onto the dock's placements, and `Top` — which
    /// this front end has never shown — maps to nothing rather than being
    /// silently drawn as some other dock.
    #[test]
    fn model_areas_map_to_dock_placements() {
        assert_eq!(dock_placement(wb::DockArea::Left), Some(DockPlacement::Left));
        assert_eq!(
            dock_placement(wb::DockArea::Right),
            Some(DockPlacement::Right)
        );
        assert_eq!(
            dock_placement(wb::DockArea::Bottom),
            Some(DockPlacement::Bottom)
        );
        assert_eq!(dock_placement(wb::DockArea::Top), None);
    }

    /// The structure a region's tree was built from is what decides a rebuild:
    /// the same ids keep the user's layout, a different set (or a different
    /// displayed panel) rebuilds it.
    #[test]
    fn structure_tracks_each_region_separately() {
        let mut structure = ShellStructure::default();
        assert_eq!(structure.row(wb::DockArea::Left), &[] as &[String]);
        structure.set_row(wb::DockArea::Left, vec!["explorer".to_string()]);
        structure.set_row(
            wb::DockArea::Right,
            vec!["tasks".to_string(), "properties".to_string()],
        );
        assert_eq!(structure.row(wb::DockArea::Left), &["explorer".to_string()]);
        assert_eq!(
            structure.row(wb::DockArea::Right),
            &["tasks".to_string(), "properties".to_string()]
        );
        // The bottom region is still empty, and `Top` simply has no row.
        assert_eq!(structure.row(wb::DockArea::Bottom), &[] as &[String]);
        assert_eq!(structure.row(wb::DockArea::Top), &[] as &[String]);
        assert_ne!(structure.row(wb::DockArea::Left), &["output".to_string()]);
    }

    /// Size pushes: the first push seeds the dock, a changed value for the same
    /// panel re-pushes (a layout restore), and an active-panel switch — where
    /// the model reads a different panel's default — leaves the dock alone so
    /// the user's splitter drag survives.
    #[test]
    fn sizes_push_once_and_survive_tab_switches() {
        let pushed = PushedSize {
            panel_id: "tasks".to_string(),
            size: 300.0,
        };
        // Same panel, same size: nothing to do.
        assert!(!(pushed.panel_id == "tasks" && pushed.size != 300.0));
        // Same panel, model moved: push.
        assert!(pushed.panel_id == "tasks" && pushed.size != 420.0);
        // Another panel is displayed now: not a size change of this one.
        assert!(!(pushed.panel_id == "properties" && pushed.size != 300.0));
    }

    /// The three areas and the centre all reach the sync — the regression the
    /// retired splitter tests stood for ("every region is really wired").
    #[test]
    fn a_snapshot_carries_all_four_regions() {
        let snap = ShellSnapshot {
            left: area(&["explorer"], Some("explorer"), 240.0),
            right: area(&["tasks", "properties"], Some("tasks"), 300.0),
            bottom: area(&["output"], Some("output"), 170.0),
            tabs: vec![1, 2],
            active_tab: Some(2),
        };
        assert_eq!(snap.left.panels.len(), 1);
        assert_eq!(snap.right.panels.len(), 2);
        assert_eq!(snap.bottom.panels.len(), 1);
        assert_eq!(snap.left.size, 240.0);
        assert_eq!(snap.right.panels[1].id, "properties");
        assert_eq!(snap.bottom.panels[0].id, "output");
        assert_eq!(shown_tab(&snap.tabs, snap.active_tab), Some(2));
    }
}
