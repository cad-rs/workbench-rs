# workbench-rs

> Repository: https://github.com/cad-rs/workbench-rs

A reusable Rust framework for generic engineering desktop applications (CAD,
mechanical design, industrial simulation, and similar products).
Requirements and architecture: [docs/design.md](docs/design.md). Development
plan and phase records: [docs/plan.md](docs/plan.md).

License: [MIT](LICENSE). Library crates are crates.io-ready under the
`workbench-*` names — **publishing is on hold for now** (see "Publishing").

## Repository layout

```text
crates/
  workbench-api         GUI-agnostic platform contracts: IDs, geometry, PaintBackend,
                        commands, tasks, documents, workspace/Ribbon model
  workbench-core        Platform runtime: assembly, command dispatch, tasks/async executor,
                        autosave, layout persistence, selfcheck
  workbench-ui          gpui frontend (gpui-ce + gpui-component + gpui-rhai script views)
  workbench-rhai        Rhai script plugin host (Host API v1: commands, toolbar,
                        script UI panels)
workbenches/
  wb-example            Example Workbench module (package name workbench-example,
                        depends only on workbench-api)
products/
  product-demo          Example desktop product
  product-web           Example web product (wasm32 + trunk, WebGPU rendering)
plugins/
  rhai-demo             Demo Rhai plugin (sync command + background progress task +
                        toolbar contribution + script UI panel)
docs/                   Requirements / plan / plugin authoring guide
```

Dependency direction: `product → ui → core → api` and `product → wb-example → api`.
**Workbench modules never touch gpui / rhai-rendering / wgpu** — GUI code lives
only in `workbench-ui`; plugin scripts live in `plugins/`.

> **UI framework decision (2026-10)**: gpui is the single frontend. The
> former egui frontend was removed; `workbench-ui-gpui` became
> `workbench-ui` and the gpui products were renamed accordingly
> (`product-demo`, `product-web`). See docs/plan.md §15.

> **gpui base is temporarily a git fork** ([patch.crates-io] →
> https://github.com/javagg/gpui-ce): it carries wgpu rendering fixes
> pending upstream merge. Remove the patch section in the root Cargo.toml to
> switch back to the crates.io release once merged.

## Quick start

```bash
cargo run -p product-demo               # run the example desktop product
cargo test --workspace                  # full test suite
powershell -File scripts/verify.ps1     # one-shot verification (check + tests + selfcheck + smoke)
```

> Evaluate interaction responsiveness (drag/zoom frame rate, GPU viewports)
> under a release build: `cargo run --release -p product-demo`. Dev builds
> carry UI-tree rebuild overhead that lowers the frame rate.

### Running in the browser (wasm32 + trunk)

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk

cd products/product-web
trunk serve                     # needs a WebGPU browser (Chrome/Edge 113+)
```

Optional: run `powershell -File scripts/web-copy-font.ps1` once to copy a
local CJK system font (not committed) into `assets/fonts/`; the app fetches
it at startup (before first frame) for Chinese text. A Latin base font
(IBM Plex Sans, OFL) is committed and embedded for the fork's empty-font
`gpui_web` platform.

Web-slice differences vs the desktop product: no script plugin stage (plugin
directories are a desktop convenience), no local file dialogs, no background
thread tasks (they fail with a terminal state; async commands run on the
main-thread event loop), and layout/autosave persistence is memory-only.

## Integration guide

### 1) Product developers: assemble a standalone application

Each product is its own crate and **fixes exactly one GUI at build time**. The
product `main` only performs assembly; the CLI conventions (run /
`--selfcheck` / `--selfcheck-crash` / `--smoke [secs]`) are hosted by the
platform:

```rust
use workbench_core::cli::{self, CliMode};

fn main() {
    let cli = cli::from_env();
    let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "My Product")
        .with_backend_label("gpui")                       // shown in the diagnostics panel
        .with_workbench(my_cad_workbench)                 // assemble Workbench modules
        .with_assets(my_assets())                         // SVG icons (see below)
        .with_plugin_stage(workbench_rhai::plugin_stage(
            workbench_core::cli::default_plugin_dirs("com.example.prod")))
        .create_default_document(true)
        .open_on_start("cad.viewport", true);
    let builder = cli.apply(builder);

    match cli.mode {
        CliMode::Selfcheck =>                             // headless selfcheck (probe-based, unrelated items SKIP)
            workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
        CliMode::CrashProbe =>                            // subprocess mode used by the selfcheck
            workbench_core::cli::crash_probe_exit(|| make_builder()),
        CliMode::Run => {
            workbench_ui::run(builder, "My Product").unwrap();
            std::process::exit(0);
        }
    }
}
```

A complete runnable version lives in `products/product-demo/src/main.rs`
(about 50 lines).

### 2) Workbench developers: contribute commands / panels / views / document types

A Workbench is a plain Rust crate that **depends only on `workbench-api`** and
implements one trait:

```rust
struct CadWorkbench;
impl wb::Workbench for CadWorkbench {
    fn id(&self) -> wb::WorkbenchId { wb::WorkbenchId::new("cad") }
    fn init(&self, reg: &mut wb::Registry) {
        // Commands (single entry point: Ribbon, hotkeys, scripts, plugins all run them)
        // .icon(...) attaches a monochrome SVG icon (see "SVG icons" below)
        reg.register_command(
            wb::CommandDef::sync("cad.extrude", "Extrude", |ctx, _args| {
                let doc = ctx.active_document().ok_or("no active document")?;
                ctx.host().edit_document(...)?;
                wb::CommandResult::done()
            })
            .hotkey(wb::Hotkey::parse("Ctrl+B").unwrap())
            .enabled_when(|snap| snap.active_document.is_some())
        );
        // Ribbon Tab (stable group IDs; legacy add_toolbar_group lands in the "Home" Tab)
        reg.add_ribbon_tab(wb::RibbonTab::new("cad.model", "Model").groups(vec![...]));
        // Dock panel / central view: implement ViewInstance::paint (GUI-agnostic drawing)
        reg.register_panel(wb::PanelDef::new("cad.props", "Properties", wb::DockArea::Right, 300.0, 0,
            || Box::new(PropsPanel)));
        // Script panel: the content is a gpui-rhai Rhai view instead of Rust
        reg.register_panel(wb::PanelDef::new("cad.notes", "Notes", wb::DockArea::Right, 300.0, 1,
            || unreachable!("script panels carry no Rust instance")).script("ui/notes.rhai"));
        reg.register_view(wb::ViewDef::new("cad.viewport", "3D View", true, || Box::new(ViewportView)));
        // Document type (serialization owned by the domain module, the platform
        // never reaches into the data model)
        reg.register_document_type(wb::DocumentTypeDef { ... });
    }
}
```

Key points:

- Views draw through `PaintBackend` (six primitives + `draw_icon`) and never
  touch GUI types;
- Panels registered with `.script(path)` mount a gpui-rhai view: the Rhai
  script returns a declarative UiNode tree rendered by the gpui frontend;
- Commands have three execution tiers: `Sync` (UI thread) / `Async` (shared
  executor) / `Background` (dedicated thread task with progress + cancellation);
- Document edits go through `DocumentService::edit/commit`; background results
  come back as `DocCommit` (version-checked) — undo/redo work automatically;
- `init` may panic — the failure is caught and isolated during assembly,
  without taking the product down.

#### SVG icons

Commands, central tabs, dock panels, and in-view buttons can carry a
monochrome SVG icon: declare a path (`.icon("icons/extrude.svg")`) and mount
an asset source on the product
(`builder.with_assets(wb_example::example_assets())` — typically a static
`include_bytes!` table via `wb::StaticAssets`). Lookup falls back to the
platform standard icons (new/open/save/undo/redo under `wb-std/`), and a
product can override those by reusing their paths. Icons render as an alpha
mask tinted with the widget foreground color (gray when disabled); a missing
or unparseable icon degrades to text-only UI.

### 3) Plugin developers: Rhai script extensions

See [docs/plugins.md](docs/plugins.md) and the runnable example
`plugins/rhai-demo/`. A plugin is a directory with `plugin.toml` plus a Rhai
entry script whose `register()` calls host-injected globals:

```rhai
// plugin.rhai
fn register() {
    register_command("rhai.hello", "Rhai 问候", "hello");           // sync command
    register_background_command("rhai.progress", "Rhai 后台任务", "progress_job");
    add_toolbar_item("plugin.rhai", "rhai.hello", "问候");          // Ribbon contribution
    register_panel("rhai.panel", "Rhai 演示", "ui/main.rhai", "right"); // script UI panel
}

fn hello(args) { `来自 Rhai 脚本的问候（{args.name}）` }          // 返回备注字符串

fn progress_job(task, args) {                                      // 后台进度 + 协作式取消
    for i in 0..20 {
        task.check_cancelled();
        task.report(i as float / 20.0, `批次 {i + 1}/20`);
    }
}
```

The `ui/main.rhai` script panel returns a declarative UiNode tree composed of
registry components (button/input/label/tag/progress/card/…), with state in
the script and callbacks as named functions — see `plugins/rhai-demo/ui/`.
Manifest validation, API version checks, error isolation, and runtime disable
are all built in.

## Running & diagnostics

| Command | Meaning |
|---|---|
| `--selfcheck` | Headless selfcheck (probe-based: items unrelated to the product SKIP automatically), exit code 0/1 |
| `--selfcheck-crash` | Crash-recovery probe (invoked by the selfcheck; not for manual runs) |
| `--smoke [secs]` | Run for the given number of seconds and exit (default 2.5), for CI smoke tests |
| `RUST_LOG=info` | wgpu/gpui diagnostics |
| stderr `wb-log [...]` | Mirror of the platform log (visible in the terminal, greppable in automation) |

## Script plugins (Rhai)

`plugins/rhai-demo/` is a runnable example: two commands (sync note +
background progress task), a Ribbon toolbar group, and one script UI panel.
Plugin directories are scanned from `./plugins` and the user config directory
(`default_plugin_dirs`). Plugin authoring: [docs/plugins.md](docs/plugins.md).

## Milestones (mapped to design.md §17 acceptance items)

- **Phase 1 (vertical slice)**: acceptance items #1–#10, #12–#18, #21–#23 prototyped and verified.
- **Phase 2 (async commands & transactions)**: three execution tiers (Sync / Async shared executor / Background thread), unified command invocation results with a queryable invocation history, `DocCommit` version-checked transactions, document revision API.
- **Phase 3 (plugin loop)**: manifest & discovery, API version check, Host API v1, error isolation, runtime disable, demo plugin — **now Rhai-based** (Python host removed, 2026-10).
- **RibbonBar**: three-level model `RibbonTab -> Group -> Command` (stable IDs, controlled merging, active-Tab persistence), rendering verified via screenshots.
- **Phase 5 (production hardening, first batch)**: settings service, autosave + crash recovery, plugin management panel, diagnostics panel with frame statistics, performance regression tests + one-shot verification script.
- **SVG icons**: GUI-agnostic icon contract (`AssetSource` + icon fields on commands/views/panels + `PaintBackend::draw_icon`), platform standard icon set, alpha-mask tinting and texture caching (design.md §9.2.1).
- **Web slice (wasm32 + trunk)**: gpui in the browser via `gpui_web` (WebGPU), fonts injected before first frame.
- **Single frontend (2026-10)**: egui frontend removed; crates renamed (`workbench-ui`, `product-demo`, `product-web`); script plugins moved from Python to Rhai (see docs/plan.md §15).
- TODO: wgpu render service (optional capability), process isolation for untrusted plugins (design sketch in plan.md section 10.6), multi-platform packaging.

## Publishing (crates.io)

**Publishing is on hold.** The gpui base currently resolves to the
javagg/gpui-ce git fork through `[patch.crates-io]`, so a crates.io release
of the `workbench-*` crates would not build from the registry alone. Once the
wgpu fixes are merged upstream and the patch is removed, publishing resumes
in dependency order (`workbench-api` → `workbench-core` → the rest);
`workbench-rhai` and `workbench-ui` are the current publishable set,
`product-demo` / `product-web` stay `publish = false`.
