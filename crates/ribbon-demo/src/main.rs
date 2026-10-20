//! Standalone RibbonBar demo: renders only the ribbon (no dock, no panels),
//! with enough groups and item kinds to exercise every visual path.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, App, Bounds, Context, Entity, Size, Window, WindowBounds, WindowOptions,
};
use ribbon::{
    ItemSize, MenuEntry, OverflowStrategy, RibbonEvent, RibbonGroupVm, RibbonItemKindVm, RibbonItemVm,
    RibbonModel, RibbonState, RibbonTabVm,
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

/// Build the demo model: two tabs, many groups, every item kind.
fn demo_model() -> RibbonModel {
    let l = |id: &str, label: &str, icon: &str| RibbonItemVm {
        id: id.to_string(),
        label: label.to_string(),
        icon: Some(icon.to_string()),
        enabled: true,
        size: ItemSize::Large,
        width: None,
        kind: RibbonItemKindVm::Command,
    };
    let s = |id: &str, label: &str, icon: &str| RibbonItemVm {
        id: id.to_string(),
        label: label.to_string(),
        icon: Some(icon.to_string()),
        enabled: true,
        size: ItemSize::Small,
        width: None,
        kind: RibbonItemKindVm::Command,
    };
    let icon = |id: &str| RibbonItemVm {
        id: id.to_string(),
        label: id.to_string(),
        icon: None,
        enabled: true,
        size: ItemSize::Icon,
        width: None,
        kind: RibbonItemKindVm::Command,
    };

    RibbonModel {
        logo: None,
        qat: vec![icon("demo.save"), icon("demo.undo"), icon("demo.redo")],
        tabs: vec![
            RibbonTabVm {
                id: "demo.home".into(),
                title: "Home".into(),
                contextual: false,
                groups: vec![
                    RibbonGroupVm {
                        id: "demo.g1".into(),
                        title: "File".into(),
                        items: vec![
                            l("demo.new", "New", "add.svg"),
                            l("demo.open", "Open", "explorer.svg"),
                            l("demo.save", "Save", "output.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g2".into(),
                        title: "Toggle".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.snap".into(),
                                label: "Snap".into(),
                                icon: Some("mode.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Toggle { checked: true },
                            },
                            RibbonItemVm {
                                id: "demo.grid".into(),
                                label: "Grid".into(),
                                icon: Some("properties.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Toggle { checked: false },
                            },
                            s("demo.zoom_fit", "Zoom Fit", "scan.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g3".into(),
                        title: "Split".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.run".into(),
                                label: "Run".into(),
                                icon: Some("generate.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Split {
                                    items: vec![MenuEntry::Item(ribbon::MenuEntryVm {
                                        id: "demo.run_all".into(),
                                        label: "Run All".into(),
                                        icon: None,
                                        enabled: true,
                                        checked: false,
                                    })],
                                },
                            },
                            RibbonItemVm {
                                id: "demo.debug".into(),
                                label: "Debug".into(),
                                icon: Some("clock.svg".into()),
                                enabled: true,
                                size: ItemSize::Large,
                                width: None,
                                kind: RibbonItemKindVm::Split {
                                    items: vec![
                                        MenuEntry::Item(ribbon::MenuEntryVm {
                                            id: "demo.debug_start".into(),
                                            label: "Start".into(),
                                            icon: None,
                                            enabled: true,
                                            checked: false,
                                        }),
                                        MenuEntry::Separator,
                                        MenuEntry::Item(ribbon::MenuEntryVm {
                                            id: "demo.debug_stop".into(),
                                            label: "Stop".into(),
                                            icon: None,
                                            enabled: false,
                                            checked: false,
                                        }),
                                    ],
                                },
                            },
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g4".into(),
                        title: "Menu".into(),
                        items: vec![RibbonItemVm {
                            id: "demo.file".into(),
                            label: "File".into(),
                            icon: Some("explorer.svg".into()),
                            enabled: true,
                            size: ItemSize::Small,
                            width: None,
                            kind: RibbonItemKindVm::Menu {
                                items: vec![
                                    MenuEntry::Item(ribbon::MenuEntryVm {
                                        id: "demo.export".into(),
                                        label: "Export".into(),
                                        icon: None,
                                        enabled: true,
                                        checked: false,
                                    }),
                                    MenuEntry::Item(ribbon::MenuEntryVm {
                                        id: "demo.import".into(),
                                        label: "Import".into(),
                                        icon: None,
                                        enabled: true,
                                        checked: false,
                                    }),
                                ],
                            },
                        }],
                    },
                    RibbonGroupVm {
                        id: "demo.g5".into(),
                        title: "Combo".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.lineweight".into(),
                                label: "Weight".into(),
                                icon: Some("properties.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: Some(120.0),
                                kind: RibbonItemKindVm::Combo {
                                    current: "0.25".into(),
                                    options: vec![
                                        "0.13".into(),
                                        "0.25".into(),
                                        "0.50".into(),
                                        "1.00".into(),
                                    ],
                                },
                            },
                            RibbonItemVm {
                                id: "demo.layer".into(),
                                label: "Layer".into(),
                                icon: Some("tasks.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: Some(120.0),
                                kind: RibbonItemKindVm::Combo {
                                    current: "0".into(),
                                    options: vec!["0".into(), "1".into(), "2".into()],
                                },
                            },
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g6".into(),
                        title: "Disabled".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.lock".into(),
                                label: "Lock".into(),
                                icon: Some("delete.svg".into()),
                                enabled: false,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Command,
                            },
                            RibbonItemVm {
                                id: "demo.readonly".into(),
                                label: "Read Only".into(),
                                icon: Some("warning.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Toggle { checked: true },
                            },
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g7".into(),
                        title: "Icon Row".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.icon1".into(),
                                label: "Measure".into(),
                                icon: Some("scan.svg".into()),
                                enabled: true,
                                size: ItemSize::Icon,
                                width: None,
                                kind: RibbonItemKindVm::Command,
                            },
                            RibbonItemVm {
                                id: "demo.icon2".into(),
                                label: "Properties".into(),
                                icon: Some("properties.svg".into()),
                                enabled: true,
                                size: ItemSize::Icon,
                                width: None,
                                kind: RibbonItemKindVm::Command,
                            },
                            RibbonItemVm {
                                id: "demo.icon3".into(),
                                label: "Clock".into(),
                                icon: Some("clock.svg".into()),
                                enabled: true,
                                size: ItemSize::Icon,
                                width: None,
                                kind: RibbonItemKindVm::Command,
                            },
                            RibbonItemVm {
                                id: "demo.icon4".into(),
                                label: "Mark".into(),
                                icon: Some("mode.svg".into()),
                                enabled: true,
                                size: ItemSize::Icon,
                                width: None,
                                kind: RibbonItemKindVm::Split {
                                    items: vec![MenuEntry::Item(ribbon::MenuEntryVm {
                                        id: "demo.icon4_menu".into(),
                                        label: "Mark Menu".into(),
                                        icon: None,
                                        enabled: true,
                                        checked: false,
                                    })],
                                },
                            },
                            RibbonItemVm {
                                id: "demo.icon5".into(),
                                label: "Tasks".into(),
                                icon: Some("tasks.svg".into()),
                                enabled: true,
                                size: ItemSize::Icon,
                                width: None,
                                kind: RibbonItemKindVm::Command,
                            },
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g10".into(),
                        title: "Edit".into(),
                        items: vec![
                            l("demo.cut", "Cut", "delete.svg"),
                            l("demo.copy", "Copy", "canvas.svg"),
                            l("demo.paste", "Paste", "add.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g11".into(),
                        title: "Annotate".into(),
                        items: vec![
                            s("demo.note", "Note", "warning.svg"),
                            s("demo.mark", "Mark Up", "mode.svg"),
                            s("demo.measure_dist", "Measure", "scan.svg"),
                            s("demo.dim", "Dimension", "generate.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g12".into(),
                        title: "View".into(),
                        items: vec![
                            l("demo.pan", "Pan", "canvas.svg"),
                            l("demo.orbit", "Orbit", "mode.svg"),
                            l("demo.zoom", "Zoom", "scan.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g13".into(),
                        title: "Filter".into(),
                        items: vec![
                            RibbonItemVm {
                                id: "demo.filter_sel".into(),
                                label: "Selected".into(),
                                icon: Some("properties.svg".into()),
                                enabled: true,
                                size: ItemSize::Small,
                                width: None,
                                kind: RibbonItemKindVm::Toggle { checked: true },
                            },
                            s("demo.filter_hidden", "Hidden", "delete.svg"),
                            s("demo.filter_locked", "Locked", "warning.svg"),
                        ],
                    },
                ],
            },
            RibbonTabVm {
                id: "demo.tools".into(),
                title: "Tools".into(),
                contextual: true,
                groups: vec![
                    RibbonGroupVm {
                        id: "demo.g8".into(),
                        title: "Contextual".into(),
                        items: vec![
                            s("demo.measure", "Measure", "scan.svg"),
                            s("demo.section", "Section", "canvas.svg"),
                        ],
                    },
                    RibbonGroupVm {
                        id: "demo.g9".into(),
                        title: "Export".into(),
                        items: vec![
                            l("demo.render", "Render", "generate.svg"),
                            s("demo.export_img", "Export Image", "output.svg"),
                            s("demo.print", "Print", "welcome.svg"),
                        ],
                    },
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
