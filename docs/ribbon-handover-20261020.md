# RibbonBar 交接（2026-10-20）——M1+M2 已交付，M3 剩余待推进

> 面向下一 session：读完本文即可直接开工，无需回溯本轮对话。
> 规格文档：`docs/ribbon-bar.md`（外观/交互/架构的单一事实来源，随代码同步维护）。

## 1. 当前状态（已提交，未推送）

workbench-rs master（自 4321188 起 4 个提交）：

| Commit | 内容 |
|--------|------|
| `8e7b3b3` | feat(api)：workbench-api/core 数据模型扩展（RibbonItemKind/ItemSize/contexts/qat/LayoutFile 字段、is_checked/combo_state 查询、CommandArgs.value、AssetSource.paths()、非阻塞文件对话框 request_dialog/frame_tick 排水） |
| `acc5f9b` | feat(ui)：`crates/ribbon` 纯组件 crate + workbench-ui 集成（ribbon_bridge）+ wb-example 演示 + 图标链/a11y 修复 |
| `8da3ceb` | docs：ribbon-bar.md（485 行规格） |
| `75763a4` | fix(ui)：折叠功能默认禁用（collapse_enabled，默认 false） |

父仓库 rcad-pro（feat/acceptance-suites 分支）：`63f4f3c57`（creator QAT 注册）、
`b2703eb22`（submodule 指针 8da3ceb → 后续提交后需再推进）。

**基线**：ribbon 4 测试、workbench-ui 23+3（assets_probe）全绿、workbench-core 19+3、
selfcheck 15 PASS/0 FAIL、`cargo tree -p ribbon` 无 workbench 依赖（上游贡献前提）。

## 2. 架构速览（改动前必读）

```text
workbench-ui/ribbon_bridge.rs   ← 唯一 workbench 耦合点：每帧 WorkspaceState →
│                                ribbon::RibbonModel；事件 → execute_command
├─ crates/ribbon/（纯 gpui+gpui-component，编译级隔离，未来贡献上游）
│   ├─ model.rs   RibbonModel/RibbonEvent/视图模型类型（bridge 逐字段映射）
│   ├─ state.rs   RibbonState：set_model 每帧替换快照；瞬态=滚动偏移+单例弹层
│   ├─ tabs.rs    30px Tab 条：Logo(默认 None)→QAT→分隔线→Tabs→折叠钮(默认隐藏)
│   ├─ panel.rs   86px→110px 面板区：render_large_column / render_row_item /
│   │            render_combo + ItemChrome 统一点击分派
│   ├─ popup.rs   弹层构建/锚定/单例管理；track_trigger(on_prepaint 记录矩形)
│   ├─ layout.rs  列布局纯逻辑（3 槽堆叠/Large 独占列）+ 单测
│   └─ tokens.rs  颜色/尺寸 token（深色一套）
```

**核心机制（不要破坏的不变量）**：

1. **统一点击分派**：每个条目按钮只有一个 `on_click`（`ItemChrome::attach_click`），
   按 `ClickEvent` 位置分派——Split 的 6 点条矩形内→菜单，Menu→整钮菜单，
   其余→默认命令。chevron 元素**纯视觉**（只挂 `track_trigger` 记录矩形），
   **绝不嵌套独立 on_click**（gpui 外层先派发，内层 stop_propagation 不可靠）。
2. **布局一致性**：所有 Large 按钮 86px = 图标区 48 + 两行标签 26 + chevron 预留条 12
   （无 chevron 的按钮该条留空）——有无 chevron 零布局差异（用户验收的硬约束）。
3. **Small 列固定 3 等分槽位**：86/3 ≈ 28.67px/槽，不足 3 项下部留空，无 2 等分。
4. **折叠默认禁用**：`RibbonModel.collapse_enabled`（默认 false）门控一切折叠入口
   （折叠钮渲染、Tab 双击、事件）；bridge 恒传 false。
5. **资源链**：gpui-ce 的 AssetRegistry 是**枚举制**——`list("")` 没报出的路径永远
   解析失败（SVG 静默不画）。WbAssets::list 必须并集 产品源+平台标准源+组件源，
   并去重（assets_probe.rs 回归测试锁死此行为）。
6. **元素 id**：`panel-item-{组id}/{序号}/{命令}`——同命令可出现在多个条目
   （Split 与 Menu 绑同一命令），id 不唯一会触发 a11y 重复节点 panic（dev 构建）。

## 3. 已踩过的坑（勿重复）

1. **多实例残留**：`( ... &)` 后台启动 + taskkill 时序不齐时，旧实例会残留，
   新旧并存时点击落在旧窗口 = "改了没生效"的假象。**每次验证前：
   `taskkill //F //IM creator.exe` 后确认 `tasklist` 计数为 1**；构建后若 exe 被
   占用会静默失败，需重跑 build 确认 Finished。
2. **CUA 自动化点击**：raster↔逻辑坐标映射跨 DPI 层不稳定，16×14 级小目标盲点
   不可靠（探针标定过也会漂）。小目标验证用探针打印矩形 + 人工点击，
   或临时放大命中区。
3. **关闭窗口时的 invalid-handle ERROR 噪声**（gpui_windows dispatcher）：
   用户确认是正常关窗伴随现象，布局保存在此之前已完成，无害；同族还有一次
   启动期崩溃（0xcfffffff）未复现，低优先级挂账。
4. gpui-component 的 `Button` 未实现 gpui 的 `StatefulInteractiveElement`，
   on_click 泛型助手需按类型拆分（历史教训，`with_menu_click_div` 已随统一
   分派删除）。

## 4. M3 剩余（下一轮直接开工）

| # | 任务 | 落点 | 备注 |
|---|------|------|------|
| 1 | 滚动动画 | panel.rs `scroll_arrow` 内 TODO：`scroll_by_page` 瞬跳 → 150ms ease-out（gpui 动画或分帧插值，注意与 `scroll_offset` 瞬态的交互） | 用户已验收功能，只差动画 |
| 2 | 折叠态持久化 | `LayoutFile.ribbon_collapsed` 字段**已存在未接线**；bridge 的 `collapsed` 是运行时值（RibbonHost `Rc<Cell<bool>>`）→ 保存时写入 layout、启动时读回 | M3 主项 |
| 3 | Ctrl+F1 切换折叠 | 未实现（规格 §4.5 有）；仅在 `collapse_enabled=true` 时生效 | 依赖 4 |
| 4 | 折叠能力开启路径 | `collapse_enabled` 目前 bridge 恒传 false；产品开启 = builder/设置项 → bridge | 依赖 4 |
| 5 | 折叠态临时浮层 | 规格 §4.1（折叠态点 Tab 弹全宽浮层），未实现 | 开启折叠后才有意义 |

M4（上下文 Tab 琥珀色 + 焦点导航细化）、M5（收编策略 B）更远，规格见 ribbon-bar.md。

## 5. 验证命令（每轮必跑）

```bash
# workbench-rs 下：
cargo test -p ribbon -p workbench-ui -p workbench-core --offline
cargo run -p creator --bin creator -- --selfcheck   # 15 PASS / 0 FAIL
# 视觉：启动单实例（先 taskkill 确认无残留）：
cargo build -p creator --bin creator --offline && ./target/debug/creator.exe
```

回归红线：`the_shell_snapshot_covers_every_region`、6 个 bridge 投影测试、
assets_probe 3 项、`cargo tree -p ribbon -e normal` 无 workbench 节点。

## 6. 已知未解（挂账）

- gpui 平台间歇崩溃：一次启动期（0xcfffffff，未复现）+ 关窗期 invalid-handle
  噪声（用户确认无害）。若再遇活体崩溃：`RUST_BACKTRACE=full` + 立即抓日志。
- 诊断面板显示 ~100-180ms/帧（debug 构建 + 3D 视口），未查是否 Ribbon 引入
  （大概率是视口/调试构建本身）。
- Tab 条自身滚动、KeyTips、上下文 Tab 琥珀色：规格标注低优先级/二期。
