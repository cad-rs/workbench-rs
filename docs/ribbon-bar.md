# RibbonBar development document (workbench-ui)

> Status: design draft (pending review)
> Related: `docs/design.md` §4.3 / §6.4 / §7 (contracts), `crates/workbench-api/src/workspace.rs` (data model),
> `crates/workbench-ui/src/lib.rs` (the existing simplified rendering, to be replaced by this component)
> Product: creator (a classic 3D CAD application shell with a gpui front end)

## 0. Current state and gaps

**Already in place (platform layer, reusable without changes):**

- A complete data model: `RibbonTab` (stable ID + title + group list + origin marker), `ToolbarGroup`
  (stable ID + title + items + origin), `ToolbarItem` (command ID + optional override label).
  Tabs with the same ID merge their groups and groups with the same ID merge their items
  (design.md §7.3 controlled contributions); cleanup when a plugin is unloaded
  (`WorkspaceState::remove_commands`); active tab persistence (`LayoutFile.ribbon_active_tab`).
- The command system pipeline: `CommandId → Registry registration → execute_command / is_enabled /
  command_title / command_icon`; hotkey dispatch is already wired up.
- Registration entry point: `Registry::add_ribbon_tab` (wb-example already demonstrates it with two
  tabs).

**Gaps (what this component fills in):**

| # | Gap | design.md §7.2 requirement | Current state |
|---|------|--------------------|------|
| G1 | Only one button form (small button: icon on the left, text on the right, laid out flat) | Large/small button tiers, group titles, separators | No distinction between forms |
| G2 | No Split Button / Menu Button / Toggle / Combo | Explicitly required (including restricted declarative controls) | None |
| G3 | No Ribbon collapse (collapse/minimize) | A product-level display preference | None |
| G4 | No overflow handling | Compact presentation or overflow | None (clipped when too narrow) |
| G5 | No contextual tabs | Appear according to work mode/document type/selection | None |
| G6 | No keyboard navigation / KeyTips | Hotkey and keyboard navigation conventions | Global hotkeys only |
| G7 | No Tooltip (ScreenTip) | — (standard in CAD applications) | None |
| G8 | Inlined in `RootView::render` (~130 lines) | — (maintainability) | Needs extracting into its own component |

## 1. Design references and positioning

creator is positioned as a **classic CAD desktop application** (in the line of SolidWorks / Inventor /
Office Fluent), not a web application top bar. So the Ribbon takes the **classic Office Fluent form**
rather than a simplified toolbar:

- Two layers: the **tab strip** (the tabs) + the **panel area** (the current tab's array of groups);
- Buttons inside a group come in two tiers, **large/small**; each group has a **group title** at the
  bottom, and there are vertical separators between groups;
- It can collapse down to just the tab strip (maximizing viewport space, a frequent action for CAD
  users);
- Commands always execute through the command system; the Ribbon itself holds no business logic
  (design.md §6.4).

Not done: the Office round application button / Application Menu (replaced by the later menu system;
clicking the Logo is reserved as its entry point), and Gallery live-preview controls (an enum slot is
reserved, not implemented). The QAT (Quick Access Toolbar) is a **static product set** (first
version): the product declares a few frequently used commands through the API; they appear as
Icon-tier small buttons in the tab strip and stay visible while collapsed; user customization
(adding/removing) and persistence come in phase 2.

## 2. UI appearance

### 2.1 Overall layout

The Ribbon sits at the top of the window, below the title bar and above the dock area, in two
vertical sections:

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Title bar (system title bar, outside this component's scope)         │
├──────────────────────────────────────────────────────────────────────┤
│ ▸Tab strip [Logo][QAT: ↶ ↷ 💾 │] [Home] [Modeling] [Sketch] [View]   │  h≈30px
├──────────────────────────────────────────────────────────────────────┤
│ ▸Panel area                                                          │
│  ┌──────────┐  ┌──────────────┐  ┌────────────┐                      │
│  │ ⬛ New   │  │ ↶     ↷      │  │ ▦ ▦        │                      │  h≈98px
│  │    Open  │  │ Undo  Redo   │  │ ▦ ▦ Shapes │                      │
│  │    Save  │  │              │  │            │                      │
│  └──────────┘  └──────────────┘  └────────────┘                      │
│      File            Edit            Shapes                          │
├──────────────────────────────────────────────────────────────────────┤
│ Dock area / central tab area (dock, outside this component's scope)  │
└──────────────────────────────────────────────────────────────────────┘
```

### 2.2 Size specs (logical pixels)

| Element | Spec |
|------|------|
| Tab strip height | 30px |
| Tab padding | 12px horizontal, 6px vertical |
| Logo icon | Leftmost in the tab strip, 20×20, 8px left margin; click behavior in §4.1 |
| QAT buttons | Icon-tier spec (16px icon, 22px hit area), vertically centered in the tab strip; 2px spacing between buttons |
| QAT/tab separator | A 1px vertical line (`ribbon_group_sep`) between the QAT area and the first tab, with a 6px margin on each side |
| Panel area height | 110px (large button 86 = icon area 48 + two-line label 26 + chevron reserved strip 12, + group title 16 + top/bottom padding 8) |
| Large button | 86px total height, **identical for all large buttons**: icon area 48×48 (24×24 icon centered), label area 26px (two lines at 11px, 13px line height, centered, a third line is clipped), chevron reserved strip 12px (left empty on buttons without a chevron); having a chevron or not does not move the icon/text |
| Small button | 16px **icon on the left** + label on the right, laid out horizontally, 22px high; stacked vertically within the group (one per row) |
| Small icon button (Icon) | A 16px icon only, no label, 22px high, about 22px wide (icon + padding); stacked vertically within the group with the same row height as Small, and can be mixed with Small in the same column |
| Combo select box | 22px high (in line with Small rows): value area + 12px chevron; default width 120px (min 60 / max 300, overridable in the declaration); its width counts toward the column's widest row |
| Split Button (large) | Same layout as a Command large button (86px); the chevron is rendered in the reserved strip below the label (6 pt, centered across the full width): clicking the main area/label runs the default command, the chevron strip opens the menu |
| Split Button (small) | 16px icon area + 8px arrow area (the same for the Icon tier, without the label segment) |
| Split Button (small) | 16px icon area + 8px arrow area (the same for the Icon tier, without the label segment) |
| Group title | 10px, centered, color `text-secondary` |
| Group vertical separator | 1px wide, as tall as the panel area content, color `border` |
| Group padding | 8px horizontal, 4px vertical |
| Panel area side margins | 8px; 8px between groups |

### 2.3 Group layout algorithm

Groups are laid out left to right in declaration order; inside a group is a **vertical slot list**:

- Large buttons / Split(Large) each take a whole column (column width = 48px);
- Small / Icon / Combo buttons stack vertically, 2~3 rows forming a column (the column width adapts
  to that column's widest row; an Icon row is 22px wide, a Combo row is its declared width);
- Declaration order is column order: `[Large A][Small B / Icon C in two rows][Large D]…`.

The model layer introduces no new layout concepts: `ToolbarItem` gains a form field (see §6), and the
layout is derived from the form:
`Large → owns a column`; `Small → fill downward in order, start a new column once 3 rows are full`.

### 2.4 Button form overview

Buttons have two orthogonal dimensions: **form** and **size tier**. Size tiers: Large (24px icon on
top + label below) / Small (icon + label on one row) / **Icon (icon only, no label)**. Large always
has a label; Icon exists only at the Small size.

| Form | Structure | Icon-tier examples (CAD) | Typical use |
|------|------|--------------------|---------|
| `Command` | Icon + label; a click runs the command | Sketch drawing (line/circle/arc/rectangle), ortho/polar snap, wireframe/shaded | New, Save, Undo |
| `Toggle` | Same + a checked state (accent fill + accent outline) | Grid display, axis triad, constraint visibility | Ortho mode, grid snap |
| `Split` | The main button runs the default command (the default = the last one used); the arrow area opens a submenu of commands | Circle/arc/chamfer family flyouts (main area icon = the last-used variant) | Extrude (keeps the last variant) |
| `Menu` | Only opens a menu as a whole, never runs anything directly | View orientation (front/top/isometric…), dimension style family | Export As… |
| `Combo` | Select box: the value area shows the current value + a chevron; a click opens an option list | — (always has a value area, no Icon tier) | Layer, line weight, snap mode, view scale |
| `Launcher` | An 8px small icon at the bottom right of a group that opens the group's extended dialog | — | Small arrow at the end of a group |

The Icon-tier criterion is **whether the icon is self-explanatory**, not the command type. CAD sketch
tools are the Icon tier's home ground: line/circle/arc/rectangle/snap/constraint icons carry strong
conventions among CAD users (the AutoCAD/SolidWorks sketch and annotation ribbons already use many
icon-only buttons, and present the circle/arc/chamfer families as pure-icon flyouts), so they need no
text and are not confusing. Conversely, proprietary commands whose icons cannot be drawn clearly must
use Small. Whatever the tier, the discoverability fallback is the Tooltip (§4.3), which the Icon tier
in particular cannot do without. When the icon is missing, an Icon-tier button degrades to a
placeholder showing the first character of its label (a 16px square + the first character); it must
never render as a blank dead zone.

`Menu` popup content shares one renderer with the Split dropdown: a list of command IDs (optionally
grouped with separators); an entry = icon + label + optional hotkey hint; clicking an entry runs it
and closes the popup. The overflow dropdown of the fold strategy (§4.5 strategy B) reuses this
renderer with the same structure.

## 3. Visual style

### 3.1 Color tokens

Continue the platform's existing dark palette (`COL_BG` rgb(0.16,0.17,0.20) / `COL_TOOLBAR`
rgb(0.20,0.21,0.25) / `ACCENT` rgb(0.2,0.55,0.95)), defined centrally in `workbench-api` as named
tokens; no raw color values appear inside the component:

| Token | Value (first version) | Use |
|-------|-----------|------|
| `ribbon_bg` | Reuses `COL_TOOLBAR` | Overall Ribbon background |
| `ribbon_tab_strip_bg` | `COL_BG` | Tab strip (one shade darker than the panel area, to create layering) |
| `ribbon_tab_text` | `LIGHT_GRAY` | Inactive tab text |
| `ribbon_tab_active_text` | `WHITE` | Active tab text |
| `ribbon_tab_active_bar` | `ACCENT` | 2px indicator bar under the active tab |
| `ribbon_tab_hover_bg` | rgb(0.24,0.25,0.30) | Tab hover |
| `ribbon_btn_rest` | Transparent (blends into `ribbon_bg`) | Button rest state |
| `ribbon_btn_hover` | rgb(0.30,0.32,0.38) | Button hover (3px corner radius) |
| `ribbon_btn_pressed` | rgb(0.26,0.28,0.33) | Pressed |
| `ribbon_btn_checked_bg` | `ACCENT` @ 25% alpha + `ACCENT` border | Toggle checked |
| `ribbon_text` | `WHITE` | Button label (enabled) |
| `ribbon_text_disabled` | `GRAY` | Disabled label/icon |
| `ribbon_disabled_bg` | Transparent | Disabled: no fill, only the text/icon is dimmed |
| `ribbon_border` | rgb(0.10,0.10,0.12) | 1px Ribbon bottom edge (existing value) |
| `ribbon_group_sep` | rgb(0.30,0.31,0.36) | Group vertical separator |

Theming (light) comes in phase 2; tokenizing now is so that phase needs no rework.

### 3.2 State matrix

Each button renders according to the `(enabled, state, checked)` triple:

| State | Background | Icon | Label | Notes |
|------|------|------|------|------|
| rest (enabled) | Transparent | WHITE | WHITE | |
| hover | `btn_hover` | WHITE | WHITE | 3px corner radius |
| pressed | `btn_pressed` | WHITE | WHITE | |
| disabled | Transparent | GRAY | GRAY | Does not respond to clicks, no hover |
| checked | accent 25% + accent border | WHITE | WHITE | Toggle only |
| focus (keyboard) | Same as rest + 1px accent inner outline | | | Visible during keyboard navigation |

### 3.3 Icons

- Monochrome SVG that follows the text color (gpui `svg().text_color()`, the same mechanism as the
  existing 15px icons);
- Two sizes: 24px for large buttons, 16px for small buttons; the SVGs themselves are drawn on a 24
  grid and scaled down proportionally for the small tier;
- Paths come from the product's `AssetSource` (`icons/*.svg`); missing ones are silently not drawn
  (existing behavior) and the label still shows;
- No separate icon assets for the disabled state; it is dimmed by color throughout.

### 3.4 Text

The system font stack (`.SystemUIFont`, Segoe UI on Windows, same as today). Font sizes:
tab 12.5px / large button label 11px / small button label 11.5px / group title 10px / popup hotkey
hint 11px. Large button labels **wrap to at most two lines** (Fluent convention) and are truncated
with an ellipsis (`…`) if still too wide; small button labels are truncated on a single line; neither
is scaled.

## 4. Interaction

### 4.1 Tab strip

| Interaction | Behavior |
|------|------|
| Logo click | A static display in the first version; once the application menu (menu system) lands it becomes its entry point (§1) |
| QAT button click | Runs the bound command (same pipeline as §4.2); **still visible and usable while collapsed**, which is the whole point of the QAT. The first version is a static product set (`WorkspaceState.qat`, §6); user customization (adding/removing) in phase 2 |
| Tab click | `set_active_ribbon_tab(id)` → model change → the panel area redraws; the active tab indicator moves |
| Collapse (disabled by default) | Collapsing is a product capability switch (`collapse_enabled`, default **false**): when disabled there is no collapse button, and neither double-clicking nor Ctrl+F1 collapses. When enabled: the ˇ button at the right end of the tab strip and double-clicking the active tab both toggle collapse (the panel area is hidden and the Ribbon is just the 30px tab strip) |
| Tab click while collapsed | **Temporary expand**: the panel area is shown as an overlay above the dock; clicking anywhere outside the Ribbon or clicking the same tab again dismisses it; the collapsed state does not change |
| Clicking another tab while collapsed | Same as above; the overlay content switches to the new tab |
| Persistence | The collapsed state and the active tab are saved together in `LayoutFile` (the active tab already has a field; the collapsed state is new) |
| Restore | The `apply_layout` fallback rules follow the existing implementation (an unregistered tab falls back to the first one) |

### 4.2 Command buttons

- **Click** → `core.execute_command(cmd, args)` → failures are written to `host.log` → `notify`
  redraws. Fully reuses the existing pipeline (the pattern from lib.rs L701-720 is moved into the
  component as-is); the Ribbon does not understand command semantics.
- **Availability**: read from `is_enabled(cmd)` every frame; the command system is responsible for
  flipping it as the document/selection state changes.
- **Toggle**: a click likewise only sends the command; **the checked state is a query result from the
  command system** (see the new `is_checked` in §6); the Ribbon does not maintain the on/off state
  itself, which avoids two sources of truth alongside the document state.
- **Split**: clicking the main area runs the default command; clicking the arrow area opens the menu;
  clicking a menu entry runs the corresponding command.
- **Menu**: a click opens the menu. Popups are gpui-component `popover`/`menu` (vendored, rendered in
  the same window).
- **Combo (select box)**: a click opens the option list, positioned by the §4.4 anchoring rules;
  **the current value and option list are queried from the command system** (`combo_state`, the same
  pattern as Toggle's `is_checked`, §6). The Ribbon holds no option data, so document state changes
  such as adding/removing layers show up naturally; selecting an item runs
  `execute_command(cmd, args)` (the selected value is passed in via CommandArgs) and closes the
  popup; the current item has a check mark in the list, and option entries render like Menu entries
  (§2.4). The disabled state is the same as for buttons (`is_enabled`). The first version is
  select-only (no typing/editing); an `editable` field is left for phase 2.

### 4.3 Tooltip (ScreenTip)

Shown after hovering ≥500ms: the **command title** (`command_title`, bold) + the hotkey (if any, in
`Ctrl+S` form) + an optional description (a phase-2 command-system field; left empty and not shown
for now). For Icon-tier buttons (no label, §2.4) the Tooltip is their **only** text explanation, so
M2 must do it first. With insufficient space (§4.5): under the scroll strategy (default), buttons
scrolled out of view cannot be hovered, and their discoverability is carried by the end arrows and
keyboard scroll-into-view; under the fold strategy the overflow button is always visible, and its
Tooltip lists the folded groups.

### 4.4 Popup positioning (Dropdown Alignment)

The Ribbon has four kinds of popups (Split/Menu dropdowns, the collapsed-state tab overlay, Tooltips,
and the overflow group dropdown under the fold strategy); the first three share the positioning
rules in the table below:

| Dimension | Rule |
|------|------|
| Vertical | Popup top edge = trigger button bottom edge + 4px; **always opens downward, never flips upward** (the Ribbon is at the top of the window, so downward is the only natural direction; when the window is too short, cap the popup's maximum height and scroll its content) |
| Horizontal preference | **Left-aligned**: popup left edge = button left edge (Fluent convention, the strongest visual anchoring to the trigger button) |
| Right overflow flip | When the popup's right edge goes past the window content area's right edge - 8px, switch to **right-aligned** (popup right edge = button right edge); if it still overflows, clamp the right edge to the content area's right edge - 8px |
| Left overflow clamp | When the button hugs the left edge and the popup is wider than the button, if the popup's left edge is < 8px after left-aligning, clamp it to 8px |
| Width | Follows the content: min 160px, max = content area width - 16px; **not stretched to the button width** (the menu width is independent of the button width, Office convention) |
| Boundary reference | **The window content area**, not the screen; a windowed application does no screen-level/multi-monitor avoidance |
| Invocation | Click and keyboard (Enter/↓) use the same position and the same rules |
| Exclusivity and dismissal | At most one popup globally: opening a new one closes the old one; clicking anywhere outside the popup, Esc, scrolling, switching tabs, toggling collapse → close |

Special cases:

- **Overflow group dropdown (fold strategy only, §4.5 strategy B)**: the trigger button is always at
  the window's right edge, so in practice it always takes the right-align/clamp branch.
- **Collapsed-state tab overlay (§4.1)**: not anchored to a button; its top edge sits against the
  bottom of the tab strip and it **spans the full width of the window content area** (the behavior
  of Office's collapsed ribbon); max height = window height - tab strip - status bar - 8px. In a
  narrow window the overlay's interior is handled by the current §4.5 strategy.
- **Tooltip (§4.3)**: the only non-anchored popup; it follows the cursor (12px offset to the bottom
  right) and is clamped at the edges.

Implementation note: the vendored gpui-component already has the building blocks: `Popover`
(`.anchor(Anchor)` corner anchoring), `PopupMenu` (entries come with icon/checked/disabled/separators,
adjustable `min_w/max_w`) and `Select` (Combo's value area/list) map directly onto this section's
rules. Where the library's flip/clamp behavior differs from the table above, the table wins (compute
the anchor corner by these rules inside the ribbon crate, or position it ourselves); the vendored
source is not modified.

### 4.5 Insufficient-space strategy: horizontal scroll (default) / group folding (optional)

When the window is too narrow there are two strategies, chosen by display preference; the product
default is **scroll** (corresponding to design.md §7.2's "product defaults and user display
preferences"). Both share the baseline of "never shrink buttons, never reflow": button sizes and
relative positions do not drift with the window width.

**Strategy A: horizontal scroll (default)**, the same strategy as the SolidWorks CommandManager. Why
it is the default:

- The **relative positions** of icons **never change**: commands are only temporarily out of view,
  the purest form of spatial memory;
- There is no Group → menu entry form-mapping transformation (error-prone and hard to test);
- Plenty of CAD precedent (SolidWorks / FreeCAD / Rhino toolbars all scroll).

| Dimension | Rule |
|------|------|
| Trigger | When the total content width > the panel area width W, the panel enters the scrollable state; ‹ › arrow buttons appear at the end(s) that can scroll (overlaid on the content edge, with a semi-transparent background) and are hidden when the content fits |
| Arrow click | Scrolls about 80% of a screen in that direction (smooth animation, ~150ms); the arrow on a side that has reached the end is disabled |
| Wheel | A horizontal wheel / trackpad horizontal swipe scrolls directly; **Shift + vertical wheel** scrolls; **a bare vertical wheel does not scroll** (CAD habit: the wheel is reserved for viewport semantics, avoiding accidental scrolling while hovering over the Ribbon) |
| Range clamp | Scroll offset ∈ [0, total content width - W]; on resize the offset is kept and re-clamped |
| Keyboard reach | When focus navigation (§4.6) enters a clipped button, it automatically scrolls into view (minimal scroll distance) |
| Edge clipping | Half-visible buttons are allowed at the viewport edges (the natural form of scrolling); half-visible buttons still respond to clicks and Tooltips |
| Persistence | The scroll offset is transient and is not stored in the layout file; the collapsed overlay starts from offset 0 each time it opens |

Known weakness (a deliberately accepted trade-off): a command scrolled out of view leaves only the
end arrow as a clue; discoverability is mitigated by "arrow visibility doubles as a scroll-state hint
+ keyboard scroll-into-view + Tooltips on half-visible buttons".

**Strategy B: group folding (optional)**, Office Fluent-style overflow:

1. The rightmost group is replaced as a whole by a dropdown button (group title + chevron); clicking
   it opens an overlay with the group's full contents (button forms are mapped to menu entries, Large
   before Small; rendered with the same structure as the Split/Menu dropdowns, §2.4. **Combo is the
   exception: the control is rendered as-is inside the overlay**, since turning it into a menu entry
   would lose the current-value display); if space is still short, keep folding groups one by one
   towards the left until everything fits;
2. **Extreme case (W is still not enough)**: every group may be folded, with no guaranteed group: the
   panel area is left with a single row of overflow buttons, **the panel area height stays at 98px**
   (so the dock area below does not jitter while the window is dragged), and the overflow buttons
   are vertically centered; a single group that is by itself wider than W also exists only in the
   overflow dropdown;
3. Invariants: **whole groups move in and out** (a group is either fully visible or entirely in the
   overflow; buttons within a group are never clipped); the overflow button appears only when some
   group has been folded, and its Tooltip lists the titles of the folded groups; the layout is a
   deterministic function of W, and widening unfolds in order (the leftmost folded group returns to
   the panel area first).

Reasons to choose it over the default (that is, why it is kept as an option): the hidden commands
have a **visible list** (the overflow button itself), which Fluent users are familiar with; the cost
is that command positions move in and out of the panel area with the window width, and a whole set
of form mappings is needed.

**Shared rules and switching:**

- Under both strategies **the panel area height is always 98px and a bare wheel never scrolls**; the
  collapsed overlay (§4.1) applies the current strategy inside; the tab strip scrolls independently
  under both strategies (a low-priority implementation), and **the Logo and QAT area stay fixed at
  the left end and do not scroll with the tabs**;
- Preference field: `ribbon_overflow: "scroll"(default) | "collapse"`, stored in the layout file (§6);
  the product can override the default via the builder; switching **takes effect immediately**
  (re-laid out in the same frame, no restart).

Implementation note: scrolling uses a horizontal scroll container + an end-arrow overlay; the full
content width is measured first to decide whether the arrows show, and the cost of one frame of
measurement is acceptable.

### 4.6 Keyboard

| Key | Behavior |
|----|------|
| Global hotkeys | Already exist (window-level on_key_down → hotkey dispatch); unchanged |
| `Ctrl+F1` | Toggles collapse (Office convention); only effective when collapsing is enabled (disabled by default, §4.1) |
| KeyTips (the `Alt`-triggered chain of letter hints) | **Phase 2**: not in the first version, and the model layer needs no reservation for it (purely a rendering-layer behavior) |
| Tab-key focus navigation | Built on the gpui focus system: tab strip → panel buttons in visual order; for the focus state see §3.2 |

### 4.7 Contextual tabs

- The trigger is the **work mode** (`work_mode`, already exists): switching modes → the corresponding
  contextual tab appears/disappears (e.g. "Sketch" mode shows the "Sketch" tab). Document-type
  triggers are hooked up in phase 2 (they need a document type enum first).
- **New in the model layer**: `RibbonTab` gets `contexts: Vec<String>` (empty = an always-visible
  tab); visibility = `contexts` is empty || `contexts.contains(work_mode)`. Filtering in the
  rendering layer is enough; no new tab concept is introduced, and the existing merge/cleanup
  semantics are not broken.
- Visuals: a contextual tab's indicator bar and title use a secondary accent color (e.g. amber
  rgb(0.9,0.7,0.2)) to set them apart (Fluent convention); the first version may use the same color
  for now, marked TODO.

### 4.8 Plugins and contributions (unchanged)

Plugins still go only through `Registry::add_ribbon_tab` / `add_toolbar_group` (merge semantics,
origin markers and unload cleanup all carry over); plugins **cannot** affect rendering beyond the
button forms (there is no custom-control entry point: design.md §7.3 "restricted declarative
Controls"; the first version only opens up the five forms of §2.4).

## 5. Architecture and code placement: reuse gpui-component, decouple from workbench

**Goal constraint:** once mature, the component is contributed upstream to gpui-component. So from
day one the RibbonBar is a **pure gpui / gpui-component component**: it lives in its own crate and is
forbidden at compile level from depending on workbench-api / workbench-core (a crate that cannot see
them can never couple to them; what review cannot enforce, a compile-time constraint does).

### 5.1 Layers

```text
+-- workbench-ui ----------------------------------------+
|  ribbon_bridge.rs (the only workbench coupling point)  |
|    WorkspaceState snapshot -> ribbon::RibbonModel      |
|    ribbon events -> execute_command / model methods    |
+-- ribbon (new crate) ----------------------------------+
|  Pure component: view-model types + RibbonBar          |
|    rendering + transient state                         |
|  Dependencies: gpui, gpui-component (only these two)   |
+-- gpui-component --------------------------------------+
|  Button / Tab / Popover / PopupMenu / Select           |
+--------------------------------------------------------+
```

| Location | Content |
|------|------|
| `crates/ribbon/` (new crate) | Its own view-model types (`RibbonModel`/`RibbonTab`/`RibbonItem`…, §6) + rendering (tab strip, panel area, strategies A/B, collapse, §4.4 popup positioning) + internal transient state (the scroll offset; the collapsed state/active tab are controlled and passed in from outside). Public interface = `RibbonModel` (data) + event callbacks (`on_command(id, args)` / `on_active_tab_change(id)` / `on_toggle_collapse`) |
| `workbench-ui/src/ribbon_bridge.rs` (new, ~200 lines) | **All** of the workbench coupling is concentrated here: every frame it assembles the `RibbonModel` from `WorkspaceState` (including the `is_enabled`/`is_checked`/`combo_state` query results), and the event callbacks forward to `execute_command` / `set_active_ribbon_tab` / `notify`. Mounted in `RootView::render` with one line; the old inline implementation at L627-757 is deleted (Rule 4) |
| `crates/workbench-api/src/workspace.rs` | The plugin contribution API is unchanged (`add_ribbon_tab` etc.); only fields are extended per §6; the bridge does a mechanical field-by-field mapping from model → view model |
| `crates/workbench-api/src/command.rs` or services | `is_checked(cmd) -> bool` (Toggle); `combo_state(cmd) -> Option<ComboState{current, options}>` (Combo, queried every frame); CommandArgs gains an optional value slot (the Combo's selected value) |
| `docs/design.md` §7 | Status write-back: once implemented, add implementation notes pointing to this document to the "design intent" paragraphs |

### 5.2 gpui-component reuse mapping

Reuse the vendored gpui-component's ready-made controls as much as possible; the ribbon crate only
writes "the Ribbon-specific layout and glue":

| Ribbon element | Reuses | Notes |
|------|------|------|
| Tab strip and active indicator | `tab::TabBar` | The active underline and hover state come ready-made; style differences are tuned with style refinement |
| Large/small/Icon buttons | `button::Button` | Large buttons customize their children (icon on top, label below); the four states hover/pressed/disabled/selected come for free |
| Toggle | `Button` selected state | Not a separate control: checked is just a visual state of Button, and the data comes from the `is_checked` query |
| Split / Menu dropdowns | `Popover` (`.anchor`) + `PopupMenu` | Entry icon/checked/disabled/separators are ready-made (§4.4 implementation note) |
| Combo (select-only) | `select::Select` | Value area + option list + check mark on the current item are ready-made; the phase-2 editable tier upgrades to `combobox` |
| Tooltip | `tooltip` | Hover delay and follow behavior are ready-made |
| Icons / separators | `icon` / `separator` | SVG tinting / 1px separators |

Only four pieces are hand-written: the group column layout (§2.3), the insufficient-space strategies
A/B (§4.5), the collapsed overlay (§4.1) and the popup positioning special cases (§4.4). These are
exactly the parts that belong to the ribbon module itself when it is contributed upstream.

### 5.3 Upstream contribution path

Once mature, move `crates/ribbon/` wholesale into gpui-component (e.g. `crates/ui/src/ribbon/`): its
only dependencies are gpui + gpui-component anyway, so this is a **move, not a rewrite**; the
workbench-ui bridge changes one import line. Naming follows upstream style from day one (module
`ribbon`, types `RibbonBar`/`RibbonModel`, no wb prefix), and no workbench concepts are brought in
(terms such as command system and Registry stay out of ribbon's public API).

### 5.4 Architecture invariants

- `crates/ribbon/Cargo.toml` contains no workbench-* entry; `cargo tree -p ribbon` is the acceptance
  check (no workbench node in the dependency tree);
- The ribbon component does not understand command semantics: it only emits `on_command(id, args)`
  events, and execution and result handling all live in the bridge;
- The view model (ribbon crate) and the plugin contribution model (workbench-api) are **two sets of
  types**, mapped field by field by the bridge. That is the cost of decoupling (every new form adds a
  field in each place + one mapping line), and it is accepted;
- Files < 2000 lines (Rule 5): the ribbon crate is estimated at 600-900 lines; if it exceeds the
  limit, split it into `tabs.rs` / `panel.rs` / `popup.rs`.

## 6. Data model extensions (workbench-api)

Backward-compatible extensions: existing registration code (wb-example, plugins) **compiles with zero
changes**. The extensions live in workbench-api (the plugin contribution model); the ribbon crate's
view model (§5.1) mirrors these concepts but defines them independently, and the bridge maps them
field by field (§5.4):

```rust
// ToolbarItem extension (workspace.rs)
pub enum RibbonItemKind {
    Command,                       // default, equivalent to today
    Toggle,                        // checked comes from the is_checked query
    Split { items: Vec<ToolbarItem> },      // dropdown subcommands
    Menu  { items: Vec<ToolbarItem> },      // dropdown only
    Combo,                         // options/current value from the combo_state query; selecting runs the command (with a value argument)
}

pub struct ToolbarItem {
    pub command: CommandId,
    pub label: Option<String>,
    pub kind: RibbonItemKind,      // serde(default) = Command
    pub size: ItemSize,            // Large | Small | Icon; serde(default) = Small (closest to today's look)
    pub icon: Option<String>,      // overrides the command's default icon (rarely used; mainly a fallback for the command system)
}

// RibbonTab extension
pub struct RibbonTab {
    // ...existing fields...
    pub contexts: Vec<String>,     // empty = always visible; contains a work_mode name = visible in that mode
}

// QAT quick access (static product set, §4.1): shown in the tab strip after the Logo and before the
// 1px separator, still visible while collapsed. WorkspaceState gains `qat: Vec<ToolbarItem>` (reusing
// ToolbarItem, rendered at the Icon tier); Registry gains `add_qat_command(cmd)`. User customization
// (adding/removing) and persistence in phase 2.

// LayoutFile extension
pub struct LayoutFile {
    // ...existing fields...
    #[serde(default)]
    pub ribbon_collapsed: Option<bool>,
    /// Insufficient-space strategy (§4.5): "scroll" (default) | "collapse".
    #[serde(default)]
    pub ribbon_overflow: Option<String>,
}
```

`Registry` gains the corresponding builder methods (`toggle()` / `split()` / `menu()` / `combo()` /
`large()` / `icon_only()` / `context()` / `add_qat_command()`); the semantics of the existing
`command()` / `labeled()` are unchanged.

## 7. Implementation milestones

| Milestone | Content | Acceptance |
|--------|------|------|
| M1 Skeleton refactor | Create the `crates/ribbon` crate (gpui/gpui-component dependencies only) + `ribbon_bridge.rs`; large/small button forms + group titles + vertical separators + a static collapse button; the tab strip Logo and QAT (static product set); color tokens | creator runs and meets the visual spec (§3), every command click works, the old inline implementation is deleted, `cargo tree -p ribbon` shows no workbench dependency |
| M2 Complete states | Toggle/Split/Menu/Combo + popover popups (positioned per §4.4) + `is_checked`/`combo_state` + Tooltip | The four new forms work in wb-example; the disabled state is correct; dropdown flip/clamp is correct |
| M3 Scrolling and collapse (default strategy) | Horizontal scrolling of the panel area/tab strip (§4.5 strategy A); popup positioning rules (§4.4); collapse + the temporary overlay while collapsed + persistence | Scrolling and arrow visibility are correct when dragging the window from 400px to 4K; the collapsed state is restored after a restart; dropdown flip/clamp is correct in every §4.4 scenario |
| M4 Contextual tabs | `contexts` + work_mode linkage + keyboard focus navigation | Switching modes makes tabs appear/disappear; layout persistence round-trips |
| M5 Fold strategy (optional) | Group fold dropdown (§4.5 strategy B) + switching and persisting the `ribbon_overflow` preference | Switching strategies takes effect immediately; the folded end state / in-order unfolding is correct; the preference round-trips |

For every milestone: `cargo check` + `cargo test -p workbench-ui` (the existing
`the_shell_snapshot_covers_every_region` must stay green) + a creator smoke test (the `--smoke` mode
already exists).

## 8. Acceptance checklist (visual + interaction)

**Appearance**

- [ ] Tab strip 30px / panel area 110px; the active tab has a 2px accent bottom bar
- [ ] Large buttons have a 24px icon on top, small buttons a 16px icon on the left; a 10px group title is centered at the bottom of each group
- [ ] Icon-tier buttons show only a 16px icon with no label and share the row height when mixed with Small in the same column; with a missing icon they show a first-character placeholder instead of a blank
- [ ] The Logo (20px) at the far left of the tab strip, followed directly by the QAT small icon area, separated from the tabs by a 1px vertical line
- [ ] 1px vertical separators between groups; a 1px `ribbon_border` at the Ribbon's bottom edge
- [ ] All five button forms (§2.4) are visually complete, with correct hover/pressed/checked/disabled states
- [ ] With a missing icon only the label shows, leaving no hole

**Interaction**

- [ ] Clicking a tab switches the panel area; active tab persistence round-trips
- [ ] Collapse (disabled by default; verify after the product enables it): the collapse button/double-click/Ctrl+F1 work; once collapsed only the tab strip remains; clicking a tab while collapsed opens the overlay, and clicking outside dismisses it
- [ ] Clicking a QAT button runs its command; the QAT stays visible and usable while collapsed
- [ ] Default (scroll): narrowing the window puts the panel area into horizontal scrolling, the end arrows appear as needed and are disabled at the ends; a bare wheel does nothing, Shift+wheel/horizontal wheel scrolls; keyboard focus scrolls into view
- [ ] Extremely narrow window (scroll): the panel area height does not change (the dock does not jitter), and every command is reachable through scrolling and the arrows alone; the same rules apply inside the collapsed overlay
- [ ] Switching to the fold strategy: group-by-group folding and the all-folded end state are correct, the overflow button's Tooltip lists the groups, widening unfolds in order; switching takes effect immediately and the preference round-trips
- [ ] Hovering an Icon-tier button shows a Tooltip (command title + hotkey), its only text explanation
- [ ] Failed commands are logged (visible in the status bar at the bottom right); disabled buttons cannot be clicked
- [ ] Clicks on a Split's main area and arrow area behave separately; picking a menu entry runs it and closes the popup
- [ ] Dropdowns are left-aligned by default; at the right edge they flip to right-aligned and are clamped if that is still not enough; popups are mutually exclusive (opening a new one closes the old one), and Esc/clicking outside/scrolling/switching tabs all close them
- [ ] Combo: the popup is anchored correctly, the current item has a check mark, selecting runs the command with the value and closes the popup, the current value and options refresh with the command system state, and the list cannot be opened while disabled
- [ ] After a plugin is unloaded, the tabs/groups/items it contributed disappear (no regression in the existing `remove_commands` behavior)
