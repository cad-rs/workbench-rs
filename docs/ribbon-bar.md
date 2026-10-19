# RibbonBar 开发文档（workbench-ui）

> 状态：设计稿（待评审）
> 关联：`docs/design.md` §4.3 / §6.4 / §7（契约）、`crates/workbench-api/src/workspace.rs`（数据模型）、
> `crates/workbench-ui/src/lib.rs`（现有简化渲染，将被本组件替换）
> 产品：creator（经典 3D CAD 应用壳，gpui 前端）

## 0. 现状与差距

**已具备（平台层，无需改动即可复用）：**

- 数据模型完整：`RibbonTab`（稳定 ID + 标题 + 组列表 + 来源标记）、`ToolbarGroup`（稳定 ID +
  标题 + 条目 + 来源）、`ToolbarItem`（命令 ID + 可选覆盖标签）。
  同 ID Tab 合并组、同 ID 组合并条目（design.md §7.3 受控贡献）；插件卸载清理
  （`WorkspaceState::remove_commands`）；活动 Tab 持久化（`LayoutFile.ribbon_active_tab`）。
- 命令系统链路：`CommandId → Registry 注册 → execute_command / is_enabled / command_title /
  command_icon`，热键分发已接通。
- 注册入口：`Registry::add_ribbon_tab`（wb-example 已有两个 Tab 的示范用法）。

**差距（本组件要补齐的）：**

| # | 差距 | design.md §7.2 要求 | 现状 |
|---|------|--------------------|------|
| G1 | 按钮形态只有一种（小按钮：左图标右文字平铺） | 大/小按钮分级、分组标题、分隔线 | 无形态区分 |
| G2 | 无 Split Button / Menu Button / Toggle / Combo | 明确要求（含受限声明式控件） | 无 |
| G3 | 无 Ribbon 折叠（collapse/minimize） | 产品级显示偏好 | 无 |
| G4 | 无溢出处理（overflow） | 紧凑呈现或溢出 | 无（过窄直接裁切） |
| G5 | 无上下文 Tab（contextual tabs） | 随工作模式/文档类型/选择出现 | 无 |
| G6 | 无键盘导航 / KeyTips | 热键与键盘导航惯例 | 仅全局热键 |
| G7 | 无 Tooltip（ScreenTip） | —（CAD 应用标配） | 无 |
| G8 | 内联在 `RootView::render`（~130 行） | —（可维护性） | 需抽为独立组件 |

## 1. 设计参考与定位

creator 的定位是**经典 CAD 桌面应用**（SolidWorks / Inventor / Office Fluent 一脉），
不是 Web 应用顶栏。因此 Ribbon 采用**经典 Office Fluent 形态**而非简化工具条：

- 两层结构：**Tab 条**（页签）+ **面板区**（当前 Tab 的 Group 阵列）；
- Group 内按钮分**大/小**两档，Group 底部有**组标题**，Group 之间有竖分隔线；
- 支持折叠成只剩 Tab 条（最大化视口空间——CAD 用户的高频动作）；
- 命令一律经命令系统执行，Ribbon 自身不持有业务逻辑（design.md §6.4）。

不做的：Office 圆形应用按钮/Application Menu（用后续的菜单系统代替，Logo 点击为其
预留入口）、Gallery 实时预览控件（预留枚举位，不做实现）。QAT 快速访问工具条做
**产品静态集**（首版）：产品经 API 声明若干高频命令，显示为 Tab 条内的 Icon 档小
按钮，折叠态仍可见；用户自定义增删与持久化二期。

## 2. UI 外观（Appearance）

### 2.1 整体布局

Ribbon 位于窗口顶部、标题栏之下、停靠区之上，纵向两段：

```text
┌──────────────────────────────────────────────────────────────────┐
│ 标题栏（系统标题栏，非本组件范围）                                  │
├──────────────────────────────────────────────────────────────────┤
│ ▸Tab 条  [Logo][QAT: ↶ ↷ 💾 │]  [主页]  [建模]  [草图]  [视图]   [▼折叠钮] │  h≈30px
├──────────────────────────────────────────────────────────────────┤
│ ▸面板区                                                           │
│  ┌─────────┐  ┌───────────────┐  ┌──────────┐              │
│  │ ⬛ 新建  │  │ ↶   ↷         │  │ ▦ ▦      │   │  h≈98px      │
│  │  打开    │  │撤销  重做      │  │ ▦ ▦ 图形 │              │
│  │  保存    │  │               │  │          │              │
│  └─────────┘  └───────────────┘  └──────────┘              │
│    文件            编辑                图形                       │
├──────────────────────────────────────────────────────────────────┤
│ 停靠区 / 中央标签区（dock，非本组件范围）                           │
└──────────────────────────────────────────────────────────────────┘
```

### 2.2 尺寸规格（逻辑像素）

| 元素 | 规格 |
|------|------|
| Tab 条高度 | 30px |
| Tab 页签内边距 | 水平 12px，垂直 6px |
| Logo 图标 | Tab 条最左，20×20，左边距 8px；点击行为见 §4.1 |
| QAT 按钮 | Icon 档规格（16px 图标，22px 点击区），垂直居中于 Tab 条；按钮间距 2px |
| QAT 与 Tab 分隔 | 1px 竖线（`ribbon_group_sep`），QAT 区与首个 Tab 之间，左右各 6px 边距 |
| 面板区高度 | 110px（大按钮 86 = 图标区 48 + 两行标签 26 + chevron 预留条 12，+ 组标题 16 + 上下边距 8） |
| 大按钮（Large） | 总高 86px，**所有大按钮一致**：图标区 48×48（图标 24×24 居中）、标签区 26px（两行 11px，行高 13px，居中，第三行裁切）、chevron 预留条 12px（无 chevron 的按钮该条留空）——有无 chevron 不改变图标/文字位置 |
| 小按钮（Small） | 16px **图标在左** + 标签在右，水平排列，高 22px；组内纵向堆叠（每行一个） |
| 小图标按钮（Icon） | 仅 16px 图标、无标签，高 22px，宽约 22px（图标 + 内边距）；组内纵向堆叠，行高与 Small 一致，可与 Small 同列混排 |
| Combo 选择框 | 高 22px（与 Small 行同排）：值区 + 12px chevron；默认宽 120px（min 60 / max 300，声明可覆盖），列宽计入该列最宽行 |
| Split Button（大） | 同 Command 大按钮布局（86px），chevron 渲染在标签下方的预留条中（6 点位，整宽居中）：主区/标签点击执行默认命令，chevron 条弹菜单 |
| Split Button（小） | 16px 图标区 + 8px 箭头区（Icon 档同理，无标签段） |
| Split Button（小） | 16px 图标区 + 8px 箭头区（Icon 档同理，无标签段） |
| Group 标题 | 10px，居中，颜色 `text-secondary` |
| Group 竖分隔线 | 宽 1px，高为面板区内容高，颜色 `border` |
| 组内边距 | 水平 8px，垂直 4px |
| 面板区左右边距 | 8px；Group 间 8px |

### 2.3 Group 布局算法

Group 按声明顺序从左到右排布；Group 内部是一个**纵向槽位列表**：

- Large 按钮 / Split(Large) 各占一整列（列宽 = 48px）；
- Small / Icon / Combo 按钮纵向堆叠，2~3 行成一组列（列宽自适应，取该列最宽行；
  Icon 行宽 22px，Combo 行宽为其声明宽）；
- 声明顺序即列顺序：`[Large A][Small B / Icon C 两行][Large D]…`。

模型层不引入新的布局概念——`ToolbarItem` 增加形态字段（见 §6），布局由形态推导：
`Large → 独占列`；`Small → 依次向下填充，填满 3 行开新列`。

### 2.4 按钮形态总表

按钮有**形态**与**尺寸档**两个正交维度。尺寸档：Large（24px 图标上置 + 标签下置）/
Small（图标 + 标签同行）/ **Icon（仅图标、无标签）**。Large 恒有标签，Icon 只存在于
Small 尺寸。

| 形态 | 结构 | Icon 档示例（CAD） | 典型用途 |
|------|------|--------------------|---------|
| `Command` | 图标 + 标签，点击执行命令 | 草图绘制（直线/圆/圆弧/矩形）、正交/极轴捕捉、线框/着色 | 新建、保存、撤销 |
| `Toggle` | 同上 + checked 态（accent 底 + accent 描边） | 栅格显示、坐标球、约束可见性 | 正交模式、栅格捕捉 |
| `Split` | 主按钮执行默认命令（默认项 = 上次使用）；箭头区弹出子命令菜单 | 圆族/圆弧族/倒角族 flyout（主区图标 = 上次用的方式） | 拉伸（保留上次方式） |
| `Menu` | 整体只弹出菜单，不直接执行 | 视图方向（主视/俯视/轴测…）、标注样式族 | 导出为… |
| `Combo` | 选择框：值区显示当前值 + chevron，点击弹选项列表 | —（恒带值区，无 Icon 档） | 图层、线宽、捕捉模式、视图比例 |
| `Launcher` | Group 右下角 8px 小图标，弹出该组的扩展对话框 | — | 组尾小箭头 |

Icon 档的判据是**图标自明性**，不是命令类型。CAD 草图工具是 Icon 档的典型主场——
直线/圆/圆弧/矩形/捕捉/约束这些图标在 CAD 用户中有强约定（AutoCAD/SolidWorks 的
草图与注释 ribbon 本就大量 icon-only 按钮，圆/弧/倒角族以 flyout 形式纯图标呈现），
不需要文字也不会不知所云。反之，图标画不清楚的专有命令才必须用 Small。无论哪档，
可发现性兜底都是 Tooltip（§4.3），Icon 档尤其不能省。图标缺失时 Icon 档按钮退化为
显示标签首字占位（16px 方块 + 首字），不允许渲染成空白死区。

`Menu` 弹层内容与 Split 的下拉共用同一渲染：命令 ID 列表（可带分隔线分组），
条目 = 图标 + 标签 + 可选热键提示，点击执行并收起弹层。收编策略（§4.5 策略 B）的
溢出下拉同构复用此渲染。

## 3. 视觉风格（Style）

### 3.1 颜色 token

延续平台现有深色基调（`COL_BG` rgb(0.16,0.17,0.20) / `COL_TOOLBAR` rgb(0.20,0.21,0.25) /
`ACCENT` rgb(0.2,0.55,0.95)），在 `workbench-api` 集中定义为命名 token，组件内不出现裸色值：

| Token | 值（首版） | 用途 |
|-------|-----------|------|
| `ribbon_bg` | 复用 `COL_TOOLBAR` | Ribbon 整体背景 |
| `ribbon_tab_strip_bg` | `COL_BG` | Tab 条（比面板区深一档，形成层次） |
| `ribbon_tab_text` | `LIGHT_GRAY` | 非活动 Tab 文字 |
| `ribbon_tab_active_text` | `WHITE` | 活动 Tab 文字 |
| `ribbon_tab_active_bar` | `ACCENT` | 活动 Tab 底部 2px 指示条 |
| `ribbon_tab_hover_bg` | rgb(0.24,0.25,0.30) | Tab 悬停 |
| `ribbon_btn_rest` | 透明（融入 `ribbon_bg`） | 按钮静默态 |
| `ribbon_btn_hover` | rgb(0.30,0.32,0.38) | 按钮悬停（圆角 3px） |
| `ribbon_btn_pressed` | rgb(0.26,0.28,0.33) | 按下 |
| `ribbon_btn_checked_bg` | `ACCENT` @ 25% alpha + `ACCENT` 边框 | Toggle 选中 |
| `ribbon_text` | `WHITE` | 按钮标签（可用） |
| `ribbon_text_disabled` | `GRAY` | 禁用标签/图标 |
| `ribbon_disabled_bg` | 透明 | 禁用不铺底、只降文字/图标 |
| `ribbon_border` | rgb(0.10,0.10,0.12) | Ribbon 底边 1px（现有值） |
| `ribbon_group_sep` | rgb(0.30,0.31,0.36) | Group 竖分隔线 |

主题（浅色）二期再做——token 化是为了那一期不返工。

### 3.2 状态矩阵

每个按钮渲染按 `(enabled, state, checked)` 三元组：

| 状态 | 背景 | 图标 | 标签 | 备注 |
|------|------|------|------|------|
| rest（可用） | 透明 | WHITE | WHITE | |
| hover | `btn_hover` | WHITE | WHITE | 圆角 3px |
| pressed | `btn_pressed` | WHITE | WHITE | |
| disabled | 透明 | GRAY | GRAY | 不响应点击，无 hover |
| checked | accent 25% + accent 边框 | WHITE | WHITE | 仅 Toggle |
| focus（键盘） | 同 rest + 1px accent 内描边 | | | 键盘导航时可见 |

### 3.3 图标

- 单色 SVG，跟随文字色（gpui `svg().text_color()`，与现有 15px 图标同机制）；
- 两档尺寸：大按钮 24px、小按钮 16px；SVG 本身按 24 网格绘制，小档等比缩小；
- 路径来自产品的 `AssetSource`（`icons/*.svg`），缺失时静默不绘制（现有行为），标签仍显示；
- 禁用态不做单独图标资源，统一用颜色降级。

### 3.4 文字

系统字体栈（`.SystemUIFont`，Windows 上 Segoe UI，与现状一致）。字号：
Tab 12.5px / 大按钮标签 11px / 小按钮标签 11.5px / Group 标题 10px / 弹层热键提示 11px。
大按钮标签**换行最多两行**（Fluent 惯例），仍超宽截断加省略号（`…`）；小按钮标签单行
截断；均不缩放。

## 4. 交互行为（Interaction）

### 4.1 Tab 条

| 交互 | 行为 |
|------|------|
| Logo 点击 | 首版静态展示；应用菜单（菜单系统）落地后接为其入口（§1） |
| QAT 按钮点击 | 执行绑定的命令（同 §4.2 链路）；**折叠态仍可见可用**——这是 QAT 的存在意义。首版为产品静态集（`WorkspaceState.qat`，§6），用户自定义增删二期 |
| 点击 Tab | `set_active_ribbon_tab(id)` → 模型变更 → 重绘面板区；活动 Tab 指示条移动 |
| 折叠钮（Tab 条右端 ˇ 图标） | 切换折叠态：面板区隐藏，Ribbon 只剩 30px Tab 条 |
| 双击活动 Tab | 同折叠钮（Fluent 惯例） |
| 折叠态点 Tab | **临时展开**：面板区以浮层（overlay）盖在 dock 上方显示，点 Ribbon 外任意处或再点同 Tab 收回；不改变折叠态 |
| 折叠态点其他 Tab | 同上，浮层内容切到新 Tab |
| 持久化 | 折叠态 + 活动 Tab 一起存进 `LayoutFile`（活动 Tab 已有字段；折叠态新增） |
| 恢复 | `apply_layout` 回落规则沿用现有实现（未注册 Tab 回落首个） |

### 4.2 命令按钮

- **点击** → `core.execute_command(cmd, args)` → 失败写 `host.log` → `notify` 重绘。
  完全复用现有链路（lib.rs L701-720 的模式原样搬入组件），Ribbon 不理解命令语义。
- **可用性**：每帧从 `is_enabled(cmd)` 读取；命令系统负责随文档/选择状态翻转。
- **Toggle**：点击同样只发命令；**checked 态是命令系统的查询结果**（见 §6 新增
  `is_checked`），Ribbon 不自己维护开关状态——避免与文档状态双源。
- **Split**：点击主区执行默认命令；点箭头区弹出菜单；菜单条目点击执行对应命令。
- **Menu**：点击弹菜单。弹层为 gpui-component `popover`/`menu`（vendored，同窗渲染）。
- **Combo（选择框）**：点击弹选项列表，定位走 §4.4 锚定规则；**当前值与选项列表从
  命令系统查询**（`combo_state`，与 Toggle 的 `is_checked` 同模式，§6）——Ribbon
  不持有选项数据，图层增删等文档状态变化自然反映；选中项执行
  `execute_command(cmd, args)`（选中值经 CommandArgs 传入）并收层；当前项在列表中
  带勾选标记，选项条目渲染同 Menu（§2.4）。禁用态同按钮（`is_enabled`）。首版
  select-only（不可键入编辑），`editable` 留字段二期。

### 4.3 Tooltip（ScreenTip）

悬停 ≥500ms 显示：**命令标题**（`command_title`，粗体）+ 热键（如有，`Ctrl+S` 形式）+
可选描述（命令系统二期字段，先留空不显示）。Icon 档按钮（无标签，§2.4）的 Tooltip
是其**唯一**文字说明入口，M2 必须先做。空间不足场景（§4.5）：滚动策略（默认）下被
移出视野的按钮无法悬停，其可发现性由端点箭头与键盘 scroll-into-view 承担；收编策略
下溢出钮常驻可见，其 Tooltip 列出被收编组的清单。

### 4.4 弹层定位（Dropdown Alignment）

Ribbon 有四类弹层（Split/Menu 下拉、折叠态 Tab 浮层、Tooltip、收编策略下的溢出
Group 下拉），前三类共用下表的定位规则：

| 维度 | 规则 |
|------|------|
| 垂直 | 弹层顶边 = 触发按钮底边 + 4px；**恒向下展开，不向上翻转**（Ribbon 在窗口顶部，向下是唯一自然方向；窗口过矮时限制弹层最大高，内容内部滚动） |
| 水平首选 | **左对齐**：弹层左边缘 = 按钮左边缘（Fluent 惯例，与触发钮的视觉锚定最强） |
| 右溢出翻转 | 弹层右边缘超出窗口内容区右边缘 - 8px 时改**右对齐**（弹层右边缘 = 按钮右边缘）；仍溢出则右边缘钳制到内容区右边缘 - 8px |
| 左溢出钳制 | 按钮贴左且弹层宽于按钮时，左对齐后若弹层左边缘 < 8px，钳制到 8px |
| 宽度 | 跟随内容：min 160px、max = 内容区宽 - 16px；**不拉伸到按钮宽度**（菜单宽与按钮宽无关，Office 惯例） |
| 边界参照 | **窗口内容区**，不是屏幕——窗口化应用不做屏幕级/多显示器避让 |
| 唤起方式 | 点击与键盘（Enter/↓）同位同规则 |
| 互斥与关闭 | 全局至多一个弹层：开新关旧；点击弹层外任意处、Esc、滚动、切 Tab、折叠切换 → 关闭 |

特例：

- **溢出 Group 下拉（仅收编策略，§4.5 策略 B）**：触发钮恒在窗口右缘，实际总走
  右对齐/钳制分支。
- **折叠态 Tab 浮层（§4.1）**：不做按钮锚定——顶边贴 Tab 条底边，左右**铺满窗口
  内容区全宽**（Office 折叠 ribbon 的行为）；最大高 = 窗口高 - Tab 条 - 状态条 - 8px。
  窄窗口下浮层内部按 §4.5 当前策略处置。
- **Tooltip（§4.3）**：唯一非锚定弹层，跟随光标（右下偏移 12px），贴边时钳制。

实现注记：vendored gpui-component 已备承载件——`Popover`（`.anchor(Anchor)` 角锚定）、
`PopupMenu`（条目自带 icon/checked/disabled/分隔线，`min_w/max_w` 可调）与 `Select`
（Combo 的值区/列表），可直接映射本节规则；库的翻转/钳制行为若与上表有出入，以本表
为准（在 ribbon crate 内按规则计算 anchor corner 或自行定位），不改 vendored 源码。

### 4.5 空间不足策略：横向滚动（默认）/ Group 收编（可选）

窗口宽度不足时的处置策略有两档，按显示偏好二选一，产品默认**滚动**（对应
design.md §7.2 的 "product defaults and user display preferences"）。两档共用
"不缩小按钮、不重排"的底线——按钮尺寸与相对位置不随窗口宽度漂移。

**策略 A：横向滚动（默认）** —— SolidWorks CommandManager 同策略。默认它：

- 图标**相对位置永不变**——命令只是暂时移出视野，空间记忆的彻底形态；
- 不存在 Group → 菜单条目的形态映射变换（易错且难测）；
- CAD 先例充分（SolidWorks / FreeCAD / Rhino 工具条均滚动）。

| 维度 | 规则 |
|------|------|
| 触发 | 内容总宽 > 面板区宽 W 时进入可滚动状态，可滚动方向的一端出现 ‹ › 箭头钮（覆盖在内容边缘、半透明底），内容放得下时隐藏 |
| 箭头点击 | 向该方向滚动约一屏的 80%（平滑动画 ~150ms）；已到头的一侧箭头呈禁用态 |
| 滚轮 | 水平滚轮 / 触控板横扫直接滚动；**Shift + 垂直滚轮**滚动；**裸垂直滚轮不滚动**（CAD 习惯：滚轮预留给视口语义，避免悬停 Ribbon 时误触） |
| 范围钳制 | 滚动偏移 ∈ [0, 内容总宽 - W]；resize 时保持偏移并重新钳制 |
| 键盘到达 | 焦点导航（§4.6）进入被裁按钮时自动 scroll-into-view（最小滚动量） |
| 边缘裁切 | 视口边缘允许半截按钮（滚动的自然形态）；半截按钮同样响应点击与 Tooltip |
| 持久化 | 滚动偏移是瞬态，不进布局文件；折叠浮层每次打开从偏移 0 开始 |

已知弱点（有意接受的取舍）：被滚出视野的命令只剩端点箭头一个线索，可发现性靠
"箭头显隐即滚动状态提示 + 键盘 scroll-into-view + 半截按钮 Tooltip"缓解。

**策略 B：Group 收编（可选）** —— Office Fluent 式 overflow：

1. 最右的 Group 整体替换为一个下拉按钮（Group 标题 + chevron），点击弹出该组完整
   内容的浮层（按钮形态映射为菜单条目，Large 优先于 Small；渲染与 Split/Menu 下拉
   同构，§2.4。**Combo 例外：浮层内原样渲染控件**——菜单化会丢失当前值展示）；
   仍不足则继续向左逐组收编，直到放得下为止；
2. **极限情形（W 还是不够）**：允许全部 Group 都被收编，不设保底组——面板区只剩一排
   溢出钮，**面板区高度保持 98px 不变**（避免下方 dock 区随拖窗抖动），溢出钮垂直
   居中；单个 Group 本身宽于 W 的，也只存在于溢出下拉中；
3. 不变量：**整组进出**（Group 要么完整可见、要么整体进溢出，组内按钮永不裁切）；
   溢出钮只在有 Group 被收编时出现，其 Tooltip 列出被收编 Group 的标题清单；
   布局是 W 的确定性函数，增宽按序回扩（最左被收编的组先回面板区）。

选它而非默认的理由（即保留为可选项的理由）：被隐藏的命令有**可见清单**（溢出钮
本体），Fluent 用户熟悉；代价是命令位置随窗口宽度进出面板区、且需要整套形态映射。

**共用规则与切换：**

- 两档下**面板区高度恒 98px、裸滚轮均不滚动**；折叠态浮层（§4.1）内部套用当前策略；
  Tab 条在两档下都独立滚动（低优先级实现），**Logo 与 QAT 区固定在左端，不随 Tab
  滚动**；
- 偏好字段：`ribbon_overflow: "scroll"(默认) | "collapse"`，进布局文件（§6）；
  产品可经 builder 覆盖默认；切换**即时生效**（同帧重排，不重启）。

实现注意：滚动用水平滚动容器 + 端点箭头覆盖层；先全量测内容总宽决定箭头显隐，
一帧测量开销可接受。

### 4.6 键盘

| 键 | 行为 |
|----|------|
| 全局热键 | 已有（窗口级 on_key_down → hotkeys 分发），不动 |
| `Ctrl+F1` | 切换折叠（Office 惯例） |
| KeyTips（`Alt` 触发的字母提示链） | **二期**：首版不做，模型层无需预留（纯渲染层行为） |
| Tab 键焦点导航 | 依托 gpui 焦点系统：Tab 条 → 面板按钮按视觉顺序；focus 态见 §3.2 |

### 4.7 上下文 Tab（Contextual Tabs）

- 触发源是**工作模式**（`work_mode`，已有）：模式切换 → 对应上下文 Tab 出现/消失
  （如「草图」模式显示「草图」Tab）。文档类型驱动二期再接（需文档类型枚举到位）。
- **模型层新增**：`RibbonTab` 加 `contexts: Vec<String>`（空 = 常驻 Tab）；
  可见性 = `contexts` 为空 || `contexts.contains(work_mode)`。渲染层过滤即可，
  不新增 Tab 概念、不破坏现有合并/清理语义。
- 视觉：上下文 Tab 的指示条与标题用第二强调色（如琥珀 rgb(0.9,0.7,0.2)）以示区别
  （Fluent 惯例），首版可先同色，标记 TODO。

### 4.8 插件与贡献（不变）

插件仍只走 `Registry::add_ribbon_tab` / `add_toolbar_group`（合并语义、来源标记、
卸载清理全部沿用）；插件**不能**影响按钮形态以外的渲染（无自定义控件入口——
design.md §7.3 "restricted declarative Controls"，首版只开放 §2.4 的五种形态）。

## 5. 架构与代码落点：复用 gpui-component，与 workbench 解耦

**目标约束：** 组件成熟后贡献到 gpui-component 上游。因此 RibbonBar 从第一天起就是
**纯 gpui / gpui-component 组件**——放在独立 crate，编译层面禁止依赖 workbench-api /
workbench-core（crate 看不到 = 永远耦合不了，靠 review 约束不住的事用编译约束）。

### 5.1 分层

```text
+-- workbench-ui -------------------------------+
|  ribbon_bridge.rs（唯一 workbench 耦合点）      |
|    WorkspaceState 快照 -> ribbon::RibbonModel |
|    ribbon 事件 -> execute_command / 模型方法   |
+-- ribbon（新 crate）--------------------------+
|  纯组件：视图模型类型 + RibbonBar 渲染 + 瞬态   |
|  依赖：gpui、gpui-component（仅此两个）        |
+-- gpui-component -----------------------------+
|  Button / Tab / Popover / PopupMenu / Select  |
+------------------------------------------------+
```

| 位置 | 内容 |
|------|------|
| `crates/ribbon/`（新 crate） | 视图模型自有类型（`RibbonModel`/`RibbonTab`/`RibbonItem`…，§6）+ 渲染（Tab 条、面板区、策略 A/B、折叠、§4.4 弹层定位）+ 内部瞬态状态（滚动偏移；折叠态/活动 Tab 由外部受控传入）。对外接口 = `RibbonModel`（数据）+ 事件回调（`on_command(id, args)` / `on_active_tab_change(id)` / `on_toggle_collapse`） |
| `workbench-ui/src/ribbon_bridge.rs`（新，~200 行） | workbench 的**全部**耦合集中于此：每帧从 `WorkspaceState` 组装 `RibbonModel`（含 `is_enabled`/`is_checked`/`combo_state` 查询结果），事件回调转 `execute_command` / `set_active_ribbon_tab` / `notify`。`RootView::render` 一行挂载；旧内联实现 L627-757 删除（Rule 4） |
| `crates/workbench-api/src/workspace.rs` | 插件贡献 API 不变（`add_ribbon_tab` 等），仅按 §6 扩展字段；bridge 做模型 → 视图模型的逐字段机械映射 |
| `crates/workbench-api/src/command.rs` 或 services | `is_checked(cmd) -> bool`（Toggle）；`combo_state(cmd) -> Option<ComboState{current, options}>`（Combo，每帧查询）；CommandArgs 增加可选 value 槽位（Combo 选中值） |
| `docs/design.md` §7 | 状态回写：实现完成后把「设计意图」段落补上指向本文档的实现注记 |

### 5.2 gpui-component 复用映射

尽量复用 vendored gpui-component 的现成控件，ribbon crate 只写"Ribbon 特有的布局与
粘合"：

| Ribbon 元素 | 复用 | 说明 |
|------|------|------|
| Tab 条与活动指示 | `tab::TabBar` | 活动下划线、hover 态现成；风格差异用 style refinement 调 |
| 大/小/Icon 按钮 | `button::Button` | 大按钮用 children 自定义（图标上、标签下）；hover/pressed/disabled/selected 四态直接获得 |
| Toggle | `Button` selected 态 | 非独立控件——checked 只是 Button 的视觉态，数据来自 `is_checked` 查询 |
| Split / Menu 下拉 | `Popover`（`.anchor`）+ `PopupMenu` | 条目 icon/checked/disabled/分隔线现成（§4.4 实现注记） |
| Combo（select-only） | `select::Select` | 值区 + 选项列表 + 勾选当前项现成；二期 editable 档升级为 `combobox` |
| Tooltip | `tooltip` | 悬停延迟与跟随行为现成 |
| 图标 / 分隔线 | `icon` / `separator` | svg 着色 / 1px 分隔 |

自写的只有四块：Group 列布局（§2.3）、空间不足策略 A/B（§4.5）、折叠浮层（§4.1）、
弹层定位特例（§4.4）——这些正是贡献上游时属于 ribbon 模块本体的内容。

### 5.3 上游贡献路径

成熟后把 `crates/ribbon/` 整体平移进 gpui-component（如 `crates/ui/src/ribbon/`）：
依赖本就只有 gpui + gpui-component，平移是**移动而非重写**；workbench-ui 的 bridge
改一行 import。命名从第一天用上游风格（模块 `ribbon`、类型 `RibbonBar`/`RibbonModel`，
不用 wb 前缀），不引 workbench 概念（命令系统、Registry 等词不进 ribbon 的公共 API）。

### 5.4 架构不变量

- `crates/ribbon/Cargo.toml` 不出现 workbench-*；以 `cargo tree -p ribbon` 为验收
  （依赖树里没有 workbench 节点）；
- ribbon 组件不理解命令语义：它只发 `on_command(id, args)` 事件，执行与结果处理全在
  bridge；
- 视图模型（ribbon crate）与插件贡献模型（workbench-api）是**两套类型**，bridge 逐字段
  映射——这是解耦的代价（每加一种形态两处各加一个字段 + 映射一行），接受；
- 文件 < 2000 行（Rule 5）：ribbon crate 预估 600-900 行，超限按 `tabs.rs` /
  `panel.rs` / `popup.rs` 拆分。

## 6. 数据模型扩展（workbench-api）

向后兼容式扩展——现有注册代码（wb-example、插件）**零改动可编译**。扩展落在
workbench-api（插件贡献模型）；ribbon crate 的视图模型（§5.1）镜像这些概念但独立
定义，bridge 逐字段映射（§5.4）：

```rust
// ToolbarItem 扩展（workspace.rs）
pub enum RibbonItemKind {
    Command,                       // 默认，等价现状
    Toggle,                        // checked 由 is_checked 查询
    Split { items: Vec<ToolbarItem> },      // 下拉子命令
    Menu  { items: Vec<ToolbarItem> },      // 纯下拉
    Combo,                         // 选项/当前值由 combo_state 查询，选中执行命令（带值参数）
}

pub struct ToolbarItem {
    pub command: CommandId,
    pub label: Option<String>,
    pub kind: RibbonItemKind,      // serde(default) = Command
    pub size: ItemSize,            // Large | Small | Icon；serde(default) = Small（与现状视觉最接近）
    pub icon: Option<String>,      // 覆盖命令默认图标（少用，主要为命令系统兜底）
}

// RibbonTab 扩展
pub struct RibbonTab {
    // ...现有字段...
    pub contexts: Vec<String>,     // 空 = 常驻；含 work_mode 名 = 该模式可见
}

// QAT 快速访问（产品静态集，§4.1）：显示于 Tab 条 Logo 之后、1px 分隔线之前，
// 折叠态仍可见。WorkspaceState 增 `qat: Vec<ToolbarItem>`（复用 ToolbarItem，
// 渲染取 Icon 档）；Registry 增 `add_qat_command(cmd)`。用户自定义增删与持久化二期。

// LayoutFile 扩展
pub struct LayoutFile {
    // ...现有字段...
    #[serde(default)]
    pub ribbon_collapsed: Option<bool>,
    /// 空间不足策略（§4.5）："scroll"（默认）| "collapse"。
    #[serde(default)]
    pub ribbon_overflow: Option<String>,
}
```

`Registry` 增加对应 builder 方法（`toggle()` / `split()` / `menu()` / `combo()` /
`large()` / `icon_only()` / `context()` / `add_qat_command()`），现有 `command()` /
`labeled()` 语义不变。

## 7. 实施里程碑

| 里程碑 | 内容 | 验收 |
|--------|------|------|
| M1 骨架重构 | 建 `crates/ribbon` crate（仅 gpui/gpui-component 依赖）+ `ribbon_bridge.rs`；大/小按钮形态 + Group 标题 + 竖分隔线 + 折叠钮静态实现；Tab 条 Logo 与 QAT（产品静态集）；颜色 token 化 | creator 跑起来视觉达标（§3），命令点击全通，旧内联实现删除，`cargo tree -p ribbon` 无 workbench 依赖 |
| M2 状态完备 | Toggle/Split/Menu/Combo + popover 弹层（定位按 §4.4）+ `is_checked`/`combo_state` + Tooltip | wb-example 上四种新形态可用；禁用态正确；下拉翻转/钳制正确 |
| M3 滚动与折叠（默认策略） | 面板区/Tab 条横向滚动（§4.5 策略 A）；弹层定位规则（§4.4）；折叠 + 折叠态临时展开浮层 + 持久化 | 400px~4K 拖窗滚动与箭头显隐正确；折叠态重启恢复；下拉翻转/钳制按 §4.4 全场景正确 |
| M4 上下文 Tab | `contexts` + work_mode 联动 + 键盘焦点导航 | 切模式 Tab 出现/消失；布局持久化往返一致 |
| M5 收编策略（可选） | Group 收编下拉（§4.5 策略 B）+ `ribbon_overflow` 偏好切换与持久化 | 两策略切换即时生效；收编终态/按序回扩正确；偏好往返一致 |

每个里程碑：`cargo check` + `cargo test -p workbench-ui`（现有 `the_shell_snapshot_covers_every_region`
必须保持绿）+ creator 冒烟（`--smoke` 模式已有）。

## 8. 验收清单（视觉 + 交互）

**外观**

- [ ] Tab 条 30px / 面板区 110px，活动 Tab 有 2px accent 底条
- [ ] 大按钮 24px 图标上置、小按钮 16px 左置，Group 底部 10px 标题居中
- [ ] Icon 档按钮仅 16px 图标无标签，与 Small 同列混排行高一致；图标缺失时显示首字占位而非空白
- [ ] Tab 条最左 Logo（20px），QAT 小图标区紧随其后并以 1px 竖线与 Tab 区分隔
- [ ] Group 间 1px 竖分隔线；Ribbon 底边 1px `ribbon_border`
- [ ] 五种按钮形态（§2.4）视觉齐备，hover/pressed/checked/disabled 四态正确
- [ ] 图标缺失时只显示标签，不留空洞

**交互**

- [ ] 点击 Tab 切换面板区；活动 Tab 持久化往返一致
- [ ] 折叠后只剩 Tab 条；折叠态点 Tab 弹浮层，点击外部收回；`Ctrl+F1` 切换
- [ ] QAT 按钮点击执行命令；折叠态下 QAT 仍可见可用
- [ ] 默认（滚动）：窗口收窄时面板区进入横向滚动，端点箭头按需出现、到头禁用；裸滚轮不动、Shift+滚轮/水平滚轮滚动；键盘焦点 scroll-into-view
- [ ] 极限窄窗（滚动）：面板区高度不变（dock 不抖动），仅靠滚动与箭头到达全部命令；折叠态浮层同规则
- [ ] 切换收编策略：逐组收编与全部收编终态正确、溢出钮 Tooltip 列组清单、增宽按序回扩；切换即时生效且偏好往返一致
- [ ] Icon 档按钮悬停出 Tooltip（命令标题 + 热键），是其唯一文字说明入口
- [ ] 命令失败写日志（右下状态条可见）；禁用按钮不可点
- [ ] Split 主区/箭头区点击行为分离；菜单点选执行并收层
- [ ] 下拉左对齐为默认；贴右缘翻转右对齐、仍不够则钳制；弹层互斥（开新关旧），Esc/点击外部/滚动/切 Tab 均关闭
- [ ] Combo：弹层锚定正确、当前项带勾选标记、选中执行带值命令并收层、当前值与选项随命令系统状态刷新、禁用态不可开列表
- [ ] 插件卸载后其贡献的 Tab/组/条目消失（现有 `remove_commands` 行为不回归）
