# workbench-rs Development Plan (vertical slice M1 and later phases)

Implements docs/design.md. This plan merged Phase 0 (GUI validation) and
Phase 1 (minimal platform loop) into one runnable vertical slice; later
sections record the completed phases 2, 3, 5, RibbonBar, and release prep.

## 1. Goals and scope of the vertical slice

### 1.1 Goals

Validate the core architecture with one complete slice:

1. platform core fully decoupled from the GUI: Workbench modules depend only
   on `workbench-api`, never on egui/gpui (acceptance #21);
2. **the product fixes its GUI at build time**: `product-egui-demo` and
   `product-gpui-demo` are separate binaries, each linking exactly one
   frontend (acceptance #1/#22/#23);
3. main window: **toolbar (RibbonBar stand-in), dock panels on four sides,
   central multi-tab area, status bar / task feedback** (acceptance #3);
4. Workbench registers **commands, toolbar groups, dock panels, central views,
   document types, services** (acceptance #4);
5. unified command entry: toolbar, hotkeys, and in-view interactions all go
   through the command system (acceptance #5);
6. commands support **sync (immediate)** and **background (threaded)**
   execution; background tasks have progress, cooperative cancellation, and
   terminal states (acceptance #14/#15/#16);
7. background tasks write back through **document version checks + controlled
   commits**, never touching UI state directly (acceptance #17);
8. reversible commands support **undo/redo**; irreversible ones are marked
   (acceptance #18);
9. layout persists/restores with **stable IDs**; entries referencing missing
   modules are skipped, never breaking startup (acceptance #8/#9);
10. views render through a **GUI-agnostic PaintBackend**; ordinary views never
    touch wgpu (acceptance #10).

### 1.2 Explicitly out of scope (later milestones)

| Item | Reason / plan |
|---|---|
| RibbonBar (Tab/Group/Contextual Tab) | replaced by a simple toolbar per product decision; the toolbar model is Ribbon-shaped (Group→Items, stable IDs, command binding), so the upgrade is smooth |
| wgpu render service | phase 2+; PaintBackend already reserves the "capability declared per view" slot |
| Python plugins (pyo3 Host API) | phase 3; interfaces the Host API will reuse are fixed first |
| floating panels, cross-area drag-docking | egui_tiles already provides tree docking; validate fixed four areas + in-area tabs first |
| menu bar, command palette | together with RibbonBar |
| Rust dynamic-library plugins | non-goal (design.md §3.2) |

### 1.3 Technology choices (confirmed)

- **egui side**: `egui` + `eframe` (window/event loop) + `egui_tiles` (tree
  docking/tab containers).
- **gpui side**: `gpui-ce` 0.2.x (community crates.io fork, declared as
  `gpui = { package = "gpui-ce" }`) + `gpui-ce-platform` (font-kit) +
  `gpui_ce_components` 0.2.0 (official README combination).
- The platform core has zero GUI dependencies; only `serde/serde_json` (layout
  persistence) and `rfd` (system file dialog, OS-level infrastructure).

## 2. Workspace structure and dependency direction

```text
crates/
  workbench-api        contracts: IDs, geometry/color, PaintBackend, Registry,
                       contribution definitions, ServiceHost (docs/tasks/workspace/logs),
                       ViewInstance, CommandCtx, Workbench trait
  workbench-core       runtime: product assembly, command registry + execution, task pump,
                       context snapshots, layout persistence, platform commands
                       (new/open/save/undo/redo/close tab)
  workbench-ui-egui    egui frontend: renders the workspace model + PaintBackend impl
  workbench-ui-gpui    gpui frontend: same
workbenches/
  wb-example           example Workbench: depends only on workbench-api (acceptance #21)
products/
  product-egui-demo    depends on core + ui-egui + wb-example
  product-gpui-demo    depends on core + ui-gpui + wb-example
```

Dependency direction (design.md §5.1):

```text
product-* -> ui-*  -> core -> api
product-* -> wb-example -> api     (never through ui-* / egui / gpui)
```

## 3. Key design decisions

### 3.1 View presentation: GUI-agnostic PaintBackend (design.md §9)

`ViewInstance::paint(&mut self, painter: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx)`.
PaintBackend offers fill_rect / stroke_rect / line / circle / text /
measure_text; the egui adapter implements it over `egui::Painter`, the gpui
adapter over a canvas element + paint_quad/paint_text. Views own their hit
testing (the cancel button in a panel is view-drawn + click hit-test),
proving the abstraction supports common engineering-panel interactions.
Ordinary views never touch the GPU; the wgpu capability slots in later as a
parallel optional trait.

### 3.2 View instances vs core state self-borrowing

View instances (`Box<dyn ViewInstance>`) live inside core state (panel/tab
slots) yet painting needs `&mut` core state. Solution: **take-out/put-back**
(`mem::replace` with a placeholder) before painting. Identical on egui/gpui.

### 3.3 Command system (design.md §10)

- `CommandDef { id, title, hotkey, enabled: Option<EnableFn>, kind }`;
- `kind = Sync(handler)`: executes immediately on the UI thread;
  `kind = Background(collect)`: collects input on the UI thread and returns a
  TaskSpec; the core dispatches it to a background thread;
- background tasks report through `TaskCtx` (stage/progress/log/check_cancelled)
  with terminal states Succeeded/Failed/Cancelled (§10.2 state machine);
- task results carry `DocCommit { doc_id, expect_revision, label, content }`;
  the core validates the **document version** inside the frame_tick event pump
  before committing (§10.5), rejecting stale revisions with a log entry;
- enablement derives from `CtxSnapshot { active_document, active_view_type,
  work_mode }`; switching the central tab updates the context and toolbar
  button states (acceptance #7).

### 3.4 Documents and undo (design.md §12.1/12.2)

The platform owns the document lifecycle (create/open/save/dirty flag/
revision/title); domain content is stored type-erased as
`Box<dyn DocumentContent>`; Workbenches register
`DocumentTypeDef { extensions, create_default, serialize, deserialize }`.
Undo is snapshot-based (`before/after` content clones) — sufficient for the
slice; the "no forced full copy" requirement remains an optimization point for
large documents (history entries can become deltas).

### 3.5 Layout persistence (design.md §6.5)

`layout.json` (product config dir): panels (stable ID → area/order/size/
visible), central tabs (view type ID + title), active tab, per-area active
page, work mode. On restore, unregistered IDs are skipped and logged.

### 3.6 Toolbar model (precursor of the RibbonBar)

`ToolbarGroup { id, title, items: Vec<ToolbarItem::Command { command, label? }> }`
— isomorphic to the Ribbon's Group→Item; upgrading to a full RibbonBar only
adds the Tab dimension and widget kinds, keeping command binding and state
mechanics unchanged.

## 4. Milestones and task breakdown

| # | Milestone | Content | Verification |
|---|---|---|---|
| M0 | skeleton | workspace, crates + products, dependency resolution (gpui-ce combo / egui_tiles) | `cargo check` |
| M1 | api contracts | IDs/geometry/PaintBackend/Registry/ServiceHost/task & document models | compile + unit tests |
| M2 | core runtime | assembly, command execution, task pump, snapshots, platform commands, layout persistence | `cargo test` (headless) |
| M3 | wb-example | doc type + 6 demo commands + 4 panels + 2 central views | selfcheck |
| M4 | egui frontend | workspace rendering, PaintBackend, hotkeys, egui_tiles docking | compile + smoke |
| M5 | gpui frontend | same (gpui-ce + components combo) | compile + smoke |
| M6 | wrap-up | full tests, headless `--selfcheck`, `--smoke` (2s auto-exit), docs | summary report |

## 5. Acceptance mapping (design.md §17 → this slice)

- #1/#22/#23 → two independent product crates, each with exactly one GUI;
- #3/#4/#5 → toolbar + dock + central tabs + unified command entry;
- #6/#7 → open/activate/reorder/close tabs; switching refreshes CtxSnapshot and button states;
- #8/#9 → layout.json stable-ID persistence, unknown IDs skipped;
- #10 → PaintBackend, ordinary GPU-free views;
- #14/#15/#16 → Sync/Background commands, progress, cancellation, terminal states, status bar;
- #17 → DocCommit + revision check;
- #18 → snapshot undo/redo + irreversible commands marked (logged, not in history);
- #19/#20 → plugin mechanism deferred; Registry keeps initialization error isolation
  (catch_unwind) and the API version field;
- #21 → wb-example's Cargo.toml has zero GUI dependencies (assertable in CI).

## 6. Risks and mitigations

| Risk | Mitigation |
|---|---|
| gpui-ce API drift | `cargo fetch` first, read the real sources under `~/.cargo/registry/src`, compile a minimal window skeleton before adding features |
| gpui_ce_components / gpui-ce version mismatch splitting types | all deps in workspace.dependencies; assert single instance via `cargo tree -i gpui-ce` |
| egui_tiles conflicting with the workspace model | core stores the "logical layout" (panel→area); egui_tiles is only the egui-side physical layouter; gpui uses hand-rolled simple layout — pixel parity not required (§6.6) |
| over-abstracted platform | strictly trim to the acceptance checklist; no interface without a UI need |

## 7. Phase 2 record (async commands & transactions, done)

Implements design.md §18 phase 2 and §10. Built on the M1 vertical slice:

### 7.1 Three-tier command execution (complete §10.6 invocation path)

| Tier | API | Carrier | Use |
|---|---|---|---|
| Sync | `CommandKind::Sync` | UI thread, immediate | lightweight ops (<16ms) |
| Async (new) | `CommandKind::Async` → `AsyncSpec{ job: TaskCtx → BoxFuture }` | **platform shared async executor** (`workbench-core::executor::AsyncExecutor`, async-executor + 2 workers) | light concurrent IO/polling, `Timer::after` awaits, no dedicated thread |
| Background | `CommandKind::Background` → `TaskSpec{ job: &mut TaskCtx → Result }` | dedicated thread per task | heavy CPU / blocking IO |

### 7.2 Unified command invocation results (§10.1/§16.4)

- `execute_command_tracked() → CommandInvocation { record, result, task_id }`:
  sync commands reach a terminal state immediately; async/background return a task handle;
- the platform keeps a **ring-buffer invocation history** (100 entries):
  `recent_invocations()` exposes execution/status/task_id/note per invocation;
- terminal task events automatically advance the matching invocation record;
  unknown commands also leave a Failed record.

### 7.3 Document transactions (§10.5/§12.2)

- `DocCommit { expect_revision }` version-checked commit (from M1); this phase
  adds the `DocumentService::revision()` query API;
- stale revisions / closed documents are always rejected with a warning log
  (policy: Reject; other policies remain a domain extension point).

### 7.4 Verification

- 5 new unit tests: async completion + record terminal state, async
  cooperative cancel, executor concurrency (4 tasks / 2-thread pool), ring
  history + unknown-command record, revision API;
- selfcheck extended to **12 items**; egui/gpui both 12/12;
- both GUIs `--smoke` pass with clean process exit (executor threads exit via
  shutdown flag; product entry `process::exit(0)` as a backstop);
- demo adds an "Async scan (executor)" command; task panel shows the carrier
  tag (`[async]`/`[thread]`).

## 8. Phase 3 record (Python plugin loop, done)

Implements design.md §11 and §18 phase 3. New crate `workbench-python`
(pyo3 0.29 + auto-initialize).

### 8.1 Plugin model

| Aspect | Implementation |
|---|---|
| manifest | `plugin.toml` (§11.2 fields), toml parsing + validation (empty ID, bad chars, entry format) |
| discovery | `discover(dirs)`: subdirectories of the search dirs containing `plugin.toml`; product paths = repo `plugins/` + `%APPDATA%/<product-id>/plugins` |
| version check | `api_version != "1"` (`HOST_API_VERSION`) → refuse with a readable reason (acceptance #20) |
| lifecycle | importlib module load → call `register(api)` → collect requests → apply to `Registry` atomically; duplicate IDs skipped |
| error isolation | manifest/load/execution failures land in `plugins_rejected` (reason queryable); the app keeps running (acceptance #19) |
| source tagging | `CommandDef::source` / `ToolbarGroup::source`; `remove_commands_from_source()` supports unload cleanup (§16.2) |

### 8.2 Host API v1 (subset of design.md §11.3/§11.4)

```python
def register(api):
    api.log_info("...")
    api.register_command(id, title, callback)                    # sync: UI thread, may return a note string
    api.register_command(id, title, callback, background=True)   # background: callback(task, args) on a task thread
    api.add_toolbar_item(group, command, label=None)             # toolbar contribution (group auto-created)
    api.api_version                                              # host API version
# task object: report(fraction, stage) / set_stage / log / check_cancelled() (raises PluginCancelled) / is_cancelled
```

- Python callbacks never hold a registry borrow — requests are collected
  (`Collected`) and applied after `register` returns (duplicate ID / source
  validation via the platform path);
- cooperative cancellation: `task.check_cancelled()` raises `PluginCancelled`,
  mapped by the host to the Cancelled task state;
- GIL boundary: sync plugin commands attach on the UI thread; background
  commands attach on the task thread (CPython releases the GIL periodically —
  heavy jobs should use the background tier).

### 8.3 Demo plugin and verification

- `plugins/demo-plugin/`: sync command `demo.hello` (note into invocation
  history) + background progress task `demo.python_progress` + toolbar group
  `plugin.demo`;
- selfcheck extended to **14 items**; egui/gpui both 14/14;
- `workbench-python` dedicated tests, **9 items**: manifest parse / version
  reject / ID validation, good-plugin load with source tag, broken-syntax
  isolation, duplicate-ID skip, sync note into history, Python exception →
  command Failed, background completion and cooperative cancel, source removal
  cleanup;
- runtime requirement: the Python interpreter directory must be on PATH (this
  machine: `C:\ProgramData\miniforge3`); the build interpreter is set via
  `PYO3_PYTHON` in `.cargo/config.toml`.

## 9. RibbonBar record (design.md §6.1/§7, done)

Originally a simple toolbar per product decision; upgraded to a full RibbonBar.

### 9.1 Platform model (workbench-api)

- `RibbonTab { id, title, groups: Vec<ToolbarGroup>, source }` — the Tab
  dimension layered over the existing Group→Item model; command binding and
  enablement mechanics unchanged;
- `WorkspaceState::add_ribbon_tab()`: same-ID Tabs merge groups, same-ID
  groups merge items (controlled contribution, §7.3);
- `add_toolbar_group()` becomes the compatibility entry: groups land in the
  default Tab `app.home` ("Home") — platform commands and plugin
  `add_toolbar_item` migrate with zero changes;
- `active_ribbon_tab` persists in `LayoutFile` (old layout files fall back to
  the first Tab; unregistered Tabs fall back with a diagnostic);
- `Registry::add_ribbon_tab()`: collected during assembly; `build()` registers
  explicit Tabs first, then legacy groups.

### 9.2 Frontend rendering (egui / gpui)

Both render a two-band Ribbon: Tab row + Group row of the active Tab. Active
Tab highlighted (egui outline / gpui underline), commands horizontally laid
out, group captions centered under the command row, group separators, hover
tooltips (command + hotkey), live enablement.

wb-example organizes two Tabs: "Home" (file/edit/shape groups + platform
"Window" group + plugin `plugin.demo` group) and "Tools" (view/demo).

### 9.3 Verification and fixes

- 2 new Ribbon unit tests (Tab merge / legacy compat / active Tab switch /
  layout round-trip / unregistered Tab fallback); core 18, python 9, all green;
- selfcheck extended to **15 items**; egui/gpui both 15/15;
- **real rendering confirmed via GPU readback screenshots**: Tab rows, 5
  command groups, group captions, disabled buttons (undo/redo greyed by
  context), plugin-contributed group, and CJK text all correct.

### 9.4 Debugging notes (valuable lessons)

1. **GDI screenshots cannot read GPU windows**: `CopyFromScreen` (white) and
   `PrintWindow` (black) both miss DirectX/GL swapchains, causing a long
   "blank window" misdiagnosis. Reliable methods: (a) the
   `EFRAME_SCREENSHOT_TO=<path>` env var (built-in eframe GPU readback, PNG on
   exit); (b) egui_kittest offscreen rendering (used once, then removed).
2. **egui lacks CJK glyphs**: built-in fonts have no Chinese; system font
   fallbacks now load (msyh.ttc etc., 7 candidate paths, cross-platform).
3. **egui reactive rendering vs smoke exit**: when the UI is idle egui emits
   no frames, so `logic()` stops being called and the smoke Close never fires
   (the old hard-coded 2.5s fit inside the startup activity window and masked
   this). Fix: `request_repaint_after(200ms)` during smoke; also `--smoke`
   now accepts a duration (`--smoke [secs]`, default 2.5) — the value was
   previously ignored.
4. eframe switched to the **glow backend** (wgpu presentation also appeared
   blank locally with zero diagnostics; glow is egui's traditional backend
   with a more conservative path).

## 10. Phase 5 record (production hardening, first batch, done)

Implements design.md §18 phase 5. Landed 5 items trimmed by real product need,
1 item design-reserved.

### 10.1 Settings service (§12.4)

- `api::Settings`: flat dotted-key JSON persistence (`<config dir>/settings.json`),
  write-through; first consumer: `autosave.interval_secs` (default 30, min 5).

### 10.2 Autosave + crash recovery (§12.1/§18)

- session marker: `<config dir>/session.clean` — deleted at build, written on
  Drop; missing means abnormal exit;
- autosave: `frame_tick` serializes **dirty documents** into
  `<config dir>/autosave/` (content file + JSON sidecar metadata) on interval,
  without touching document dirty/path state;
- startup recovery: without a clean marker, autosaved documents reopen with a
  `[Recovered]` title prefix and dirty=true (existing "save failure keeps
  unsaved state" semantics intact); the directory is cleaned afterwards;
- `app.autosave.now` command for manual trigger; the selfcheck validates the
  full chain via a **subprocess bootstrap** (session A dirty doc → autosave →
  delete marker → session B recovery).

### 10.3 Plugin runtime management (§16.2/§18)

- `AppServices::disable_plugin(id)`: removes plugin commands
  (`Registry::remove_commands_from_source`), cleans Ribbon groups/Tabs
  (`WorkspaceState::remove_commands`), updates the `host.plugins` mirror;
- platform command `app.plugins.disable` (args: id);
- platform "Plugins" panel: lists Loaded/Rejected/Disabled with reasons;
  Loaded entries expose a Disable button (panel-drawn + hit-tested).

### 10.4 Diagnostics panel + frame statistics (§16.3/§18)

- `ServiceHost::record_frame(dt_ms)`: reported by the frontend every frame
  (egui logic / gpui render), maintaining frame count and EMA frame time;
- platform "Diagnostics" panel: GUI backend label
  (`builder.with_backend_label`), uptime, frame stats, document/unsaved
  counts, background tasks, autosave state and interval.

### 10.5 Automation & performance tests (§16.4/§18)

- `core/tests/perf.rs`, 3 items: 10k sync command dispatches (µs-level each),
  200 reversible edits + full-capacity undo/redo on a 20k-element document,
  100 concurrent background tasks through the pump — with loose magnitude
  assertions against regressions;
- `scripts/verify.ps1`: check → full tests → both selfchecks → both smokes in
  one shot (handles the Python PATH);
- `[profile.release]` tuning (thin LTO, codegen-units=1, strip debuginfo).

### 10.6 Design reservations (not implemented; trigger conditions and shape)

| Item | Trigger | Sketch |
|---|---|---|
| process isolation for untrusted plugins (§11.5 "if needed") | a real need to run untrusted plugins | manifest gains `sandbox = "process"`; product binary gains a `--plugin-worker <dir>` subprocess mode embedding pyo3; host forwards register/call/progress over stdio JSON-RPC; command calls reuse the existing InvocationRecord channel |
| plugin source management & updates | once a distribution channel exists | `plugin.toml [source]` + local repo directory version comparison (no network dependency initially) |
| GPU resource recovery / view diagnostics | after the wgpu render service lands (§9.4) | render service owns device lifecycle; diagnostics panel gains GPU adapter / per-view resource entries |
| multi-platform packaging | once multi-platform distribution is needed | cargo-dist or per-target scripts; the Windows package is already producible via release profile + verify.ps1 |

## 11. Open-source release preparation (done)

- License converged to **MIT** (workspace `license` + root [LICENSE](LICENSE));
- all publishable crates carry full metadata: description/keywords/categories/
  repository/readme;
- crates.io name availability confirmed (workbench-api/core/python/ui-egui/
  ui-gpui/example all free);
- `wb-example` package renamed to `workbench-example` (lib name stays
  `wb_example`, zero code changes);
- internal dependencies in workspace.dependencies gained `version` (path +
  version dual declaration, required for publishing);
- both example products `publish = false`;
- `cargo package -p workbench-api` (including build verification) passes; the
  other crates' package failures are only the "dependency not yet published"
  ordering constraint — the README "Publishing" section documents the order
  (api → core → the rest);
- eframe drops the internal debug feature `__screenshot` (the verification
  method is preserved as a README note);
- leftovers cleaned: selfcheck `_path_use` dead code, `Check::skip` redundant allow;
- `.cargo/config.toml` `PYO3_PYTHON` gains cross-platform probing notes
  (machine-specific value; each machine adjusts after open-sourcing).
