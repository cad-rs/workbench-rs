# workbench-rs 开发计划（垂直切片 M1）

对应需求文档：`docs/design.md`。本计划把阶段 0（GUI 选型验证）与阶段 1（平台最小闭环）合并为一个可运行的垂直切片。

## 1. 本期目标与范围

### 1.1 目标

用一个完整切片验证 design.md 的核心架构：

1. 平台核心与 GUI **完全解耦**：Workbench 模块只依赖 `workbench-api`，不依赖 egui/gpui（验收 #21）；
2. **产品构建时固定 GUI**：`product-egui-demo` 与 `product-gpui-demo` 是两个独立二进制，各自只链接一种前端（验收 #1/#22/#23）；
3. 主窗口包含 **工具栏（RibbonBar 简化替身）、四周 Dock Panel、中央多标签内容区、状态栏/任务反馈区**（验收 #3）；
4. Workbench 注册 **命令、工具栏组、Dock Panel、中央视图、文档类型、服务**（验收 #4）；
5. 命令统一入口：工具栏、快捷键、视图内部交互都走命令系统（验收 #5）；
6. 命令支持 **同步立即完成** 与 **后台线程执行**，后台任务有进度、协作式取消、明确终态（验收 #14/#15/#16）；
7. 后台任务通过 **文档版本校验 + 受控提交** 写回文档，不直接改 UI 状态（验收 #17）；
8. 可撤销命令支持 **撤销/重做**，不可撤销命令明确标识（验收 #18）；
9. 布局用 **稳定 ID** 保存/恢复，缺失模块的布局项被跳过而不是启动失败（验收 #8/#9）；
10. 视图通过 **GUI 无关的绘制抽象（PaintBackend）** 渲染，普通视图不接触 wgpu（验收 #10）。

### 1.2 明确不做（留待后续里程碑）

| 项 | 原因 / 计划 |
|---|---|
| RibbonBar（Tab/Group/Contextual Tab） | 用户指定本期用简单工具栏替代；工具栏模型按 Ribbon 形状建模（Group→Items，稳定 ID + 命令绑定），后续可平滑升级 |
| wgpu 渲染服务 | 阶段 2 后；本期 PaintBackend 已预留"渲染能力按需声明"的位置 |
| Python 插件（pyo3 Host API） | 阶段 3；本期先固化注册表/命令/上下文等将被 Host API 复用的接口 |
| 浮动面板、跨区域拖拽停靠 | egui_tiles 已提供树形停靠基础，先验证固定四区 + 区域内标签 |
| 菜单栏、命令面板 | RibbonBar 一起做 |
| Rust 动态库插件 | 非目标（design.md §3.2） |

### 1.3 技术选型（已确认）

- **egui 侧**：`egui` + `eframe`（窗口/事件循环）+ `egui_tiles`（树形 Dock/标签容器）。
- **gpui 侧**：`gpui-ce` 0.2.x（crates.io 社区维护版，声明为 `gpui = { package = "gpui-ce" }`）+ `gpui-ce-platform`（font-kit）+ `gpui_ce_components` 0.2.0（官方 README 推荐组合）。
- 平台核心零 GUI 依赖；仅 `serde/serde_json`（布局持久化）、`rfd`（系统文件对话框，OS 层基础设施）。

## 2. Workspace 结构与依赖方向

```text
crates/
  workbench-api        # 契约层：ID、几何/颜色、PaintBackend、Registry、贡献定义、
                       #   ServiceHost(文档/任务/工作区/日志)、ViewInstance、CommandCtx、Workbench trait
  workbench-core       # 运行时：产品装配、命令注册表+执行、任务事件泵、
                       #   上下文快照、布局持久化、平台命令(新建/打开/保存/undo/redo/关闭标签)
  workbench-ui-egui    # egui 前端适配器：渲染工作区模型 + PaintBackend 实现
  workbench-ui-gpui    # gpui 前端适配器：同上
workbenches/
  wb-example           # 示例 Workbench：只依赖 workbench-api（验收 #21 的硬约束）
products/
  product-egui-demo    # 依赖 core + ui-egui + wb-example
  product-gpui-demo    # 依赖 core + ui-gpui + wb-example
```

依赖方向（与 design.md §5.1 一致）：

```text
product-* ─→ ui-*  ─→ core ─→ api
product-* ─→ wb-example ─→ api     （绝不经过 ui-* / egui / gpui）
```

## 3. 关键设计决策

### 3.1 视图呈现：GUI 无关的 PaintBackend（design.md §9）

`ViewInstance::paint(&mut self, painter: &mut dyn PaintBackend, rect: Rect, ctx: &mut ViewCtx)`。
PaintBackend 提供 fill_rect / stroke_rect / line / circle / text / measure_text 六个原语，
egui 适配器用 `egui::Painter` 实现，gpui 适配器用 canvas 元素 + paint_quad/paint_text 实现。
视图自管命中测试（面板中的"取消"按钮即视图绘制 + 点击命中），证明该抽象足够支撑工程面板的常见交互。
普通视图完全不接触 GPU；wgpu 能力后续作为可选 trait 扩展（`GpuViewInstance`），与本 trait 并列。

### 3.2 视图实例与核心状态的自借用

视图实例（Box<dyn ViewInstance>）存放于核心状态（面板/标签槽位）内，绘制时又需要 &mut 核心状态。
采用**换出-换回**（`mem::replace` 占位）模式：绘制前把实例从槽位取出，绘制后放回。egui/gpui 两端一致。

### 3.3 命令系统（design.md §10）

- `CommandDef { id, title, hotkey, enabled: Option<EnableFn>, kind }`；
- `kind = Sync(handler)`：UI 线程立即执行；`kind = Background(collect)`：UI 线程收集输入并返回 TaskSpec，核心投入后台线程；
- 后台任务经 `TaskCtx`（stage/progress/log/check_cancelled）回报事件，终态为 Succeeded/Failed/Cancelled（§10.2 状态机）；
- 任务结果携带 `DocCommit { doc_id, expect_revision, label, content }`，主线程 frame_tick 泵事件时**校验文档版本**后提交（§10.5），版本过期则拒绝并记日志；
- 启用状态由 `CtxSnapshot { active_document, active_view_type, work_mode }` 驱动，中央标签切换 → 上下文更新 → 工具栏按钮状态刷新（验收 #7）。

### 3.4 文档与撤销（design.md §12.1/12.2）

平台维护通用文档生命周期（新建/打开/保存/脏标记/修订号/标题），领域内容以 `Box<dyn DocumentContent>` 类型擦除存放，
由 Workbench 注册 `DocumentTypeDef { extensions, create_default, serialize, deserialize }` 提供编解码。
撤销采用快照式（`before/after` 内容克隆）——对切片够用；design.md 要求的"不强制全量复制"留作大文档优化点（历史条目可换 delta）。

### 3.5 布局持久化（design.md §6.5）

`layout.json`（产品配置目录）：面板（稳定 ID → 区域/顺序/尺寸/可见）、中央标签（视图类型 ID + 标题）、活动标签、区域活动页、工作模式。
恢复时未注册的 ID 跳过并记日志；每帧可保存、启动时恢复。

### 3.6 工具栏模型（RibbonBar 前身）

`ToolbarGroup { id, title, items: Vec<ToolbarItem::Command { command, label? }> }`——与 Ribbon 的 Group→Item 同构，
升级 RibbonBar 时仅需在外层加 Tab 维度与控件类型，命令绑定与状态机制不变。

## 4. 里程碑与任务分解

| # | 里程碑 | 内容 | 验证 |
|---|---|---|---|
| M0 | 骨架 | workspace、7 crate + 2 product、依赖组合（gpui-ce 官方组合 / egui_tiles）解析通过 | `cargo check` |
| M1 | api 契约层 | ID/几何/PaintBackend/Registry/ServiceHost/任务与文档模型 | 编译 + 单测 |
| M2 | core 运行时 | 装配、命令执行、任务泵、快照、平台命令、布局持久化 | `cargo test`（无窗口可测） |
| M3 | wb-example | 文档类型+6 个演示命令+4 个面板+2 个中央视图 | selfcheck |
| M4 | egui 前端 | 工作区渲染、PaintBackend、快捷键、egui_tiles 停靠 | 编译 + smoke |
| M5 | gpui 前端 | 同上（gpui-ce + components 组合） | 编译 + smoke |
| M6 | 收尾 | 全量测试、selfcheck(--selfcheck 无头自检)、smoke(--smoke 2s 自动退出)、文档 | 汇总报告 |

## 5. 验收映射（design.md §17 → 本期实现）

- #1/#22/#23 → 两个独立 product crate，各自唯一 GUI；
- #3/#4/#5 → 工具栏 + Dock + 中央标签 + 命令系统统一入口；
- #6/#7 → 打开/激活/重排/关闭标签，切换即刷新 CtxSnapshot 与按钮状态；
- #8/#9 → layout.json 稳定 ID 持久化，未知 ID 跳过；
- #10 → PaintBackend 普通 GPU 无关视图；
- #14/#15/#16 → Sync/Background 命令、进度、取消、终态、状态栏展示；
- #17 → DocCommit + revision 校验；
- #18 → 快照式 undo/redo + 不可撤销命令标识（`CommandDef::undoable=false` 只记日志不入历史）；
- #19/#20 → 插件机制本期未涉及，Registry 预留初始化错误隔离（catch_unwind）与 API 版本字段；
- #21 → wb-example 的 Cargo.toml 无任何 GUI 依赖（CI 可断言）。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| gpui-ce API 与记忆不符、迭代快 | 先 `cargo fetch` 后直接读 `~/.cargo/registry/src` 中的真实源码再写适配器；先编译最小窗口骨架再堆功能 |
| gpui_ce_components 与 gpui-ce 版本不统一导致类型分裂 | 依赖统一放 workspace.dependencies；`cargo tree -i gpui-ce` 断言单一实例 |
| egui_tiles 与自有工作区模型冲突 | 核心保存"逻辑布局"（面板→区域），egui_tiles 只作为 egui 端的物理布局器；gpui 端用自绘简单布局，两端不要求像素一致（§6.6） |
| 平台抽象过度 | 严格按验收清单裁剪，没有 UI 需求的接口一律不加 |

## 7. 阶段 2 实施记录（异步命令与事务，已完成）

对应 design.md §18 阶段 2 与 §10 命令系统。在 M1 垂直切片基础上补齐：

### 7.1 三档命令执行模型（§10.6 调用路径的完整实现）

| 档位 | API | 载体 | 适用 |
|---|---|---|---|
| Sync | `CommandKind::Sync` | UI 线程立即执行 | 轻量操作（<16ms） |
| Async（新增） | `CommandKind::Async` → `AsyncSpec{ job: TaskCtx → BoxFuture }` | **平台共享异步执行器**（`workbench-core::executor::AsyncExecutor`，async-executor + 2 工作线程） | 轻量并发 IO/轮询，`async_io::Timer` 等异步等待，不独占线程 |
| Background | `CommandKind::Background` → `TaskSpec{ job: &mut TaskCtx → Result }` | 每任务独占线程 | 重 CPU/阻塞 IO |

### 7.2 统一命令调用结果（§10.1/§16.4）

- `execute_command_tracked() → CommandInvocation { record, result, task_id }`：同步命令立即终态；异步/后台返回任务句柄；
- 平台维护**调用历史环形缓冲**（100 条）：`recent_invocations()` 可查询每次调用的 `execution/status/task_id/note`；
- 异步与后台任务的终态事件自动推进对应调用记录（Succeeded/Failed/Cancelled + 备注），未注册命令也留有 Failed 记录。

### 7.3 文档事务（§10.5/§12.2）

- `DocCommit { expect_revision }` 版本校验提交（M1 已有）；本期补充 `DocumentService::revision()` 查询 API；
- 过期版本/已关闭文档一律拒绝并记录警告日志（策略：Reject；领域模块参与定义的其他策略留扩展点）。

### 7.4 验证

- 新增 5 项单元测试：异步完成+记录终态、异步协作取消、执行器并发（4 任务/2 线程池）、调用历史环形容量+未注册记录、修订号 API；
- selfcheck 扩展至 **12 项**（新增 #10 异步命令、#11 统一调用结果、#12 修订号 API），egui/gpui 双端 12/12 通过；
- 双 GUI `--smoke` 通过，进程干净退出（执行器线程经 shutdown 标志退出，产品入口 `process::exit(0)` 兜底）；
- demo 新增「异步扫描(执行器)」命令，任务面板显示任务载体标签（`[异步]`/`[线程]`）。

## 8. 阶段 3 实施记录（Python 插件闭环，已完成）

对应 design.md §11 与 §18 阶段 3。新增 crate `workbench-python`（pyo3 0.29 + auto-initialize）。

### 8.1 插件模型

| 环节 | 实现 |
|---|---|
| 清单 | `plugin.toml`（§11.2 字段：id/name/version/api_version/entry_point/compatibility/permissions），`toml` 解析 + 校验（空 ID、非法字符、入口格式） |
| 发现 | `discover(dirs)`：目录列表下每个含 `plugin.toml` 的子目录；产品发现路径 = 仓库 `plugins/` + `%APPDATA%/<product-id>/plugins` |
| 版本检查 | `api_version != "1"`（`HOST_API_VERSION`）→ 拒绝加载并给出可读原因（验收 #20） |
| 生命周期 | importlib 从文件加载模块 → 调用 `register(api)` → 收集请求 → 统一落盘到 `Registry`；重复 ID 跳过 |
| 错误隔离 | 清单/加载/执行失败全部进入 `plugins_rejected`（原因可查询），应用继续运行（验收 #19） |
| 来源标记 | `CommandDef::source` / `ToolbarGroup::source`；`remove_commands_from_source()` 支持卸载清理（§16.2） |

### 8.2 Host API v1（design.md §11.3/§11.4 的子集）

```python
def register(api):
    api.log_info("…")
    api.register_command(id, title, callback)                    # 同步：UI 线程，可返回字符串备注
    api.register_command(id, title, callback, background=True)   # 后台：callback(task, args) 任务线程
    api.add_toolbar_item(group, command, label=None)             # 工具栏贡献（组不存在则创建）
    api.api_version                                              # 宿主 API 版本
# task 对象：report(fraction, stage) / set_stage / log / check_cancelled()（抛 PluginCancelled）/ is_cancelled()
```

- Python 回调期间不持注册表借用——请求先收集（`Collected`），register 返回后统一应用（重复 ID/来源校验走平台路径）；
- 协作式取消：`task.check_cancelled()` 抛 `PluginCancelled`，宿主映射为任务 Cancelled 终态；
- GIL 边界：同步插件命令在 UI 线程 attach；后台命令在任务线程 attach（CPython 周期性让出 GIL，UI 不会被长时间阻塞；重计算任务应走 background 档）。

### 8.3 演示插件与验证

- `plugins/demo-plugin/`：同步命令 `demo.hello`（返回备注进调用历史）+ 后台进度任务 `demo.python_progress` + 工具栏组 `plugin.demo`；
- selfcheck 扩展至 **14 项**（#13 发现/注册/工具栏贡献、#14 同步备注 + 后台任务完成），egui/gpui 双端 14/14；
- `workbench-python` 专项测试 **9 项**：清单解析/版本拒绝/ID 校验、好插件装载与来源标记、损坏语法隔离、重复 ID 跳过、同步命令备注进历史、Python 异常→命令 Failed、后台任务完成与协作取消、来源移除清理；
- 运行前提：Python 解释器目录需在 PATH（本机 `C:\ProgramData\miniforge3`），构建用解释器经 `.cargo/config.toml` 的 `PYO3_PYTHON` 指定。

## 9. RibbonBar 实施记录（design.md §6.1/§7，已完成）

按原计划以简单工具栏过渡，本期升级为完整 RibbonBar。

### 9.1 平台模型（workbench-api）

- `RibbonTab { id, title, groups: Vec<ToolbarGroup>, source }`——Tab 维度叠加在既有 Group→Item 模型上，命令绑定与启用状态机制不变；
- `WorkspaceState::add_ribbon_tab()`：同 ID Tab 合并组、组内同 ID 合并条目（受控贡献 §7.3）；
- `add_toolbar_group()` 成为兼容入口：组落入默认 Tab `app.home`「主页」——平台命令、插件 `add_toolbar_item` 等旧路径零改动迁移；
- `active_ribbon_tab` 持久化进 `LayoutFile`（旧布局文件无此字段回落首个 Tab，未注册 Tab 回落并记诊断）；
- `Registry::add_ribbon_tab()`：装配期收集，`build()` 先注册显式 Tab 再落 legacy 组。

### 9.2 前端渲染（egui / gpui）

- 两端均为「Tab 行 + 活动 Tab 的 Group 行」两段式 Ribbon：Tab 高亮（egui 描边 / gpui 蓝色下划线）、组内命令按钮横排、组名与按钮行等宽居中、组分隔线、悬停 tooltip（命令名 + 快捷键）、命令启用状态实时刷新；
- wb-example 分两个 Tab：「主页」（文件/编辑/图形 + 平台「窗口」组 + 插件 `plugin.demo` 组）、「工具」（视图/演示）。

### 9.3 验证与修复

- 新增 2 项 Ribbon 单元测试（Tab 合并/legacy 兼容/活动 Tab 切换/布局往返/未注册 Tab 回落），core 18 项、python 9 项全过；
- selfcheck 扩展至 **15 项**（#15 Ribbon 模型检查），egui/gpui 双端 15/15；
- **真实渲染经 GPU 回读截图确认**（见 §9.4 方法）：两端的 Tab 行、5 个命令组、组名、禁用态按钮（撤销/重做按上下文置灰）、插件贡献组、中文文本全部正确。

### 9.4 本期排障记录（重要经验）

1. **GDI 截屏读不到 GPU 窗口**：`CopyFromScreen`（白）与 `PrintWindow`（黑）均无法捕获 Directx/GL 交换链内容，造成"窗口白屏"的误判。可靠方法：① `EFRAME_SCREENSHOT_TO=<path>` 环境变量（eframe 内置 GPU 回读，退出时存 PNG）；② egui_kittest 离屏渲染（已验证后移除，保留记录）。
2. **egui 缺 CJK 字形**：内置字体无中文，已加载系统字体回退（msyh.ttc 等 7 个候选路径，跨平台）。
3. **egui 反应式渲染 vs smoke 退出**：界面静止后 egui 不产帧，`logic()` 不被调用，smoke 的 Close 永不发送（旧版写死 2.5s 恰在活动帧窗口内掩盖了此问题）。修复：smoke 期间 `request_repaint_after(200ms)`；同时 `--smoke` 支持自定义秒数（`--smoke [secs]`，默认 2.5），此前秒数参数被忽略。
4. eframe 切换为 **glow 后端**（wgpu 呈现在本机同样白屏但无诊断信息；glow 为 egui 官方传统后端，路径更保守）。

## 10. 阶段 5 实施记录（生产级扩展第一期，已完成）

对应 design.md §18 阶段 5。按"真实产品需要"裁剪落地 5 项，1 项设计预留。

### 10.1 设置服务（§12.4）

- `api::Settings`：扁平点分键的 JSON 持久化（`<配置目录>/settings.json`），写入即落盘；
- 首个消费方：`autosave.interval_secs`（默认 30，最小 5）。

### 10.2 自动保存 + 崩溃恢复（§12.1/§18）

- 会话标记：`<配置目录>/session.clean`——build 时删除、Drop 时写入；缺失即判定异常退出；
- 自动保存：`frame_tick` 按间隔对**脏文档**序列化到 `<配置目录>/autosave/`（内容文件 + JSON 旁车元数据），不改文档 dirty/path；
- 启动恢复：无 clean 标记时自动打开自动保存文档，标题加 `[恢复]` 前缀并标记未保存（保存失败保留未保存状态的既有语义不变），恢复后清理目录；
- `app.autosave.now` 命令手动触发；selfcheck 以**子进程自举**验证全链路（A 会话脏文档→自动保存→删标记模拟崩溃→B 会话恢复）。

### 10.3 插件运行时管理（§16.2/§18）

- `AppServices::disable_plugin(id)`：移除插件命令（`Registry::remove_commands_from_source`）、清理 Ribbon 组/Tab（`WorkspaceState::remove_commands`）、更新 `host.plugins` 状态镜像；
- 平台命令 `app.plugins.disable`（args: id）；
- 平台「插件」面板：列出 Loaded/Rejected/Disabled 及原因，Loaded 项提供「禁用」按钮（面板内自绘 + 命中测试）。

### 10.4 诊断面板 + 帧统计（§16.3/§18）

- `ServiceHost::record_frame(dt_ms)`：前端每帧上报（egui logic / gpui render），维护帧数与 EMA 平均帧耗时；
- 平台「诊断」面板：GUI 后端标签（`builder.with_backend_label`）、运行时长、帧统计、文档/未保存数、后台任务、自动保存状态与间隔。

### 10.5 自动化与性能测试（§16.4/§18）

- `core/tests/perf.rs` 3 项：1 万次同步命令分发（µs 级/次）、20k 元素大文档 200 次可撤销编辑 + 满容量撤销/重做、100 个并发后台任务泵——带宽松量级断言防退化；
- `scripts/verify.ps1`：check → 全量 test → 双 selfcheck → 双 smoke 一键验证（含 Python PATH 处理）；
- `[profile.release]` 调优（thin LTO、codegen-units=1、strip debuginfo）。

### 10.6 设计预留（本期未实现，触发条件与形状）

| 项 | 触发条件 | 设计草图 |
|---|---|---|
| 不可信插件进程隔离（§11.5"如需要"） | 出现运行不可信插件的真实需求 | manifest 增加 `sandbox = "process"`；产品二进制 `--plugin-worker <dir>` 子进程模式内嵌 pyo3 加载插件，宿主经 stdio JSON-RPC 转发 register/call/progress；命令调用走现有 InvocationRecord 通道 |
| 插件来源管理与更新 | 插件分发渠道建立后 | `plugin.toml [source]` + 本地 repo 目录比对版本（无网络依赖的增量） |
| GPU 资源恢复/视图诊断 | wgpu 渲染服务落地后（§9.4） | 渲染服务持有设备生命周期，诊断面板扩展 GPU 适配器/视图资源条目 |
| 多平台打包 | 多平台发行需求确立后 | cargo-dist 或 per-target 脚本；Windows 包已可由 release profile + verify.ps1 产出 |
