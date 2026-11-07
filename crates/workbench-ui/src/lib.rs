//! gpui frontend for workbench-rs: renders the platform workspace model into
//! a gpui window (gpui-fast).
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
    div, prelude::*, px, relative, size, AnyElement, App, Bounds, Context, Div, Hsla, KeyDownEvent,
    Pixels, Render, Rgba, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use workbench_api as wb;
use workbench_core::AppRuntime;

pub mod gpu_view;

/// gpui-component's default icons (embedded on the web too).
mod component_assets;

/// About dialog overlay (product brand: logo / name / version / links).
pub(crate) mod about;

/// In-app file picker overlay (open/save for browser storage, cloud stores).
mod file_picker;
/// Settings text-edit overlay (gpui-component Input hosted at root level).
pub(crate) mod text_edit;

/// Built-in platform panels (diagnostics / plugins).
pub mod platform_panels;

pub(crate) mod dock_panel;

/// Ribbon mount bridge (the only coupling point with the ribbon crate, ribbon-bar.md §5.1).
pub(crate) mod ribbon_bridge;

/// UI theme (dark/light): shell colors + applying the theme to the whole window.
pub mod theme;

/// Platform runtime handle: every use is on the UI main thread (gpui's foreground executor),
/// hence `Rc`.
type SharedCore = std::rc::Rc<Mutex<AppRuntime>>;

/// Base Latin font (IBM Plex Sans, OFL licensed, committed together with license.txt).
/// gpui_web ships no fonts, so the web must inject them explicitly before the first frame.
#[cfg(target_arch = "wasm32")]
const LATIN_FONT: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");

/// Combined asset source: product assets first, gpui-component's built-in icon assets as the
/// fallback. (Component icons are compiled into the binary on both desktop and web; see
/// component_assets)
struct WbAssets {
    app: Arc<dyn wb::AssetSource>,
    component: component_assets::ComponentAssets,
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
        // gpui's AssetRegistry builds its table by enumerating list("")
        // (AssetSource::iter_ondemand): only the paths reported here get registered, and loading
        // an unregistered path always misses (the icon silently doesn't render). So the root
        // listing must be the union of the product/platform standard paths and the component's
        // built-in paths; a path present in both (e.g. icons/delete.svg) is deduplicated, since a
        // duplicate registry insert is an error.
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

// ---- Color conversion ----

pub fn col(c: wb::Color) -> Hsla {
    Hsla::from(Rgba { r: c.r, g: c.g, b: c.b, a: c.a })
}

#[derive(Clone, Debug)]
enum SlotKey {
    Central(u64),
    Panel(String),
}

// ---- Root view ----

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
    /// Ribbon mount point (a ribbon crate entity; built lazily on the first frame, like the dock).
    ribbon: ribbon_bridge::RibbonHost,
    /// Last-applied `ui.theme` / `ui.accent` settings: the render loop
    /// re-applies the theme when either diverges from what is on screen
    /// (settings panel writes land live).
    applied_theme: String,
    applied_accent: String,
    /// Settings text edit (Phase 1.5): the Input entity for the open edit,
    /// its Enter subscription, and the key it was mounted for.
    edit_input: Option<gpui::Entity<gpui_component::input::InputState>>,
    edit_subscription: Option<gpui::Subscription>,
    edit_key: Option<String>,
    /// The in-app file picker is drawn; a save picker's file-name input and
    /// its Enter subscription.
    picker_mounted: bool,
    picker_input: Option<(gpui::Entity<gpui_component::input::InputState>, gpui::Subscription)>,
}

impl RootView {
    fn new(core: SharedCore, pending_dock: Option<gpui_component::dock::DockAreaState>) -> Self {
        publish_runtime_dispatcher(&core);
        // Seed the applied-theme trackers from the current settings so the
        // first frame does not re-apply what launch_app already applied.
        let (applied_theme, applied_accent) = {
            let guard = core.lock().unwrap_or_else(|p| p.into_inner());
            (
                guard.host.settings.get_str("ui.theme", "dark"),
                guard.host.settings.get_str("ui.accent", "#338CF2"),
            )
        };
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
            applied_theme,
            applied_accent,
            edit_input: None,
            edit_subscription: None,
            edit_key: None,
            picker_mounted: false,
            picker_input: None,
        }
    }
}

/// Request a repaint: the root view + every dock panel.
///
/// The frontend's command dispatcher for view element handlers (they run at
/// event time, after the render that built them). It lives as long as the
/// runtime. `try_lock`: a dispatch issued while the runtime is locked — from
/// inside a view's `element` — fails instead of deadlocking.
fn publish_runtime_dispatcher(core: &SharedCore) {
    let core = core.clone();
    wb::publish_dispatcher(std::rc::Rc::new(move |command, args| {
        match core.try_lock() {
            Ok(mut guard) => guard.execute_command(command, &args),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                poisoned.into_inner().execute_command(command, &args)
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                wb::CommandResult::failed("Commands cannot run while a view is being built")
            }
        }
    }));
}

/// Save the workspace layout: the core's model plus the dock area's physical
/// layout. Returns whether anything was written (an unchanged layout isn't);
/// a failure (read-only directory, full browser storage) is logged.
fn save_layout(core: &SharedCore, root: &gpui::Entity<RootView>, cx: &App) -> bool {
    let dock = root.read(cx).dock.as_ref().and_then(|shell| shell.dump(cx));
    let mut core = core.lock().unwrap_or_else(|p| p.into_inner());
    match core.save_layout(serde_json::Value::Null, dock) {
        Ok(written) => written,
        Err(e) => {
            core.host.log.warn(format!("Failed to save the layout: {e}"));
            false
        }
    }
}

/// Panels are now independent entities, and the dock draws panel content through gpui's
/// **cached view** path (a panel entity's subtree re-renders only when that entity is notified).
/// The old layout drew the panels inside the root view's subtree, so notifying the root view was
/// enough; this mirroring keeps the "any input drives a repaint of every viewport" semantics.
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

// ---- Panel / central view drawing ----

/// Placeholder background while a GPU viewport isn't ready / has failed (dark, matching the
/// viewport background).
pub(crate) fn paint_placeholder(window: &mut Window, bounds: Bounds<Pixels>) {
    window.paint_quad(gpui::fill(bounds, col(theme::ui_theme().placeholder)));
}

/// A modal overlay (About, text edit), drawn deferred: on GPUI's overlay
/// layer, above everything else — including native viewport surfaces, which
/// window composition stacks between the base content and the overlays.
fn modal_overlay(overlay: impl IntoElement) -> gpui::Deferred {
    gpui::deferred(overlay).with_priority(1)
}

/// Draws a panel / central view into a canvas.
/// The instances live in the core state and drawing needs &mut core: swap out, draw, put back.
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
) -> (AnyElement, bool) {
    // A script central tab has no Rust instance: the frontend mounts the script
    // view and draws it here, keyed by the tab instance (`wb.tab.{id}`).
    if let SlotKey::Central(id) = &key {
        if tab_script(&core, *id).is_some() {
            return (scripts.borrow().element(&format!("wb.tab.{id}")), false);
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
        return (slot_placeholder(), false);
    };

    // The view reads the host while building its element tree, so the model
    // lock is held for the duration of `element`; the published dispatcher lets
    // its event handlers reach the same runtime. Published per frame and kept
    // until the next frame's publish (round-191: clearing it here made every
    // view-content button dead — gpui fires click handlers between frames,
    // after the clear, so wb::dispatch silently failed with
    // "The view element is not in a render context"). The handle points at the same live runtime
    // the view was built from; a new frame always replaces it before any
    // further handler runs.
    let (element, wants_repaint) = {
        let Ok(mut guard) = core.lock() else {
            return (slot_placeholder(), false);
        };
        let mut view = instance.borrow_mut();
        let built = view.element(&mut *guard as &mut dyn wb::AppServices);
        // Read after `element`: it is what clears the flag (adopting a landed
        // result) or sets it (spawning a task).
        let wants = view.wants_repaint();
        (built, wants)
    };

    let element = match element {
        Some(handle) => match handle.downcast::<AnyElement>() {
            Ok(element) => *element,
            Err(_) => slot_placeholder(),
        },
        None => slot_placeholder(),
    };
    (element, wants_repaint)
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Frame stats are measured only here: they describe the render cadence. The platform tick
        // (task pump / autosave / context snapshot) is driven by `launch_app`'s timer, decoupled
        // from rendering: gpui only redraws on invalidation, so a render-driven pump stalls while
        // the UI is idle and task results sit unclaimed in their channel.
        {
            let mut core = self.core.lock().unwrap_or_else(|p| p.into_inner());
            let now = web_time::Instant::now();
            let dt_ms = (now - self.last_render).as_secs_f32() * 1000.0;
            self.last_render = now;
            core.record_frame(dt_ms);
        }
        let weak = cx.entity().downgrade();
        let core = self.core.clone();

        // ---- Theme/accent live sync: the settings panel writes through
        // `app.settings.set`; re-apply whichever surface diverged. ----
        {
            let (theme_now, accent_now) = {
                let guard = self.core.lock().unwrap_or_else(|p| p.into_inner());
                (
                    guard.host.settings.get_str("ui.theme", "dark"),
                    guard.host.settings.get_str("ui.accent", "#338CF2"),
                )
            };
            if theme_now != self.applied_theme {
                theme::apply_theme(&theme_now, cx);
                self.applied_theme = theme_now;
            }
            if accent_now != self.applied_accent {
                theme::apply_accent(&accent_now);
                self.applied_accent = accent_now;
            }
        }

        // ---- Core state read (once) ----
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
            let doc_prefix = core.host.tr("Doc: ");
            let doc_none = core.host.tr("(none)");
            let rev_fmt = core.host.tr(" · rev r");
            let status = StatusData {
                doc: snap
                    .active_document
                    .as_ref()
                    .map(|d| {
                        format!(
                            "{doc_prefix}{}{}{rev_fmt}{}",
                            d.title,
                            if d.dirty { " •" } else { "" },
                            d.revision
                        )
                    })
                    .unwrap_or_else(|| format!("{doc_prefix}{doc_none}")),
                mode: core.host.tr(&snap.work_mode),
                running,
                task: last_task.cloned(),
                last_log: core
                    .host
                    .log
                    .last()
                    .map(|e| (e.level, format!("{} {}", e.time, e.message))),
                // Account entry (Phase 3): only present when the product
                // injected an account service.
                account: core.host.account.as_ref().map(|_| {
                    let tr = |k: &str| core.host.tr(k);
                    match &core.host.ws.account {
                        wb::AccountPhase::LoggedOut => AccountChip::LoggedOut {
                            login_label: tr("Sign in"),
                        },
                        wb::AccountPhase::Pending(req) => AccountChip::Pending {
                            user_code: req.user_code.clone(),
                            uri: req.verification_uri.clone(),
                            cancel_label: tr("Cancel"),
                        },
                        wb::AccountPhase::LoggedIn(user) => AccountChip::LoggedIn {
                            name: user.name.clone(),
                            logout_label: tr("Sign out"),
                        },
                        wb::AccountPhase::Failed(msg) => AccountChip::Failed {
                            message: msg.clone(),
                            retry_label: tr("Sign in"),
                        },
                    }
                }),
            };
            (shell_snap, status)
        };

        // ---- Dock sync (model → dock; rebuilt only on structural changes) ----
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

        // ---- RibbonBar (ribbon crate mount; ribbon_bridge feeds per-frame model snapshots) ----
        self.ribbon.ensure_mounted(&core, weak.clone(), cx);
        self.ribbon.sync(&core, cx);
        let ribbon_element = self.ribbon.element();

        // ---- Assembly ----
        let mut root_col = div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
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
                    // While a text edit is open the Input owns the
                    // keystrokes; hotkeys stay disabled so typing a key that
                    // is also a shortcut does not fire the shortcut.
                    if text_edit::is_active(&core_k) || file_picker::is_open(&core_k) {
                        return;
                    }
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

        // ---- About dialog overlay (rendered while the app.about command has it set) ----
        if let Some(overlay) = about::maybe_about_overlay(&core, cx) {
            root_col = root_col.child(modal_overlay(overlay));
        }

        // ---- In-app file picker (open/save for OPFS and cloud stores):
        // mount a save picker's file-name input when it opens, drop it when
        // the picker closes. Drawn below the text-edit overlay, which hosts
        // its "new folder" prompt.
        let picker_open = file_picker::is_open(&core);
        if picker_open != self.picker_mounted {
            self.picker_mounted = picker_open;
            self.picker_input = file_picker::save_name(&core)
                .map(|seed| file_picker::create_name_input(&core, &seed, window, cx));
        }
        let name_input = self.picker_input.as_ref().map(|(input, _)| input.clone());
        if let Some(overlay) = file_picker::overlay(&core, name_input.as_ref(), cx) {
            root_col = root_col.child(modal_overlay(overlay));
        }

        // ---- Text-edit overlay (gpui-component Input): settings values and
        // command value prompts. Mount when an edit opens (or its key
        // changes); drop when it closes.
        if let Some(mount) = text_edit::mount_for(&core, self.edit_key.as_deref()) {
            let input = text_edit::create_input(&mount.seed, window, cx);
            let sub = text_edit::subscribe_enter(
                core.clone(),
                input.clone(),
                mount.key.clone(),
                mount.commit,
                cx,
            );
            self.edit_input = Some(input);
            self.edit_subscription = Some(sub);
            self.edit_key = Some(mount.key);
        } else if self.edit_key.is_some()
            && core
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .ws
                .text_edit
                .is_none()
        {
            self.edit_input = None;
            self.edit_subscription = None;
            self.edit_key = None;
        }
        if let Some(input) = &self.edit_input {
            let caption = text_edit::caption(&core);
            let input_slot = div()
                .rounded(px(4.0))
                .border_1()
                .border_color(col(theme::ui_theme().placeholder))
                .child(gpui_component::input::Input::new(input));
            let overlay = text_edit::overlay_shell(&caption, &core, input_slot, cx);
            root_col = root_col.child(modal_overlay(overlay));
        }

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
    /// Account chip (Phase 3); None when the product has no account service.
    account: Option<AccountChip>,
}

/// What the status bar's account entry shows (pre-`tr`ed labels).
#[derive(Clone)]
enum AccountChip {
    LoggedOut {
        login_label: String,
    },
    Pending {
        user_code: String,
        uri: String,
        cancel_label: String,
    },
    LoggedIn {
        name: String,
        logout_label: String,
    },
    Failed {
        message: String,
        retry_label: String,
    },
}

fn render_status_bar(
    data: &StatusData,
    core: SharedCore,
    weak: gpui::WeakEntity<RootView>,
) -> gpui::AnyElement {
    let mut right_side = div().flex().flex_row().items_center().gap_2();
    // Account chip (Phase 3): login / pending code / user + logout.
    if let Some(chip) = &data.account {
        right_side = right_side.child(render_account_chip(chip, core.clone(), weak.clone()));
    }
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
                        .child(core.lock().unwrap_or_else(|p| p.into_inner()).host.tr("Cancel"))
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
                        .child(format!(
                            "{}{}",
                            core.lock().unwrap_or_else(|p| p.into_inner()).host.tr("Mode: "),
                            data.mode
                        )),
                ),
        )
        .child(right_side)
        .into_any_element()
}

/// The status-bar account entry (Phase 3): a click-dispatch chip per phase.
fn render_account_chip(
    chip: &AccountChip,
    _core: SharedCore,
    weak: gpui::WeakEntity<RootView>,
) -> gpui::AnyElement {
    /// A small clickable chip dispatching an account command.
    fn chip_button(
        label: String,
        command: &'static str,
        weak: &gpui::WeakEntity<RootView>,
    ) -> gpui::Stateful<Div> {
        let weak = weak.clone();
        div()
            .id(SharedString::from(format!("account-chip-{command}")))
            .px(px(6.0))
            .py(px(1.0))
            .rounded_sm()
            .cursor_pointer()
            .text_size(px(11.0))
            .text_color(col(wb::Color::LIGHT_GRAY))
            .bg(col(theme::ui_theme().toolbar))
            .hover(|s| s.bg(col(wb::Color::rgb(0.30, 0.31, 0.36))))
            .child(label)
            .on_click(move |_ev: &gpui::ClickEvent, _w: &mut Window, cx: &mut App| {
                wb::dispatch(command, wb::CommandArgs::default());
                notify(&weak, cx);
            })
    }

    match chip {
        AccountChip::LoggedOut { login_label } => div().child(chip_button(
            login_label.clone(),
            "app.account.login",
            &weak,
        )),
        AccountChip::Pending {
            user_code,
            uri,
            cancel_label,
        } => div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_size(px(11.0))
            .text_color(col(wb::Color::YELLOW))
            .child(div().child(format!("{user_code} · {uri}")))
            .child(chip_button(
                cancel_label.clone(),
                "app.account.cancel_login",
                &weak,
            )),
        AccountChip::LoggedIn { name, logout_label } => div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_size(px(11.0))
            .text_color(col(wb::Color::LIGHT_GRAY))
            .child(div().child(name.clone()))
            .child(chip_button(
                logout_label.clone(),
                "app.account.logout",
                &weak,
            )),
        AccountChip::Failed {
            message,
            retry_label,
        } => div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .text_size(px(11.0))
            .child(div().text_color(col(wb::Color::RED)).child(message.clone()))
            .child(chip_button(retry_label.clone(), "app.account.login", &weak)),
    }
    .into_any_element()
}

/// gpui entry point: builds the product and runs the main loop.
///
/// Desktop: `Application::run` blocks until exit. Web (wasm32): builds `gpui_web`'s WebPlatform
/// itself (WebGPU/WebGL rendering; the GPU viewports need its wgpu context), driven by the
/// browser event loop; injects the fonts first (gpui_web ships no fonts: the embedded Latin font
/// plus a same-origin CJK font fetched asynchronously, all before the first frame so the layout
/// cache doesn't hit stale glyphs), and keeps the app alive with the handle `run_embedded`
/// returns (page lifetime = app lifetime).
pub fn run(builder: workbench_core::WorkbenchAppBuilder, window_title: &str) -> Result<(), String> {
    let mut builder = builder;
    // The built-in platform panels are view implementations, so the frontend
    // registers them — before core's `platform_cmds::register` builds the
    // "View" toolbar group, which then picks them up with every other panel.
    builder = builder.with_plugin_stage(Box::new(platform_panels::register));
    let smoke = builder.smoke_seconds();
    let title: SharedString = window_title.to_string().into();
    let assets = builder.asset_source();
    // The product's declared GPU device requirements (`gpu_view::GpuRequirements`): in place
    // before the GPU viewport device is created; the viewport baseline applies if none are
    // declared.
    let gpu_requirements = builder.take_gpu_requirements();
    let runtime = builder.build()?;
    let core: SharedCore = std::rc::Rc::new(Mutex::new(runtime));
    // Theme from settings (builder default already seeded): applied after
    // gpui_component::init in launch_app, before any window opens.
    let theme_name = {
        let core = core.lock().unwrap_or_else(|p| p.into_inner());
        core.host.settings.get_str("ui.theme", "dark")
    };
    // Restore the last layout (model + the dock areas' physical layout): the model lands first,
    // and the dock layout is laid over it on the first frame after the window opens. With no
    // layout file (first launch) or a parse failure this returns None → the default layout.
    let dock = {
        let mut core = core.lock().unwrap_or_else(|p| p.into_inner());
        core.load_layout()
            .and_then(|restored| restored.dock)
            .and_then(|dock| serde_json::from_value(dock).ok())
    };
    // Window/taskbar icon from the product profile (`window_icon` PNG asset).
    // Decode failure is non-fatal: the exe's embedded icon stays the fallback.
    let window_icon = {
        let path = core
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .profile()
            .window_icon
            .clone();
        path.and_then(|p| decode_window_icon(&assets, &p))
    };

    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(async move {
            // Fonts are injected before the window opens: cosmic-text's fallback resolution and
            // gpui's layout cache both hit by content hash, so fonts added after startup keep
            // hitting stale glyphs. The Latin font is embedded by this crate (the gpui_web fork
            // has an empty font library).
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
            // The web platform is built here rather than by gpui_platform,
            // whose constructors don't hand it back: the GPU viewports render
            // on its wgpu context (`WebPlatform::wgpu_context`). Single
            // threaded — wasm threads need nightly and a cross-origin
            // isolated page. Canvas fallback covers CJK when no cjk.ttf was
            // fetched.
            let platform = Rc::new(gpui_web::WebPlatform::new_with_backend_and_font_fallback(
                false,
                gpui_web::WebBackendPreference::Auto,
                gpui_web::CanvasFontFallback::EmojiAndCjk,
            ));
            gpu_view::set_web_platform(platform.clone());
            let app = gpui::Application::with_platform(platform).with_assets(WbAssets {
                app: assets,
                component: component_assets::ComponentAssets::new(""),
            });
            let handle =
                app.run_embedded(launch_app(core, title, smoke, fonts, gpu_requirements, dock, theme_name, window_icon));
            // The web platform's run returns at once and the handle keeps the app state alive;
            // it lives and dies with the page, so the handle is leaked on purpose.
            std::mem::forget(handle);
        });
        return Ok(());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Product asset source + gpui-component's built-in assets as the fallback
        let app = gpui_platform::application().with_assets(WbAssets {
            app: assets,
            component: component_assets::ComponentAssets::new(""),
        });
        app.run(launch_app(
            core,
            title,
            smoke,
            Vec::new(),
            gpu_requirements,
            dock,
            theme_name,
            window_icon,
        ));
        Ok(())
    }
}

/// Decode the product's window/taskbar icon (PNG asset path) into an
/// `RgbaImage` for `WindowOptions::icon`. Returns None on a missing asset or
/// decode failure — the exe's embedded icon stays the fallback.
#[cfg(not(target_arch = "wasm32"))]
fn decode_window_icon(
    assets: &std::sync::Arc<dyn wb::AssetSource>,
    path: &str,
) -> Option<std::sync::Arc<image::RgbaImage>> {
    let bytes = assets.load(path)?;
    let img = image::load_from_memory(&bytes).ok()?;
    Some(std::sync::Arc::new(img.into_rgba8()))
}

#[cfg(target_arch = "wasm32")]
fn decode_window_icon(
    _assets: &std::sync::Arc<dyn wb::AssetSource>,
    _path: &str,
) -> Option<std::sync::Arc<image::RgbaImage>> {
    None
}

/// Builds the app launch closure: registers components, opens the main window, hooks up the task
/// pump and font injection.
fn launch_app(
    core: SharedCore,
    title: SharedString,
    smoke: Option<f32>,
    fonts: Vec<std::borrow::Cow<'static, [u8]>>,
    gpu_requirements: Option<Box<dyn std::any::Any>>,
    dock: Option<gpui_component::dock::DockAreaState>,
    theme_name: String,
    window_icon: Option<std::sync::Arc<image::RgbaImage>>,
) -> impl FnOnce(&mut App) {
    move |cx: &mut App| {
        if !fonts.is_empty() {
            let _ = cx.text_system().add_fonts(fonts);
        }
        // The product's declared extra GPU device requirements must be in place before the GPU
        // viewport device is created (when the first GPU view draws).
        if let Some(requirements) = gpu_requirements {
            match requirements.downcast::<gpu_view::GpuRequirements>() {
                Ok(requirements) => gpu_view::set_requirements(*requirements),
                Err(_) => eprintln!(
                    "with_gpu_requirements: expected workbench_ui::gpu_view::GpuRequirements; ignored"
                ),
            }
        }
        gpui_component::init(cx);
        gpui_rhai::install(cx);
        // The theme is applied after component init and before any window opens: shell colors,
        // ribbon tokens and the gpui-component global theme switch over together. The brand
        // accent (ui.accent, from product.toml's branding.accent) lands in the ribbon tokens at
        // the same time.
        theme::apply_theme(&theme_name, cx);
        {
            let accent = core
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .host
                .settings
                .get_str("ui.accent", "#338CF2");
            theme::apply_accent(&accent);
        }
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone()),
                appears_transparent: false,
                ..Default::default()
            }),
            // Product window icon (product.toml `window_icon`); None keeps
            // the platform default (exe resource on Windows).
            icon: window_icon,
            ..Default::default()
        };
        let core_window = core.clone();
        let dock_window = dock.clone();
        let handle = cx
            .open_window(options, |_window, cx| {
                cx.new(|_cx| RootView::new(core_window, dock_window))
            })
            .expect("failed to open the window");
        let _ = handle;
        cx.activate(true);

        // Platform tick timer: pumps task events, drives autosave, refreshes the context snapshot,
        // and schedules a frame when there is UI-visible work (a task running / events received /
        // dialog results).
        let root_entity = cx
            .active_window()
            .and_then(|w| w.downcast::<RootView>())
            .and_then(|h| h.entity(cx).ok());
        let weak_root = root_entity.as_ref().map(|e| e.downgrade());
        let core_loop = core.clone();

        // Closing the window saves the layout: the core part (panels/tabs/mode) + the dock areas'
        // physical layout (dock dump). One window, one project is the product's settled shape
        // (web and desktop alike), so one layout = one window.
        //
        // This holds a **strong** handle to the root view: when the window closes its own handle
        // is released first, so a weak reference would be too late to upgrade and the layout
        // wouldn't get saved (verified in practice).
        let core_save = core.clone();
        cx.on_window_closed(move |cx, _| {
            // A failed save is only logged; it doesn't block exit.
            if let Some(root) = &root_entity {
                if save_layout(&core_save, root, cx) {
                    let mut core = core_save.lock().unwrap_or_else(|p| p.into_inner());
                    core.host.log.info("Layout saved");
                }
            }
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // A browser tab closes without a window-closed event, so the web
        // saves the layout as it goes; an unchanged layout isn't rewritten.
        #[cfg(target_arch = "wasm32")]
        if let Some(weak) = weak_root.clone() {
            let core_web = core.clone();
            cx.spawn(async move |cx: &mut gpui::AsyncApp| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                cx.update(|cx| {
                    if let Some(root) = weak.upgrade() {
                        save_layout(&core_web, &root, cx);
                    }
                });
            })
            .detach();
        }

        cx.spawn(async move |cx: &mut gpui::AsyncApp| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            // The tick itself returns whether a repaint is needed: true for a running task, a
            // received terminal event, a dialog result, an autosave or an active-document change.
            // The old implementation only looked at `running > 0`, so the predicate turned false
            // the moment a task finished and the terminal event never got pumped; the Diagnostics
            // panel's "Background tasks running/total" then lagged until the next input.
            let needs_repaint = {
                let mut core = core_loop.lock().unwrap_or_else(|p| p.into_inner());
                core.frame_tick(0.1)
            };
            if needs_repaint {
                if let Some(weak) = weak_root.clone() {
                    cx.update(|cx| notify(&weak, cx));
                }
            }
        })
        .detach();

        // Smoke mode: quit automatically after N seconds (desktop only; the web has no notion of
        // quitting)
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
            component: component_assets::ComponentAssets::new(""),
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

        // Every area carries the model's panels (the example workbench registers
        // Explorer/Tasks/Properties/Output, and the platform adds Diagnostics/Plugins on the
        // right).
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
                "app.panel.plugins",
                "app.panel.settings"
            ]
        );
        assert_eq!(snap.right.active.as_deref(), Some("example.panel.tasks"));
        assert_eq!(snap.bottom.panels[0].id, "example.panel.output");
        assert_eq!(snap.bottom.size, 170.0);

        // Central tabs and the active tab.
        assert_eq!(snap.tabs.len(), 1, "the welcome tab is open");
        assert_eq!(snap.active_tab, Some(snap.tabs[0]));
    }
}

