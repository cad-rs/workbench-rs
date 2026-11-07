# RibbonBar handover (2026-10-20): M1+M2 delivered, M3 remaining

> For the next session: after reading this you can start work directly, with no need to go back
> through this round's conversation.
> Spec: `docs/ribbon-bar.md` (the single source of truth for appearance/interaction/architecture,
> maintained alongside the code).

## 1. Current state (committed, not pushed)

workbench-rs master (6 commits since 4321188):

| Commit | Content |
|--------|------|
| `8e7b3b3` | feat(api): workbench-api/core data model extensions (RibbonItemKind/ItemSize/contexts/qat/LayoutFile fields, is_checked/combo_state queries, CommandArgs.value, AssetSource.paths(), non-blocking file dialogs with request_dialog/frame_tick draining) |
| `acc5f9b` | feat(ui): `crates/ribbon` pure component crate + workbench-ui integration (ribbon_bridge) + wb-example demo + icon chain/a11y fixes |
| `8da3ceb` | docs: ribbon-bar.md (485-line spec) |
| `75763a4` | fix(ui): collapsing disabled by default (collapse_enabled, default false) |
| `a60bf56` | docs: this document (handover) |
| `df58174` | refactor(ui): unified item click dispatch (ItemChrome) + separator height/spec fixes |

Parent repo rcad-pro (branch feat/acceptance-suites): `63f4f3c57` (creator QAT registration),
`b2703eb22` (submodule pointer 8da3ceb; it has to be advanced again after the later commits).

**Baseline**: ribbon 4 tests, workbench-ui 23+3 (assets_probe) all green, workbench-core 19+3,
selfcheck 15 PASS/0 FAIL, `cargo tree -p ribbon` has no workbench dependency (a prerequisite for
contributing upstream).

## 2. Architecture at a glance (read before changing anything)

```text
workbench-ui/ribbon_bridge.rs   ← the only workbench coupling point: every frame
│                                 WorkspaceState → ribbon::RibbonModel; events → execute_command
├─ crates/ribbon/ (pure gpui+gpui-component, isolated at compile level, to be contributed upstream)
│   ├─ model.rs   RibbonModel/RibbonEvent/view-model types (the bridge maps them field by field)
│   ├─ state.rs   RibbonState: set_model replaces the snapshot every frame;
│   │             transient state = scroll offset + singleton popup
│   ├─ tabs.rs    30px tab strip: Logo (default None)→QAT→separator→Tabs→collapse button (hidden by default)
│   ├─ panel.rs   86px→110px panel area: render_large_column / render_row_item /
│   │             render_combo + ItemChrome unified click dispatch
│   ├─ popup.rs   popup building/anchoring/singleton management; track_trigger (records the rect in on_prepaint)
│   ├─ layout.rs  pure column-layout logic (3-slot stacking / Large owns a column) + unit tests
│   └─ tokens.rs  color/size tokens (one dark set)
```

**Core mechanisms (invariants not to break)**:

1. **Unified click dispatch**: every item button has exactly one `on_click`
   (`ItemChrome::attach_click`), dispatched by the `ClickEvent` position: inside the Split's 6 pt
   strip rect → menu, Menu → whole-button menu, everything else → default command. The chevron
   element is **purely visual** (it only hooks `track_trigger` to record its rect) and **never
   nests its own on_click** (gpui dispatches to the outer element first; stop_propagation on the
   inner one is unreliable).
2. **Layout consistency**: every Large button is 86px = icon area 48 + two-line label 26 + chevron
   reserved strip 12 (buttons without a chevron leave that strip empty), so having a chevron or not
   makes zero layout difference (a hard constraint from user acceptance).
3. **A Small column has 3 fixed equal slots**: 86/3 ≈ 28.67px per slot; with fewer than 3 items the
   bottom stays empty; there is no 2-way split.
4. **Collapsing is disabled by default**: `RibbonModel.collapse_enabled` (default false) gates every
   collapse entry point (rendering the collapse button, double-clicking a tab, events); the bridge
   always passes false.
5. **Asset chain**: gpui-ce's AssetRegistry is **enumeration-based**: a path that `list("")` does not
   report never resolves (the SVG silently does not draw). WbAssets::list must take the union of the
   product source + the platform standard source + the component source and deduplicate it (the
   assets_probe.rs regression test locks this behavior in).
6. **Element ids**: `panel-item-{group_id}/{index}/{command}`. The same command can appear in several
   items (a Split and a Menu bound to the same command), and non-unique ids trigger an a11y
   duplicate-node panic (dev builds).

## 3. Pitfalls already hit (don't repeat them)

1. **Leftover instances**: when launching in the background with `( ... &)` and the taskkill timing
   is off, an old instance lingers; with old and new running side by side, clicks land on the old
   window, which looks like "the change had no effect". **Before every verification:
   `taskkill //F //IM creator.exe`, then confirm the `tasklist` count is 1**; if the exe is locked
   after a build, the build fails silently, so rerun the build and confirm Finished.
2. **CUA automated clicks**: the raster↔logical coordinate mapping is unstable across DPI layers, and
   blind clicks on small targets around 16×14 are unreliable (they drift even after probe
   calibration). To verify small targets, print the rects with a probe and click manually, or
   temporarily enlarge the hit area.
3. **invalid-handle ERROR noise when closing the window** (gpui_windows dispatcher): the user
   confirmed this is a normal side effect of closing the window; the layout has already been saved
   by then, so it is harmless. A related startup crash (0xcfffffff) happened once and has not been
   reproduced; parked as low priority.
4. gpui-component's `Button` does not implement gpui's `StatefulInteractiveElement`, so generic
   on_click helpers had to be split by type (a historical lesson; `with_menu_click_div` was removed
   along with the unified dispatch).

## 4. M3 remaining (start directly next round)

| # | Task | Where | Notes |
|---|------|------|------|
| 1 | Scroll animation | TODO in panel.rs `scroll_arrow`: `scroll_by_page` jumps instantly → 150ms ease-out (gpui animation or per-frame interpolation; mind the interaction with the transient `scroll_offset`) | The user has accepted the feature; only the animation is missing |
| 2 | Persist the collapsed state | The `LayoutFile.ribbon_collapsed` field **exists but is not wired up**; the bridge's `collapsed` is a runtime value (RibbonHost `Rc<Cell<bool>>`) → write it into the layout on save, read it back on startup | Main M3 item |
| 3 | Ctrl+F1 toggles collapse | Not implemented (in spec §4.5); only effective when `collapse_enabled=true` | Depends on 4 |
| 4 | Path to enabling collapse | The bridge currently always passes `collapse_enabled` as false; for a product to enable it = builder/setting → bridge | Depends on 4 |
| 5 | Temporary overlay while collapsed | Spec §4.1 (clicking a tab while collapsed opens a full-width overlay), not implemented | Only meaningful once collapsing is enabled |

M4 (amber contextual tabs + finer focus navigation) and M5 (fold strategy B) are further out; see
ribbon-bar.md for their spec.

## 5. Verification commands (run every round)

```bash
# In workbench-rs:
cargo test -p ribbon -p workbench-ui -p workbench-core --offline
cargo run -p creator --bin creator -- --selfcheck   # 15 PASS / 0 FAIL
# Visual check: start a single instance (taskkill first and confirm nothing is left over):
cargo build -p creator --bin creator --offline && ./target/debug/creator.exe
```

Regression red lines: `the_shell_snapshot_covers_every_region`, the 6 bridge projection tests,
the 3 assets_probe tests, and no workbench node in `cargo tree -p ribbon -e normal`.

## 6. Known open issues (parked)

- Intermittent gpui platform crashes: one at startup (0xcfffffff, not reproduced) + invalid-handle
  noise on window close (the user confirmed it is harmless). If a live crash happens again:
  `RUST_BACKTRACE=full` + grab the logs immediately.
- The Diagnostics panel shows ~100-180ms/frame (debug build + 3D viewport); not yet checked whether
  the Ribbon introduced it (most likely it is the viewport/debug build itself).
- Scrolling of the tab strip itself, KeyTips, amber contextual tabs: the spec marks these as low
  priority / phase 2.
