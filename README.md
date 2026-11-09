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
  workbench-api         GUI-agnostic platform contracts: IDs, geometry, commands, tasks,
                        documents, storage, entitlements, workspace/Ribbon model
  workbench-core        Platform runtime: assembly, command dispatch, tasks/async executor,
                        storage providers, autosave, diagnostics, layout persistence, selfcheck
  workbench-ui          gpui frontend (gpui-fast + gpui-component + gpui-rhai script views)
  workbench-render      wgpu viewport render core (camera, pipelines, headless-testable)
  workbench-rhai        Rhai script plugin host (Host API v1: commands, toolbar,
                        script UI panels)
  ribbon                RibbonBar component for gpui (no workbench dependency)
  ribbon-demo           Standalone RibbonBar demo
workbenches/
  wb-example            Example Workbench module (package name workbench-example)
products/
  product-demo          Example desktop product
  product-web           Example web product (wasm32 + trunk, WebGPU rendering)
plugins/
  rhai-demo             Demo Rhai plugin (sync command + background progress task +
                        toolbar contribution + script UI panel)
docs/                   Requirements / plan / plugin authoring guide
scripts/                One-shot verification, web font helper
.github/workflows/      CI (see "Continuous integration")
```

Dependency direction: `product → ui → core → api` and `product → wb-example → api`.
Commands, documents and the rest of a Workbench module are GUI-agnostic. The
exception is a view's content, which is a gpui element tree
(`ViewInstance::element`), so a module with views also depends on `gpui`; it
never touches `wgpu` or the frontend crates. Plugin scripts live in `plugins/`.

> **UI framework decision (2026-10)**: gpui is the single frontend. The
> former egui frontend was removed; `workbench-ui-gpui` became
> `workbench-ui` and the gpui products were renamed accordingly
> (`product-demo`, `product-web`). See docs/plan.md §15.

> **gpui base is gpui-fast from git** (https://github.com/longbridge/gpui-fast,
> main): the web GPU viewport needs `WebPlatform::wgpu_context` and
> `WgpuDynamicTexture`, which no crates.io release carries yet. gpui-component
> and gpui-rhai come from crates.io; `[patch.crates-io]` points the `gpui-pre`
> snapshots they build on at the same gpui-fast commit (see the root
> Cargo.toml and `.vendor/gpui-pre-compat`). GPU viewports present through
> a DirectComposition surface on Windows and a GPUI dynamic texture on the
> web, and read frames back on other desktops
> (`crates/workbench-ui/src/gpu_view/present.rs`).

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
directories are a desktop convenience) and no background thread tasks (they
fail with a terminal state; async commands run on the main-thread event loop).
Documents live in the browser's private file system (OPFS), browsed by an
in-app picker; Import and Export move copies to and from the device through the
browser's chooser and downloads. Settings and the layout are kept in
localStorage. There is no autosave, crash recovery or log file, and the
device-flow login's HTTP client is desktop-only. Browser sign-in opens the
sign-in page in another tab and needs `auth-callback.html` served on the app's
origin, that URL accepted as a redirect, and CORS on the token and userinfo
endpoints. Closing the desktop window
with unsaved documents asks whether to save them; on the web, where a page
cannot save as it closes, the browser asks whether to leave.

## Integration guide

### 1) Product developers: assemble a standalone application

Each product is its own crate and **fixes exactly one GUI at build time**. The
product `main` only performs assembly; the CLI conventions (run /
`--selfcheck` / `--selfcheck-crash` / `--smoke [secs]`) are hosted by the
platform:

```rust
use workbench_core::cli::{Cli, CliMode};

fn main() {
    let cli = Cli::from_env();
    let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "My Product")
        .with_backend_label("gpui")                       // shown in the diagnostics panel
        .with_workbench(my_cad_workbench)                 // assemble Workbench modules
        .with_assets(my_assets())                         // SVG icons (see below)
        .with_plugin_stage(workbench_rhai::plugin_stage(
            workbench_core::cli::default_plugin_dirs("com.example.prod")))
        .create_default_document(true)
        .open_on_start("cad.viewport", true);
    let builder = cli.apply(builder);
    workbench_core::diagnostics::init(&builder);          // log file + crash reports

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
(about 70 lines).

#### What a product can plug in

| Builder call | What it adds |
|---|---|
| `with_storage(id, provider)` | Another place documents can live (a cloud store, …), offered in the in-app file picker |
| `with_account(service)` | Sign-in; `auth_code::AuthCodeClient` (desktop, loopback redirect) and `auth_code::WebAuthCodeClient` (web, callback page) sign in through the browser with the authorization code + PKCE, configured with `AuthCodeConfig`; `account::DeviceFlowClient` is a ready OAuth 2.0 device-flow client (its session lasts until the app exits) |
| `with_entitlements(e)` | Which features the user may use (below) |
| `with_crash_reporter(r)` | Where crash reports go; adds a setting that is off by default |
| `with_profile(p)` | Identity and branding from `product.toml` (`product::discover()` finds one) |

#### Limiting features

The framework is open source and free for commercial use, and how a product is
sold is the product's decision, so there are no tiers or license formats here.
A feature is a name the product chooses, and the product decides who has it:

```rust
struct MyLicense { /* … */ }

impl wb::Entitlements for MyLicense {
    fn allows(&self, feature: &str) -> bool { /* … */ }     // asked many times a second: answer from memory
    fn refusal(&self, feature: &str) -> String {            // optional
        format!("{feature} needs the Pro plan")
    }
}

let builder = builder.with_entitlements(Arc::new(MyLicense::load()));
```

Put `.requires("pro.export")` on a `CommandDef`, `ViewDef` or `PanelDef`, or
`requires = "pro.export"` in a plugin manifest. Without the feature, commands
and panel toggles are disabled, views don't open, panels stay hidden and plugins
don't load. A feature gained while the app runs applies within a tick. A feature
lost stays usable until the app closes, so nobody loses a view mid-work; the
next launch closes what is no longer allowed, and documents are untouched.
Signing out ends that grace at once, and so does a check at startup that finds
the plan has lapsed. Domain code can ask `ctx.host().allows("…")` for anything
finer. With nothing injected, everything is allowed.

A plan can change while the app runs. To pick that up without a restart, fetch
it in `refresh`:

```rust
fn refresh(self: Arc<Self>) -> wb::BoxFuture<Result<(), String>> {
    Box::pin(async move { /* fetch the plan, update what `allows` reads */ Ok(()) })
}
```

The platform calls it one at a time: at startup, when the window comes back to
the foreground (at most every 30 seconds), after sign-in and sign-out, every 30
minutes and from **Help → Refresh License**. Open the checkout with
`ctx.open_purchase_page(url)`: when the user comes back, the platform checks
every few seconds for two minutes, because a payment can take a moment to reach
the product's server. On an error, leave the answers as they are, so a lost
connection takes nothing away. Plugins are still only checked as they load, so
a plugin the new plan allows needs a restart.

Verifying licenses is the product's job. A manifest is a plain text file, so
gate what matters in compiled code.

#### Languages

UI text is written in English, and those strings are the keys of translation
tables: `reg.register_locales("zh-CN", [("Save", "保存")])`. Chinese ships for the
platform's own text. The `ui.locale` setting picks the language, and
`[i18n] default_locale` in `product.toml` sets its default.

### 2) Workbench developers: contribute commands / panels / views / document types

A Workbench is a plain Rust crate that **depends on `workbench-api`** (plus
`gpui` for the content of its views) and implements one trait:

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
        // Dock panel / central view: implement ViewInstance::element (a gpui element tree)
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

- A view's `element` returns a gpui element tree, handed to the frontend
  type-erased so the platform contract stays GUI-agnostic;
- Panels registered with `.script(path)` mount a gpui-rhai view: the Rhai
  script returns a declarative UiNode tree rendered by the gpui frontend;
- Commands have three execution tiers: `Sync` (UI thread) / `Async` (shared
  executor) / `Background` (dedicated thread task with progress + cancellation);
- Document edits go through `DocumentService::edit/commit`; background results
  come back as `DocCommit` (version-checked) — undo/redo work automatically;
- Files go through storage providers: opening and saving run off the UI thread,
  and a save is all-or-nothing, so a crash or full disk can't damage the file;
- `DocumentTypeDef::format` versions a type's files, with one upgrade step per
  version (`None` for formats defined elsewhere). Older files upgrade as they
  open, the first save over one keeps a copy (`part.v1.ext`), and files this
  build can't read are refused with the reason;
- `init` may panic — the failure is caught and isolated during assembly,
  without taking the product down.

#### SVG icons

Commands, central tabs, dock panels, and in-view buttons can carry a
monochrome SVG icon: declare a path (`.icon("icons/extrude.svg")`) and mount
an asset source on the product
(`builder.with_assets(wb_example::example_assets())` — typically a static
`include_bytes!` table via `wb::StaticAssets`). Lookup falls back to the
platform standard icons (new/open/save/undo/redo and
diagnostics/plugins/settings/about under `wb-std/`), and a
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
    register_command("rhai.hello", "Rhai Hello", "hello");          // sync command
    register_background_command("rhai.progress", "Rhai Background Task", "progress_job");
    add_toolbar_item("plugin.rhai", "rhai.hello", "Hello");         // Ribbon contribution
    register_panel("rhai.panel", "Rhai Demo", "ui/main.rhai", "right"); // script UI panel
}

fn hello(args) { `Hello from the Rhai script ({args.name})` }      // returns the note string

fn progress_job(task, args) {                                      // background progress + cooperative cancel
    for i in 0..20 {
        task.check_cancelled();
        task.report(i as float / 20.0, `Batch {i + 1}/20`);
    }
}
```

The `ui/main.rhai` script panel returns a declarative UiNode tree composed of
registry components (button/input/label/tag/progress/card/…), with state in
the script and callbacks as named functions — see `plugins/rhai-demo/ui/`.
Manifest validation, API version checks, error isolation, and runtime disable
are all built in, and `requires` puts a plugin behind a product feature.

## Running & diagnostics

| Command | Meaning |
|---|---|
| `--selfcheck` | Headless selfcheck (probe-based: items unrelated to the product SKIP automatically), exit code 0/1 |
| `--selfcheck-crash` | Crash-recovery probe (invoked by the selfcheck; not for manual runs) |
| `--smoke [secs]` | Run for the given number of seconds and exit (default 2.5), for CI smoke tests |
| `RUST_LOG=info` | What reaches stderr (errors only by default); when set it also replaces the log file's filter |

A product that calls `diagnostics::init` keeps these in its config directory
(`%APPDATA%\<product id>` on Windows, `~/.config/<product id>` elsewhere):

- `logs/app.log`: warnings and up, plus information from the workbench crates.
  It rolls over at 5 MB and three older files are kept.
- `crashes/`: a report for each panic, with the location, the backtrace and the
  end of the log. The next launch says so once, and the newest 20 are kept.
  Native crashes (an access violation in a driver, say) are not captured yet.

**Help → Export Diagnostics…** saves one plain text file for support: versions,
system, plugins, platform settings, the end of the log and the latest crash
reports. Account details and plugin settings stay out; the log can contain file
paths. Nothing is sent anywhere unless the product injects a crash reporter and
the user turns on its setting.

## Continuous integration

`.github/workflows/ci.yml` runs on pull requests and pushes to master: tests and
selfcheck on Windows, Linux and macOS, a wasm32 check, and `cargo deny` against
`deny.toml` (licenses that put no limit on commercial use, known sources). Linux
and macOS report without blocking until they have been green once. Packaging,
code signing and updates are left to products.

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
- **SVG icons**: GUI-agnostic icon contract (`AssetSource` + icon fields on commands/views/panels), platform standard icon set, alpha-mask tinting and texture caching (design.md §9.2.1).
- **Web slice (wasm32 + trunk)**: gpui in the browser via `gpui_web` (WebGPU), fonts injected before first frame.
- **Single frontend (2026-10)**: egui frontend removed; crates renamed (`workbench-ui`, `product-demo`, `product-web`); script plugins moved from Python to Rhai (see docs/plan.md §15).
- **gpui-fast base (2026-11)**: gpui-fast from git replaces the gpui-ce fork; GPU viewports on DirectComposition (Windows) and a GPUI dynamic texture (web).
- **Files and state**: storage providers behind async open/save, in-app file picker, OPFS and import/export on the web, app state behind a `StateStore`.
- **Data safety**: atomic saves, versioned document formats with upgrades on opening and the original kept on the first save over it.
- **Diagnostics**: rolling log file, crash reports, Export Diagnostics, opt-in crash reporting.
- **Entitlements**: product-defined features gating commands, views, panels and plugins, following changes while the app runs.
- TODO: process isolation for untrusted plugins (design sketch in plan.md section 10.6), native crash capture.

## Publishing (crates.io)

**Publishing is on hold.** The gpui base currently resolves to gpui-fast
from git (and wgpu to the git revision gpui-fast pins), so a crates.io release
of the `workbench-*` crates would not build from the registry alone. Once a
gpui-fast release carries what the viewports need, publishing resumes
in dependency order (`workbench-api` → `workbench-core` → the rest);
`workbench-rhai` and `workbench-ui` are the current publishable set,
`product-demo` / `product-web` stay `publish = false`.
