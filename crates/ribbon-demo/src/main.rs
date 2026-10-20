//! Standalone RibbonBar demo: renders only the ribbon (no dock, no panels),
//! with enough groups and item kinds to exercise every visual path.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, App, Bounds, Context, Entity, Size, Window, WindowBounds, WindowOptions,
};
use ribbon::{
    ItemSize, MenuEntry, MenuEntryVm, OverflowStrategy, RibbonEvent, RibbonGroupVm, RibbonItemKindVm,
    RibbonItemVm, RibbonModel, RibbonState, RibbonTabVm,
};

/// Filesystem-based AssetSource: loads SVG icons from a directory so the
/// demo can render them without embedding binary blobs.
struct FsAssetSource {
    root: PathBuf,
}

impl gpui::AssetSource for FsAssetSource {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let full = self.root.join(path);
        match std::fs::read(&full) {
            Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        let mut paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "svg").unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        paths.push(gpui::SharedString::from(name.to_string()));
                    }
                }
            }
        }
        Ok(paths)
    }
}

/// Chained source: demo SVG icons first, gpui-component built-in icons
/// (chevrons, popup glyphs) as fallback — same shape as workbench-ui's
/// WbAssets.
struct DemoAssets {
    app: FsAssetSource,
    component: gpui_component_assets::Assets,
}

impl gpui::AssetSource for DemoAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = self.app.load(path)? {
            return Ok(Some(bytes));
        }
        self.component.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        // gpui builds its AssetRegistry from list(""): the union of both
        // sources must be reported or fallback lookups silently miss.
        let mut seen = std::collections::HashSet::new();
        let mut paths = Vec::new();
        for p in self
            .app
            .list(path)?
            .into_iter()
            .chain(self.component.list(path)?.into_iter())
        {
            if seen.insert(p.clone()) {
                paths.push(p);
            }
        }
        Ok(paths)
    }
}

/// One dropdown entry.
fn entry(id: &str, label: &str, enabled: bool) -> MenuEntry {
    MenuEntry::Item(MenuEntryVm {
        id: id.to_string(),
        label: label.to_string(),
        icon: None,
        enabled,
        checked: false,
    })
}

/// A dropdown separator.
fn sep() -> MenuEntry {
    MenuEntry::Separator
}

/// Builds the demo model: two tabs, many groups, every item kind. The local
/// helpers keep each item to one line — the model stays a flat, readable
/// inventory of the demo's ribbon.
fn demo_model() -> RibbonModel {
    // (id, label, icon, size, kind) — the shared item constructor.
    let item = |id: &str, label: &str, icon: Option<&str>, size: ItemSize, kind: RibbonItemKindVm| {
        RibbonItemVm {
            id: id.to_string(),
            label: label.to_string(),
            icon: icon.map(str::to_string),
            enabled: true,
            size,
            width: None,
            kind,
        }
    };
    let l = |id: &str, label: &str, icon: &str| {
        item(id, label, Some(icon), ItemSize::Large, RibbonItemKindVm::Command)
    };
    let s = |id: &str, label: &str, icon: &str| {
        item(id, label, Some(icon), ItemSize::Small, RibbonItemKindVm::Command)
    };
    let qat_icon = |id: &str| item(id, id, None, ItemSize::Icon, RibbonItemKindVm::Command);
    let icon_btn = |id: &str, label: &str, icon: &str| {
        item(id, label, Some(icon), ItemSize::Icon, RibbonItemKindVm::Command)
    };
    let toggle = |id: &str, label: &str, icon: &str, checked: bool| {
        item(id, label, Some(icon), ItemSize::Small, RibbonItemKindVm::Toggle { checked })
    };
    let disabled = |id: &str, label: &str, icon: &str| RibbonItemVm {
        enabled: false,
        ..item(id, label, Some(icon), ItemSize::Small, RibbonItemKindVm::Command)
    };
    let split = |id: &str, label: &str, icon: &str, size: ItemSize, items: Vec<MenuEntry>| {
        item(id, label, Some(icon), size, RibbonItemKindVm::Split { items })
    };
    let menu_s = |id: &str, label: &str, icon: &str, items: Vec<MenuEntry>| {
        item(id, label, Some(icon), ItemSize::Small, RibbonItemKindVm::Menu { items })
    };
    let combo_s = |id: &str, label: &str, icon: &str, current: &str, options: Vec<&str>| RibbonItemVm {
        width: Some(120.0),
        ..item(
            id,
            label,
            Some(icon),
            ItemSize::Small,
            RibbonItemKindVm::Combo {
                current: current.to_string(),
                options: options.into_iter().map(str::to_string).collect(),
            },
        )
    };

    let group = |id: &str, title: &str, items: Vec<RibbonItemVm>| RibbonGroupVm {
        id: id.to_string(),
        title: title.to_string(),
        items,
    };

    RibbonModel {
        logo: None,
        qat: vec![qat_icon("demo.save"), qat_icon("demo.undo"), qat_icon("demo.redo")],
        tabs: vec![
            RibbonTabVm {
                id: "demo.home".into(),
                title: "Home".into(),
                contextual: false,
                groups: vec![
                    group("demo.g1", "File", vec![
                        l("demo.new", "New", "add.svg"),
                        l("demo.open", "Open", "explorer.svg"),
                        l("demo.save", "Save", "output.svg"),
                    ]),
                    group("demo.g2", "Toggle", vec![
                        toggle("demo.snap", "Snap", "mode.svg", true),
                        toggle("demo.grid", "Grid", "properties.svg", false),
                        s("demo.zoom_fit", "Zoom Fit", "scan.svg"),
                    ]),
                    group("demo.g3", "Split", vec![
                        split("demo.run", "Run", "generate.svg", ItemSize::Small, vec![
                            entry("demo.run_all", "Run All", true),
                        ]),
                        split("demo.debug", "Debug", "clock.svg", ItemSize::Large, vec![
                            entry("demo.debug_start", "Start", true),
                            sep(),
                            entry("demo.debug_stop", "Stop", false),
                        ]),
                    ]),
                    group("demo.g4", "Menu", vec![menu_s("demo.file", "File", "explorer.svg", vec![
                        entry("demo.export", "Export", true),
                        entry("demo.import", "Import", true),
                    ])]),
                    group("demo.g5", "Combo", vec![
                        combo_s("demo.lineweight", "Weight", "properties.svg", "0.25",
                            vec!["0.13", "0.25", "0.50", "1.00"]),
                        combo_s("demo.layer", "Layer", "tasks.svg", "0", vec!["0", "1", "2"]),
                    ]),
                    group("demo.g6", "Disabled", vec![
                        disabled("demo.lock", "Lock", "delete.svg"),
                        toggle("demo.readonly", "Read Only", "warning.svg", true),
                    ]),
                    group("demo.g7", "Icon Row", vec![
                        icon_btn("demo.icon1", "Measure", "scan.svg"),
                        icon_btn("demo.icon2", "Properties", "properties.svg"),
                        icon_btn("demo.icon3", "Clock", "clock.svg"),
                        split("demo.icon4", "Mark", "mode.svg", ItemSize::Icon, vec![
                            entry("demo.icon4_menu", "Mark Menu", true),
                        ]),
                        icon_btn("demo.icon5", "Tasks", "tasks.svg"),
                    ]),
                    group("demo.g10", "Edit", vec![
                        l("demo.cut", "Cut", "delete.svg"),
                        l("demo.copy", "Copy", "canvas.svg"),
                        l("demo.paste", "Paste", "add.svg"),
                    ]),
                    group("demo.g11", "Annotate", vec![
                        s("demo.note", "Note", "warning.svg"),
                        s("demo.mark", "Mark Up", "mode.svg"),
                        s("demo.measure_dist", "Measure", "scan.svg"),
                        s("demo.dim", "Dimension", "generate.svg"),
                    ]),
                    group("demo.g12", "View", vec![
                        l("demo.pan", "Pan", "canvas.svg"),
                        l("demo.orbit", "Orbit", "mode.svg"),
                        l("demo.zoom", "Zoom", "scan.svg"),
                    ]),
                    group("demo.g13", "Filter", vec![
                        toggle("demo.filter_sel", "Selected", "properties.svg", true),
                        s("demo.filter_hidden", "Hidden", "delete.svg"),
                        s("demo.filter_locked", "Locked", "warning.svg"),
                    ]),
                ],
            },
            RibbonTabVm {
                id: "demo.tools".into(),
                title: "Tools".into(),
                contextual: true,
                groups: vec![
                    group("demo.g8", "Contextual", vec![
                        s("demo.measure", "Measure", "scan.svg"),
                        s("demo.section", "Section", "canvas.svg"),
                    ]),
                    group("demo.g9", "Export", vec![
                        l("demo.render", "Render", "generate.svg"),
                        s("demo.export_img", "Export Image", "output.svg"),
                        s("demo.print", "Print", "welcome.svg"),
                    ]),
                ],
            },
        ],
        active_tab_id: "demo.home".into(),
        collapsed: false,
        collapse_enabled: true,
        overflow: OverflowStrategy::Scroll,
        work_mode: "demo".into(),
    }
}

struct DemoView {
    _ribbon: Entity<RibbonState>,
}

impl Render for DemoView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let bg = match ribbon::theme() {
            ribbon::ThemeKind::Light => rgb(0xEFEDEB),
            ribbon::ThemeKind::Dark => rgb(0x2b2c30),
        };
        div().flex().flex_col().w_full().bg(bg).child(self._ribbon.clone())
    }
}

fn main() {
    env_logger::init();
    // `--light` starts with the light palette (default: dark).
    let light = std::env::args().any(|a| a == "--light");
    if light {
        ribbon::set_theme(ribbon::ThemeKind::Light);
    }
    let app = gpui_platform::application();
    // Register the demo's SVG icon directory with gpui's AssetRegistry,
    // with gpui-component built-ins as fallback (chevrons, menu glyphs).
    let icon_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../workbenches/wb-example/assets/icons");
    let app = app.with_assets(
        Arc::new(DemoAssets {
            app: FsAssetSource { root: icon_dir },
            component: gpui_component_assets::Assets::new(""),
        }) as Arc<dyn gpui::AssetSource>,
    );

    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        let bounds = Bounds::centered(
            None,
            Size { width: px(600.), height: px(200.) },
            cx,
        );
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| {
            let model = demo_model();
            let ribbon = cx.new(|_| {
                RibbonState::new(
                    model,
                    |_state, event, _window, _cx| match event {
                        RibbonEvent::Command { id, value } => {
                            println!("command: {id} value: {value:?}");
                        }
                        RibbonEvent::ActiveTabChanged { id } => {
                            println!("tab changed: {id}");
                        }
                        RibbonEvent::ToggleCollapse => {
                            println!("toggle collapse");
                        }
                    },
                )
            });
            cx.new(|_| DemoView { _ribbon: ribbon })
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
