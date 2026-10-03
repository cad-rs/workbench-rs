# workbench-rs: Requirements & Design for a Generic Engineering Desktop Application Framework

- **Project name:** `workbench-rs`
- **Document status:** initial design baseline
- **Target language:** Rust
- **Target application types:** CAD, mechanical design, industrial simulation,
  multiphysics simulation, and other professional engineering desktop applications
- **GUI candidates:** egui, GPUI
- **Product GUI rule:** each shipped product fixes exactly one GUI at development/build time;
  end users cannot choose or switch GUIs
- **Optional rendering capability:** `wgpu`
- **User plugin language:** Python

---

## 1. Purpose

This document defines the requirements and architecture of `workbench-rs`.

`workbench-rs` is a reusable generic Workbench framework for developing multiple
**independent professional desktop applications**. It provides common desktop
application infrastructure, a Workbench composition mechanism, and plugin
extension points — but ships no domain capabilities such as CAD kernels,
assembly, meshing, materials, or solvers.

Product teams pick the Workbench modules they need and compose them into a
standalone application; end users extend the product through Python plugins.

## 2. Core goals

### 2.1 Product goals

`workbench-rs` should support developing multiple independent products, e.g.:

- an extensible CAD application in the spirit of FreeCAD;
- a mechanical design application in the spirit of SolidWorks;
- a simulation & post-processing application in the spirit of COMSOL.

These are product-direction examples; they do not imply the framework itself
provides CAD kernels, simulation capabilities, or file-format compatibility
matching those products.

### 2.2 Framework goals

The framework shall provide:

1. a composable Rust Workbench module mechanism;
2. the classic engineering desktop UI: RibbonBar, dock panels around a central
   multi-tab content area;
3. extension points for commands, menus, RibbonBar, hotkeys, panels, views, and
   document types;
4. platform APIs decoupled from any specific GUI framework;
5. multiple ways of presenting views; `wgpu` is an optional rendering
   capability, not a requirement for all views;
6. on-demand access to a controlled `wgpu` rendering context for Rust view extensions;
7. a command system supporting synchronous, asynchronous, and background execution;
8. a unified task lifecycle with progress, cancellation, and error reporting;
9. discovery, loading, lifecycle management, and extension capabilities for Python plugins;
10. common application services: documents, tasks, settings, logging, events;
11. GUI frontend selection and Workbench module composition at product build time;
12. multiple independent products sharing platform code while keeping their own
    product configuration and distribution.

### 2.3 Design principles

- **The platform contains no domain semantics.** The platform does not define
  CAD features, physical fields, material models, or solver models.
- **Products compose modules explicitly.** The product decides which
  Workbenches, plugin capabilities, and default layouts are enabled.
- **Domain modules cooperate through stable interfaces.** Avoid cross-domain
  access to each other's internals.
- **GUI is replaceable; the product's GUI is fixed.** Each product builds and
  ships exactly one GUI frontend.
- **Rendering is chosen per view.** Ordinary panels and content views do not
  need `wgpu`.
- **Command entry points are decoupled from execution.** The RibbonBar, menus,
  hotkeys, and Python plugins all execute through the same command system.
- **Long tasks never block the UI.** Background tasks submit results via
  messages or controlled commands.
- **Static composition first, dynamic extension later.** Rust Workbenches are
  composed via Cargo at build time in the early stages; Python plugins provide
  runtime extension.
- **Plugin capability boundaries are explicit.** In-process Python plugins are
  not treated as a security sandbox.

## 3. Scope and non-goals

### 3.1 Framework scope

`workbench-rs` is responsible for:

- application lifecycle, product configuration, and Workbench module composition;
- GUI frontend adaptation;
- RibbonBar, menus, hotkeys, and the command palette;
- dock panels, the central multi-tab content area, and layout management;
- view registration, lifecycle, and presentation-mode extension;
- optional `wgpu` rendering infrastructure;
- generic document & project facilities;
- command execution, undo/redo, and transaction boundaries;
- background tasks, progress, cancellation, and logging;
- plugin manifests, discovery, loading, error handling, and API version management;
- settings, diagnostics, events, and test facilities.

### 3.2 Non-goals

The generic framework does NOT cover:

- CAD geometry kernels, B-rep, sketch constraints, feature trees, or topological naming;
- assembly solving, drawings, manufacturing, or industry-specific workflows;
- FEM, CFD, multiphysics, or other solvers;
- mesh generation, material databases, or physical field models;
- file formats fully compatible with any commercial product;
- a default security sandbox for Python plugins;
- a Rust dynamic-library plugin ABI in the early stages;
- guaranteeing identical widget implementations, visuals, or internal state
  between egui and GPUI;
- forcing all views or panels onto `wgpu`;
- letting Python plugins touch raw `wgpu` resources.

## 4. Definitions

### 4.1 Platform

The common application runtime and its APIs provided by `workbench-rs`:
commands, context, document infrastructure, plugin lifecycle, background
tasks, persistence services, workspace, and the GUI frontend interface.

### 4.2 Workbench module

A Rust feature module compiled into the product. It may register commands,
RibbonBar items, panels, views, document types, importers/exporters, and
services. A Workbench module is not necessarily a screen layout.

### 4.3 RibbonBar

The ribbon control at the top of the main window, composed of Tabs, Groups,
and concrete actions/controls. Workbenches may contribute ribbon content; the
platform owns layout, command binding, and state management.

### 4.4 Dock Panel

A composable panel docked around the main window. It can be moved, resized,
collapsed, hidden, floated, or re-docked.

### 4.5 Central content tab

A tab instance in the central multi-tab area, hosting document editors,
engineering views, charts, tables, text, or other primary work content.

### 4.6 Perspective / work mode

A set of workflow and UI organization settings: which panels are enabled,
default layout, RibbonBar display policy, and view layout.

### 4.7 Product

A standalone desktop application built with `workbench-rs`. The product
selects the GUI frontend, Workbench modules, default work mode, product
branding, and distribution configuration.

### 4.8 Python plugin

A user extension discovered and loaded by the host at runtime. Plugins
contribute through a stable Python Host API and never depend on Rust internal
types, GUI framework widgets, or raw GPU resources.

## 5. Overall architecture

```text
+------------------------- standalone products -------------------------+
| Product A: egui selected at build time, composes the CAD Workbench     |
| Product B: GPUI selected at build time, composes Assembly / Drawing    |
| Product C: one GUI selected at build time, composes Simulation/Results |
| Product config: modules, Ribbon, layout, plugin policy, distribution   |
+--------------------------- Workbench layer ----------------------------+
| Sketch | Part | Assembly | Drawing | Simulation | Results              |
| contribute domain commands, Ribbon, panels, views, doc types, services |
+--------------------------- platform layer -----------------------------+
| plugin Host API | events | Ribbon model | workspace | settings/logging |
+--------------------------- GUI & rendering ----------------------------+
| egui or GPUI frontend (one per product, chosen at build time)          |
| windows, input, concrete Ribbon/Dock/tab adaptation                    |
| optional wgpu render service and view scheduling                       |
+--------------------------- system base --------------------------------+
| windows & input | GPU | filesystem | OS integration | crash diagnostics|
+------------------------------------------------------------------------+

Rust Workbench views --on-demand render APIs--> host render service
Python plugins --stable Host API--> generic platform
generic platform --UI model--> the product's chosen GUI frontend
```

### 5.1 Dependency direction

```text
Product composition
  +-- depends on the generic platform
  +-- depends on the chosen GUI adapter
  +-- depends on the chosen Workbench modules

Workbench modules
  +-- depend on the platform API and the domain APIs they explicitly declare

GUI adapters
  +-- depend on the platform UI model and the GUI framework

Render service
  +-- provides GPU resources and scheduling when the product enables it

Python plugins
  +-- depend on a versioned Python Host API
```

The platform core must not depend on any CAD, CAE, or other domain module.
Domain modules must not depend on the internals of another domain crate.

## 6. Main window and workspace layout

### 6.1 Main window structure

The main window uses the classic engineering desktop layout:

```text
+------------------------------------------------------------+
| RibbonBar                                                  |
+----------+-----------------------------------+-------------+
|          |       central multi-tab area      |             |
| left     |  [Doc A] [View B] [Doc C]         | right       |
| dock     |                                   | dock        |
| panels   |                                   | panels      |
+----------+                                   +-------------+
| bottom   |                                   |             |
| panels   |                                   |             |
+----------+-----------------------------------+-------------+
| status bar / task status / progress                        |
+------------------------------------------------------------+
```

The main window includes:

1. **Top RibbonBar** — the primary command entry point for the current product and work mode.
2. **Dock areas** — left, right, top, bottom docking regions of the main window.
3. **Central multi-tab area** — shows opened documents, editors, and views.
4. **Status & task area** — current context, background task status, progress,
   cancellation, and necessary notifications.

Dock panel contents and default positions are decided by product configuration
and Workbench modules. The platform does not require all products to share the
same initial layout.

### 6.2 Dock panels

The workspace manager shall support:

- docking in the left/right/top/bottom areas;
- combining multiple panels of one area into tabs;
- resizing, collapsing, hiding, and re-showing;
- floating as independent windows and re-docking;
- saving and restoring user layout;
- panel registration by Workbenches or plugins;
- default dock area, default size, and applicable work mode per panel;
- skipping stale layout entries with diagnostics when the owning module or
  plugin is unavailable.

Panels register under a stable `PanelId`; registration may include title,
icon, default dock suggestion, applicable context, and a creation entry point.
The platform owns panel lifecycle and layout management; the chosen GUI
frontend implements presentation.

### 6.3 Central multi-tab area

Central tabs may host: document editors; CAD or simulation domain views;
charts, tables, text, and reports; other product-defined primary content.

The platform manages open/activate/close/reorder/restore of content instances
and updates the current application context when the active tab changes.

The central area shall support: multiple documents or views open
simultaneously; tab switching with active-context updates; tab reordering;
closing with unsaved-state prompts; tab context menus; product-defined session
restore policy.

The first phase requires central multi-tab support only. Splitting the central
area into parallel tab groups is a later capability, not mandatory for the
initial architecture.

### 6.4 RibbonBar and workspace context

- Active document, active view, selection, and work mode form the current command context;
- the command system derives enabled/checked/visible state from the context;
- Workbenches contribute RibbonBar Tabs, Groups, and command items;
- the product may define default Tab order, default work mode, and layout;
- after a dock panel or central tab change, the platform updates the current context;
- the RibbonBar never mutates domain document state directly — everything goes
  through the command system.

### 6.5 Layout persistence

Layout persistence uses stable panel IDs, view type IDs, and content
identifiers — never GUI-framework-internal control IDs.

At minimum, persist: dock panel area, order, size, visibility, and floating
state; central tab order, active tab, and restorable content identifiers; the
current work mode; the active RibbonBar Tab (or product-permitted display
preferences).

When a plugin or Workbench is missing, the platform skips unrestorable layout
entries and allows the user to reset the layout.

### 6.6 GUI frontend relationship

The platform defines the GUI-agnostic RibbonBar, dock panel, central tab, and
layout models. The egui and GPUI frontends each implement their interaction
and drawing.

- Domain Workbenches do not depend on GUI framework internals;
- a single product builds and ships exactly one GUI;
- the two GUI implementations do not need identical layout details;
- both GUIs are verified against the same layout behavior acceptance tests.

### 6.7 View presentation

Views hosted in central tabs may use different presentation techniques.
Ordinary forms, property panels, charts, text, or tables do not require
`wgpu`. GPU-accelerated views may opt into the GPU rendering capability on
demand.

## 7. RibbonBar design

### 7.1 Goals

The RibbonBar is a first-class UI extension point of the main window. The
platform owns the generic structure and command binding; Workbench modules and
Python plugins contribute content through a stable API.

### 7.2 Structure model

```text
RibbonBar
  +-- Tab
       +-- Group
       |    +-- Command Button
       |    +-- Toggle / Check Button
       |    +-- Split Button
       |    +-- Menu Button
       |    +-- restricted declarative Controls
       +-- ...
       +-- Contextual Tab (optional)
```

The platform model shall support: stable IDs for Tabs and Groups; title, icon,
ordering, and grouping; command buttons bound to command IDs;
enabled/disabled/checked/visible state; split buttons and dropdown menus; Tabs
related to work mode, document type, or selection context; controlled
contributions to existing Tabs/Groups by Workbenches and plugins; compact
presentation or overflow when space is tight; hotkeys and keyboard navigation
conventions; product defaults and user display preferences.

### 7.3 Extension rules

- Ribbon items use stable IDs and bind commands by command ID;
- Python plugins contribute Tabs, Groups, or command items through the platform API;
- plugins must not modify the widget implementation registered by other modules;
- extending an existing Tab/Group requires declaring the target ID;
- a missing target produces a diagnosable registration result;
- command state and execution results come from the command system;
- the product may disable, reorder, or set default visibility of extension items.

### 7.4 Example

```rust
RibbonTab {
    id: "part.model",
    title: "Model",
    groups: vec![
        RibbonGroup {
            id: "part.create",
            title: "Create",
            items: vec![
                RibbonItem::Command { command: "part.create_box" },
                RibbonItem::Command { command: "part.create_cylinder" },
            ],
        },
    ],
}
```

The example expresses data-model intent only; it is not the final Rust API.

## 8. Workbench module design

### 8.1 Rust Workbench responsibilities

A Workbench module may contribute: commands, parameters, applicable context,
and execution logic; RibbonBar Tabs, Groups, command items, and hotkeys; menus
and context actions; dock panels and central content views; document types and
domain data services; importers, exporters, and file filters; work modes and
layout suggestions; background task kinds; diagnostics and in-product help
entries.

A Workbench module must not: mutate other modules' internal state directly;
assume it is included in every product; bypass the platform API to create
global UI; force domain objects into the platform's generic data model.

### 8.2 Rust module composition

In the first phase, Rust Workbenches are organized as Cargo workspace crates
and linked into the application at product build time. Product code assembles
the enabled modules explicitly.

## 9. Views and rendering

### 9.1 Goals

The platform defines generic view registration, lifecycle, layout, and context
mechanisms, but does not require all views to share one rendering technology.

A view may choose: the native widgets of the current GUI frontend; the drawing
interface provided by the GUI frontend; a CPU-drawn canvas or image; `wgpu`
GPU drawing; other specialized rendering backends added later.

The platform models "view" and "renderer" separately.

### 9.2 Generic view extension points

A view extension may: register a stable view type ID; declare name, icon,
applicable document types, and a creation entry point; create, update, and
destroy view instances; receive size, scale, focus, input, and context
changes; declare the presentation mode or render capability it needs; use the
host API of the corresponding presentation mode; report initialization,
update, and drawing errors.

Ordinary views never implement GPU initialization, device access, or render
lifecycle callbacks.

### 9.3 Rendering capabilities

Rendering capability is declared per view and provided by the platform or the
product. Capabilities may include: GUI native widgets; the GUI frontend
drawing interface; CPU image or canvas drawing; `wgpu` GPU drawing; other
rendering backends added later.

The product may mark a view unavailable based on target platform, performance
requirements, and enabled capabilities. When a capability is unavailable, the
platform provides diagnostics or lets the view choose a supported fallback.

### 9.4 `wgpu` view extension

Rust views requiring GPU rendering may declare the `wgpu` capability and
access a controlled context in the host-provided rendering lifecycle. The
context may contain: `wgpu::Device` and `wgpu::Queue`; the render target of
the current view or a platform-defined drawing interface; viewport size,
scale, surface format, and other metadata; resource creation, caching, and
device-recovery services; rendering diagnostics.

The `wgpu` context is passed only to Rust view extensions that explicitly
declare and implement the corresponding GPU interface; it is never part of the
mandatory generic view API.

The platform coordinates GPU resource lifecycle, view closing, window resize,
surface recreation, and device loss. Extensions must not assume devices or
per-frame resources stay valid forever.

### 9.5 Python view extension

Python plugins never receive `wgpu::Device`, `wgpu::Queue`, surfaces, texture
views, or command encoders.

Python plugins may extend views through higher-level interfaces, e.g.:
registering declarative panels, tables, or charts; submitting scene data drawn
by the host or a domain Workbench; adding annotations, color maps, or
visualization configuration; using host-provided restricted drawing
instructions or overlay APIs; running expensive data processing as background
tasks and handing results to the host for display.

## 10. Commands, async execution, and background tasks

### 10.1 Command system goals

Commands are the unified execution entry point for the RibbonBar, menus,
hotkeys, Python plugins, and automation. Callers never need to know whether a
command runs on a thread, an async executor, or a background process.

The platform shall support: stable command IDs; parameter definition and
validation; current context and enablement conditions; lightweight commands
that finish immediately; asynchronous commands; long-running commands
submitted as background tasks; command execution state, progress,
cancellation, and error propagation; document changes executed through
commands uniformly; undo/redo for reversible commands; decoupling commands
from RibbonBar, menus, hotkeys, and the Python API.

### 10.2 Execution states

```text
Pending
  +-- Running
  |    +-- Succeeded
  |    +-- Failed
  |    +-- Cancelled
  +-- Failed
  +-- Cancelled
```

A command may finish immediately or return an async handle / background task
handle. The platform tracks the lifecycle.

### 10.3 Progress feedback

Commands and tasks may report: task title and optional description; current
stage or subtask name; optional determinate progress value; indeterminate
progress state; success, failure, or cancellation result; diagnostic logs and
error details; whether the user may cancel.

The UI may present: progress in the status bar or task area; stage, progress,
and logs in a task panel; a cancel button; completion, failure, or
cancellation notifications; running state on RibbonBar commands or related UI
elements.

Commands never depend on egui or GPUI progress widgets; presentation belongs
to the chosen GUI frontend.

### 10.4 Cancellation and error handling

- Cancellable commands check a cancellation signal or task context;
- cancellation is cooperative; arbitrary computation cannot be force-interrupted;
- after cancellation there is a clear terminal state, and temporary resources
  are cleaned up where possible;
- errors carry a readable message and diagnosable details;
- plugin exceptions are caught and shown by the host; the main application
  never dies silently because of a plugin;
- task states are queryable for logs, automated tests, and product diagnostics.

### 10.5 Background tasks and document transactions

Background commands never modify UI or shared document internals on arbitrary
threads. The recommended flow:

1. the command reads inputs in a controlled context and creates a task;
2. the background task computes without holding uncontrolled mutable UI/document references;
3. the task returns results or candidate changes via messages;
4. the host commits changes at a well-defined document transaction boundary;
5. reversible operations record the information needed for undo;
6. if the document was closed, updated, or the task cancelled, the host rejects
   or revalidates the submission per an explicit policy.

The platform provides the document version / change-sequence mechanism needed
to prevent background tasks from overwriting newer user edits. Concrete
conflict policies involve the domain modules.

### 10.6 Command invocation path

```text
RibbonBar / menu / hotkey / Python API
                 |
                 v
           command registry
                 |
                 v
      context check, argument validation, execution
                 |
       +---------+---------+
       v                   v
  immediate           async / background task
                           |
               progress, cancel, logs, result
                           |
                           v
                    document transaction commit
```

## 11. Python plugin mechanism

### 11.1 Goals

Users extend the product's exposed commands, RibbonBar, menus, hotkeys,
panels, views, import/export, and event handling with Python plugins.

Plugins target a stable Host API — never Rust crate internals, GUI widgets, or
raw GPU resources.

### 11.2 Example manifest

```toml
[plugin]
id = "com.example.mesh-tools"
name = "Mesh Tools"
version = "0.1.0"
api_version = "1"
entry_point = "plugin.py:register"

[compatibility]
products = ["product-cad"]
platform = ">=0.1.0"

[permissions]
document_read = true
document_write = true
filesystem_read = false
network = false
```

Manifest fields may evolve. Plugin IDs must be stable and unique. Permission
declarations drive capability management, user prompts, and auditing — they do
not form a security sandbox for in-process Python.

### 11.3 Plugin extension points

The first phase should support: command registration; RibbonBar Tab, Group,
and item contributions; menu, toolbar, and hotkey contributions; enablement
conditions and context filters; parameter schema and command inputs;
declarative panels; restricted view contributions and visualization overlays;
document read access and document modification via host commands; async
commands and background tasks; progress reporting and cooperative
cancellation; versioned event subscription; importer/exporter registration;
plugin settings, logging, and error reporting.

### 11.4 Python API example

```python
def register(api):
    api.commands.register(
        id="mesh.refine",
        title="Refine Mesh",
        callback=refine_mesh,
    )

    api.ribbon.add_command(
        tab="mesh",
        group="mesh.modify",
        command="mesh.refine",
    )
```

The example shows interface intent only. The formal API must specify async
callbacks, interpreter constraints, errors, cancellation, progress, and
document commit behavior.

### 11.5 Plugin runtime and security boundary

- the host owns plugin discovery, loading, initialization, disable, and unload;
- Python callbacks reach platform capabilities through the controlled Host API;
- document changes are committed via commands or transactions;
- long operations use the task API;
- plugin exceptions are captured into logs and diagnostics;
- incompatible API versions refuse to load with a readable reason;
- in-process Python plugins are not a security sandbox;
- running untrusted plugins requires a separate process + IPC isolation design.

### 11.6 Plugin UI and rendering constraints

- plugins never depend on concrete GUI framework objects;
- ordinary plugin panels prefer declarative UI;
- Python view plugins extend via scene data, annotations, overlays, or
  high-level visualization APIs;
- Python plugins never hold raw `wgpu` resources;
- arbitrary Python-native drawing and complex custom widgets are not a
  first-phase commitment.

## 12. Common platform services

### 12.1 Document & project infrastructure

The platform provides: new, open, save, save-as, close, and recovery; stable
IDs for document objects; unsaved-state tracking; file format versioning and
migration entry points; multi-document context; namespaced, isolated plugin
data per document; autosave and crash-recovery extension points.

Domain document schemas are defined by domain modules. The platform never
adopts internal Rust object layouts as a long-term file format.

### 12.2 Transactions and undo/redo

- commands commit changes with well-defined transaction boundaries;
- reversible commands explicitly store or derive undo data;
- large domain data is never forced into expensive full copies;
- failed transactions leave no half-finished state;
- undo history is tied to the document lifecycle;
- explicitly irreversible operations are declared as such.

### 12.3 Context, events, and selection

- current product, document, window, central content, and work mode are
  expressed by the platform context;
- domain selection models are extended by domain modules;
- events have stable IDs, versions, and explicit lifecycles;
- plugin unload cleans up their event subscriptions;
- high-frequency render events are never broadcast as generic plugin events.

### 12.4 Settings, logging, and diagnostics

The platform provides: application-, product-, user-, and plugin-scoped
setting namespaces; structured logging with level filtering; diagnostics for
plugin loading, commands, tasks, and view errors; task- and
document-correlated logs; runtime configuration diagnostics; task and error
state interfaces queryable by automated tests.

## 13. Product composition and build

### 13.1 Product responsibilities

Each product defines: a unique product ID, name, icon, and distribution
configuration; the single GUI frontend it uses; the enabled Workbench modules;
default RibbonBar Tabs, Groups, and layout; default work mode, dock panel, and
central tab policy; the document types and file associations it provides; the
range and policy of installable plugins; the Python runtime and plugin
distribution policy.

### 13.2 Build-time selection

GUI selection happens in product configuration or at build time. It may be
expressed via a separate product crate, Cargo feature, or product build
configuration — but the final product binary contains exactly one GUI frontend
implementation. End users cannot switch GUIs.

### 13.3 Example product configuration

```toml
[product]
id = "com.example.cad"
name = "Example CAD"

[ui]
backend = "egui"

[workbenches]
enabled = ["sketch", "part", "assembly", "drawing"]

[plugins]
python = true
allowed_sources = ["user", "bundled"]
```

This configuration expresses assembly intent; the implementation may equally
use static typed assembly in a Rust product crate.

## 14. Suggested Cargo workspace layout

```text
crates/
  workbench-api/          # contracts visible to the platform and plugins
  workbench-core/         # lifecycle, context, module registry
  workbench-command/      # command execution, async states, transactions
  workbench-document/     # generic document & project services
  workbench-task/         # background tasks, progress, cancellation
  workbench-workspace/    # Ribbon, dock, central tabs, layout models
  workbench-view/         # generic view registration & lifecycle
  workbench-render-wgpu/  # optional wgpu render service
  workbench-ui-egui/      # egui frontend adapter (candidate)
  workbench-ui-gpui/      # GPUI frontend adapter (candidate)
  workbench-python/       # Python runtime bridge and plugin Host API
  workbench-test/         # test tooling for plugins, commands, Workbenches

workbenches/
  wb-example/             # example module validating the framework
  wb-domain-a/  wb-domain-b/

products/
  product-example-egui/   # example product, egui selected at build time
  product-example-gpui/   # candidate product build for evaluation only
```

Crate boundaries should be confirmed against real dependency edges. The first
phase does not require pre-creating all domain crates, nor maintaining two full
products simultaneously.

## 15. API and compatibility requirements

- the public platform API should stay small and semantically clear, with
  stable / experimental / internal interfaces distinguished;
- the plugin API is explicitly versioned; plugin manifests declare compatibility;
- Rust internal types never become stable plugin interfaces automatically;
- serialized schemas carry explicit versions and support migration;
- commands, events, document types, Ribbon items, panels, and views use stable IDs;
- removing or changing a plugin API provides a compatibility policy and
  readable diagnostics;
- GUI adapters and the render service are validated against platform contracts
  by automated tests;
- a Rust dynamic-library plugin ABI is not a first-phase commitment;
- the `wgpu` native interface is a controlled Rust extension capability, never
  exposed directly to Python plugins;
- task and progress APIs never depend on GUI frontend widget types;
- views that do not declare GPU capability never implement or depend on `wgpu`
  interfaces.

## 16. Non-functional requirements

### 16.1 Maintainability

- no circular dependencies between core crates;
- domain crates never depend on GUI implementation details;
- the plugin API has documentation, examples, and version notes;
- product configuration can audit enabled modules, render capabilities, and GUI frontend.

### 16.2 Stability

- plugin failures never crash the main application silently;
- background task errors, cancellation, and completion states are always visible;
- failed document saves keep the unsaved state;
- plugin unload cleans up commands, Ribbon items, event subscriptions, and
  panel registrations;
- GPU views follow the agreed resource-recovery flow on device/surface recreation;
- ordinary non-GPU views are unaffected by GPU device loss.

### 16.3 Performance

- long work never blocks UI responsiveness;
- platform events avoid unnecessary high-frequency global broadcast;
- document infrastructure never forces a full copy of the domain model per edit;
- the render service only provides resources for views that opted into GPU
  rendering;
- the task service leaves integration points for large-scale and
  domain-parallel execution.

### 16.4 Testability

- the platform core is testable without launching real windows where possible;
- command registration, async states, progress, cancellation, plugin
  manifests, and API compatibility all have tests;
- GUI frontends, workspace layout, and GPU view lifecycle have corresponding tests;
- at least one example Python plugin participates in CI;
- product assembly configuration validates dependencies and extension points.

## 17. Acceptance criteria

The phase-1 framework prototype validates the core architecture when:

1. a standalone product compiles with exactly one GUI selected at build time;
2. the product can assemble a Rust Workbench module;
3. the main window contains a RibbonBar, dock areas around it, a central
   multi-tab content area, and a status/task feedback area;
4. a Workbench can register RibbonBar Tabs, Groups, command items, dock
   panels, central content views, and work modes;
5. RibbonBar commands share enablement state and the execution entry with the
   platform command system;
6. the platform can open, activate, reorder, and close different kinds of
   content in the central multi-tab area;
7. switching the active central tab updates the command context and RibbonBar
   command states;
8. dock panel and central tab layouts persist and restore using stable IDs;
9. layout entries referencing missing plugins or Workbenches never break
   startup;
10. views can use presentation suited to their content, with no requirement
    that all views use `wgpu`;
11. only Rust view extensions that explicitly need GPU rendering can access
    the controlled `wgpu` rendering context;
12. Python plugins are discovered via manifests and can register commands and
    Ribbon items;
13. Python plugins cannot obtain raw `wgpu` GPU resources;
14. commands can finish immediately, run async, or submit as background tasks;
15. tasks report progress, accept cooperative cancellation, and return a clear
    terminal state;
16. the UI presents task progress, errors, and cancellation states;
17. background tasks commit document changes through controlled transactions,
    never by mutating UI state directly;
18. reversible commands support undo and redo; irreversible commands are marked;
19. a failing plugin initialization leaves the application running with logs
    or diagnostics;
20. incompatible plugin API versions are rejected with an understandable reason;
21. domain Workbenches never depend on egui or GPUI directly;
22. both GUI candidate implementations validate the core platform
    capabilities, but a single product never loads both;
23. a product package contains only the GUI frontend that product selected.

## 18. Implementation phases

### Phase 0: validate GUI choices and window workspace

Small prototypes with egui and GPUI respectively, validating: basic RibbonBar
layout and command binding; dock panels on all four sides; the central
multi-tab area; window layout save & restore; an ordinary GUI view plus one
optional GPU view; build and distribution experience across platforms.

The prototypes are for comparison; maintaining two full GUIs long-term is not
committed here.

### Phase 1: minimal platform loop

Implement: product entry and static Workbench composition; command
registration and the RibbonBar; dock panels and the central multi-tab area;
document create, modify, save, and open; basic undo/redo; work modes and
layout restore; task status, progress, and logs; one ordinary view and one
optional `wgpu` example view.

### Phase 2: async commands and transactions

Implement: unified command invocation results; async commands and background
tasks; progress, cancellation, errors, and logs; background-task /
document-transaction coordination; UI task feedback; commit policies when
documents expire or close.

### Phase 3: Python plugin loop

Implement: plugin manifests and discovery; API version checks; Python
initialization and plugin lifecycle; command, RibbonBar, menu, and panel
extensions; async plugin commands and progress reporting; plugin error
diagnostics and disabling; one example plugin with automated tests.

### Phase 4: establish the platform mainline

Pick the primary GUI technology route based on prototype results and real
Workbench development experience. Keep maintaining the other frontend only
when a clear product demand and maintenance resources exist; otherwise stop
extending it.

### Phase 5: production hardening

Add per real product needs: plugin source management and update mechanism;
process isolation protocol for untrusted plugins (if needed); optimizations
for large documents and long tasks; GPU resource recovery and view diagnostics
tooling; crash recovery, autosave, and multi-platform packaging; more complete
automation and performance testing.

## 19. Key risks and mitigations

| Risk | Mitigation |
|---|---|
| over-abstracted platform slowing down product work | build one complete vertical slice first; extract only repeatedly needed capabilities |
| maintaining two GUIs is too expensive long-term | treat them as selection prototypes; converge on the mainline per product demand |
| RibbonBar/Dock behavior diverging between GUIs | define platform-level models and interaction conventions; constrain behavior with shared acceptance tests |
| all views wrongly bound to GPU rendering | design rendering as an opt-in capability; ordinary views never touch `wgpu` |
| GPU views holding stale resources | render service owns lifecycle; define device-rebuild and resource-recovery flows |
| Python plugins mistaken for a security sandbox | state the boundary clearly; untrusted plugins need separate process isolation |
| background tasks overwriting stale document state | document version checks and explicit transaction commit boundaries |
| UI abstraction failing complex engineering panels | common features via declarative interfaces; complex views via bounded Rust extensions |
| premature dynamic Rust plugins causing ABI pain | Rust Workbenches statically linked in the beginning |
| unstable Python distribution/deployment | validate interpreter, dependencies, and product packaging in the early vertical slice |

## 20. Final architectural conclusion

`workbench-rs` is a **multi-product generic Workbench platform**, not a
super-application that bundles CAD, mechanical design, and simulation domain
capabilities.

- The platform provides the common application runtime, RibbonBar, dock
  workspace, central multi-tab area, commands, tasks, document infrastructure,
  and extension points;
- Rust Workbench modules provide composable product features;
- product developers decide module composition and the single GUI frontend;
- views integrate through a unified lifecycle and pick the presentation that
  suits their content;
- `wgpu` is an optional rendering capability; only Rust view extensions that
  explicitly declare it receive the corresponding context;
- Python plugins extend commands, the RibbonBar, panels, and restricted view
  capabilities through a versioned Host API;
- commands uniformly support immediate, async, and background execution, with
  progress, cancellation, and error state provided by the platform;
- background computation keeps explicit boundaries against the UI and document
  transactions;
- complex domain cores stay in domain modules; the generic platform never
  invades their data models;
- early development delivers a complete loop with static Rust modules, the
  classic workspace layout, async tasks, and controlled Python plugins, then
  evolves per real product needs.
