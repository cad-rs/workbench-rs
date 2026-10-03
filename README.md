# workbench-rs

> Repository: https://github.com/cad-rs/workbench-rs

A reusable Rust framework for generic engineering desktop applications (CAD,
mechanical design, industrial simulation, and similar products).
Requirements and architecture: [docs/design.md](docs/design.md). Development
plan and phase records: [docs/plan.md](docs/plan.md).

License: [MIT](LICENSE). All library crates are published to crates.io under
the `workbench-*` names (see "Publishing" at the end).

## Repository layout

```text
crates/
  workbench-api         GUI-agnostic platform contracts: IDs, geometry, PaintBackend,
                        commands, tasks, documents, workspace/Ribbon model
  workbench-core        Platform runtime: assembly, command dispatch, tasks/async executor,
                        autosave, layout persistence, selfcheck
  workbench-ui-egui     egui frontend adapter (glow backend + egui_tiles docking)
  workbench-ui-gpui     gpui frontend adapter (gpui-ce + official component library)
  workbench-python      Python plugin host (pyo3, Host API v1)
workbenches/
  wb-example            Example Workbench module (package name workbench-example,
                        depends only on workbench-api)
products/
  product-egui-demo     Example product A (egui selected at build time)
  product-gpui-demo     Example product B (gpui selected at build time)
plugins/
  demo-plugin           Demo Python plugin (sync command + background progress task + toolbar contribution)
docs/                   Requirements / plan / plugin authoring guide
```

Dependency direction: `product → ui → core → api` and `product → wb-example → api`.
**Workbench modules never touch egui / gpui / wgpu** — GUI code lives only in
`workbench-ui-*`.

## Quick start

```bash
cargo run -p product-egui-demo          # run the two example products
cargo run -p product-gpui-demo
cargo test --workspace                  # full test suite
powershell -File scripts/verify.ps1     # one-shot verification (check + tests + selfcheck + smoke)
```

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
        .with_backend_label("egui")                       // shown in the diagnostics panel
        .with_workbench(my_cad_workbench)                 // assemble Workbench modules
        .with_plugin_stage(workbench_python::plugin_stage(
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
            workbench_ui_egui::run(builder, "My Product").unwrap();
            std::process::exit(0);
        }
    }
}
```

A complete runnable version lives in `products/product-egui-demo/src/main.rs`
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
        reg.register_command(
            wb::CommandDef::sync("cad.add_box", "Add Box", |ctx, _args| {
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
        reg.register_view(wb::ViewDef::new("cad.viewport", "3D View", true, || Box::new(ViewportView)));
        // Document type (serialization owned by the domain module, the platform
        // never reaches into the data model)
        reg.register_document_type(wb::DocumentTypeDef { ... });
    }
}
```

Key points:

- Views draw through `PaintBackend` (six primitives: fill/stroke/line/circle/text)
  and never touch GUI types;
- Commands have three execution tiers: `Sync` (UI thread) / `Async` (shared
  executor) / `Background` (dedicated thread task with progress + cancellation);
- Document edits go through `DocumentService::edit/commit`; background results
  come back as `DocCommit` (version-checked) — undo/redo work automatically;
- `init` may panic — the failure is caught and isolated during assembly,
  without taking the product down.

### 3) Plugin developers: Python extensions

See [docs/plugins.md](docs/plugins.md) and the runnable example
`plugins/demo-plugin/`. Host capabilities (Host API v1): sync/background
commands, progress with cooperative cancellation, Ribbon toolbar contributions;
manifest validation, API version checks, error isolation, and runtime disable
are all built in.

## Running & diagnostics

| Command | Meaning |
|---|---|
| `--selfcheck` | Headless selfcheck (probe-based: items unrelated to the product SKIP automatically), exit code 0/1 |
| `--selfcheck-crash` | Crash-recovery probe (invoked by the selfcheck; not for manual runs) |
| `--smoke [secs]` | Run for the given number of seconds and exit (default 2.5), for CI smoke tests |
| `RUST_LOG=info` | wgpu/eframe diagnostics |
| stderr `wb-log [...]` | Mirror of the platform log (visible in the terminal, greppable in automation) |

> Note: GDI screenshots (PrintWindow/CopyFromScreen) cannot capture GPU swapchain
> content. To verify egui rendering, run with the `EFRAME_SCREENSHOT_TO=<path>`
> environment variable plus `--smoke` (GPU readback, PNG written on exit).

## Python plugins

`plugins/demo-plugin/` is a runnable example. Python must be on PATH before
running the products (on this machine: `C:\ProgramData\miniforge3`); the
interpreter used at build time is configured via `PYO3_PYTHON` in
`.cargo/config.toml`. Plugin authoring: [docs/plugins.md](docs/plugins.md).

## Milestones (mapped to design.md §17 acceptance items)

- **Phase 1 (vertical slice)**: acceptance items #1–#10, #12–#18, #21–#23 prototyped and verified.
- **Phase 2 (async commands & transactions)**: three execution tiers (Sync / Async shared executor / Background thread), unified command invocation results with a queryable invocation history, `DocCommit` version-checked transactions, document revision API.
- **Phase 3 (Python plugin loop)**: manifest & discovery, API version check, Host API v1, error isolation, runtime disable, demo plugin.
- **RibbonBar**: three-level model `RibbonTab -> Group -> Command` (stable IDs, controlled merging, active-Tab persistence), rendering verified on both frontends via GPU readback screenshots.
- **Phase 5 (production hardening, first batch)**: settings service, autosave + crash recovery, plugin management panel, diagnostics panel with frame statistics, performance regression tests + one-shot verification script.
- TODO: wgpu render service (optional capability), process isolation for untrusted plugins (design sketch in plan.md section 10.6), multi-platform packaging.

## Publishing (crates.io)

Publishable crates: `workbench-api`, `workbench-core`, `workbench-python`,
`workbench-ui-egui`, `workbench-ui-gpui`, `workbench-example` (all MIT, full
metadata; `cargo package -p workbench-api` passed including build verification).
The two example products are not published (`publish = false`).

Internal dependencies carry explicit versions, so publishing must follow the
dependency order:

```bash
cargo publish -p workbench-api
cargo publish -p workbench-core
cargo publish -p workbench-python   # then workbench-example / ui-egui / ui-gpui (any order)
```

Run `powershell -File scripts/verify.ps1 -SkipSmoke` right before publishing as
a final check.
