# workbench-rs

> 仓库：https://github.com/cad-rs/workbench-rs


协议：[MIT](LICENSE)。crate 均以 `workbench-*` 命名发布到 crates.io（见文末发布说明）。
通用工程桌面应用框架（CAD / 机械设计 / 工业仿真方向）的 Rust 实现。
需求与架构见 [docs/design.md](docs/design.md)，开发计划与阶段记录见 [docs/plan.md](docs/plan.md)。

## 仓库结构

```text
crates/
  workbench-api         平台契约层（GUI 无关）：ID、几何、PaintBackend、命令、任务、文档、工作区/Ribbon 模型
  workbench-core        平台运行时：装配、命令执行、任务/异步执行器、自动保存、布局持久化、selfcheck
  workbench-ui-egui     egui 前端适配器（glow 后端 + egui_tiles 停靠）
  workbench-ui-gpui     gpui 前端适配器（gpui-ce + 官方组件库）
  workbench-python      Python 插件宿主（pyo3，Host API v1）
workbenches/
  wb-example            示例 Workbench 模块（包名 workbench-example，只依赖 workbench-api）
products/
  product-egui-demo     示例产品 A（构建时选定 egui）
  product-gpui-demo     示例产品 B（构建时选定 gpui）
plugins/
  demo-plugin           演示 Python 插件（同步命令 + 后台进度任务 + 工具栏贡献）
docs/                   需求 / 计划 / 插件开发指南
```

依赖方向：`product → ui → core → api`，`product → wb-example → api`。
**Workbench 模块永远不接触 egui / gpui / wgpu**——GUI 只在 `workbench-ui-*` 出现。

## 快速开始

```bash
cargo run -p product-egui-demo          # 运行两个示例产品
cargo run -p product-gpui-demo
cargo test --workspace                  # 全量测试
powershell -File scripts/verify.ps1     # 一键验证（check+test+自检+冒烟）
```

## 集成指南

### 1) 产品开发者：装配一个独立应用

每个产品是一个独立 crate，**构建时固定一种 GUI**。产品 `main` 只做装配，CLI
（运行 / `--selfcheck` / `--selfcheck-crash` / `--smoke [秒]`）由平台约定托管：

```rust
use workbench_core::cli::{self, CliMode};

fn main() {
    let cli = cli::from_env();
    let builder = workbench_core::WorkbenchAppBuilder::new("com.example.prod", "我的产品")
        .with_backend_label("egui")                       // 诊断面板显示用
        .with_workbench(my_cad_workbench)                 // 装配 Workbench 模块
        .with_plugin_stage(workbench_python::plugin_stage(
            workbench_core::cli::default_plugin_dirs("com.example.prod")))
        .create_default_document(true)
        .open_on_start("cad.viewport", true);
    let builder = cli.apply(builder);

    match cli.mode {
        CliMode::Selfcheck =>                             // 无头自检（探测式，无关项自动 SKIP）
            workbench_core::selfcheck::exit_with(&mut builder.build().unwrap()),
        CliMode::CrashProbe =>                            // 自检的子进程模式
            workbench_core::cli::crash_probe_exit(|| make_builder()),
        CliMode::Run => {
            workbench_ui_egui::run(builder, "我的产品").unwrap();
            std::process::exit(0);
        }
    }
}
```

完整可运行版本见 `products/product-egui-demo/src/main.rs`（约 50 行）。

### 2) Workbench 开发者：贡献命令 / 面板 / 视图 / 文档类型

Workbench 是普通 Rust crate，**只依赖 `workbench-api`**，实现一个 trait：

```rust
struct CadWorkbench;
impl wb::Workbench for CadWorkbench {
    fn id(&self) -> wb::WorkbenchId { wb::WorkbenchId::new("cad") }
    fn init(&self, reg: &mut wb::Registry) {
        // 命令（统一入口：Ribbon、快捷键、脚本、插件都执行它）
        reg.register_command(
            wb::CommandDef::sync("cad.add_box", "添加方块", |ctx, _args| {
                let doc = ctx.active_document().ok_or("无文档")?;
                ctx.host().edit_document(...)?;
                wb::CommandResult::done()
            })
            .hotkey(wb::Hotkey::parse("Ctrl+B").unwrap())
            .enabled_when(|snap| snap.active_document.is_some())
            .background(...)  // 或异步/后台执行（三档执行模型）
        );
        // Ribbon Tab（组内稳定 ID；旧式 add_toolbar_group 自动落入「主页」Tab）
        reg.add_ribbon_tab(wb::RibbonTab::new("cad.model", "模型").groups(vec![...]));
        // Dock 面板 / 中央视图：实现 ViewInstance::paint（GUI 无关绘制）
        reg.register_panel(wb::PanelDef::new("cad.props", "属性", wb::DockArea::Right, 300.0, 0,
            || Box::new(PropsPanel)));
        reg.register_view(wb::ViewDef::new("cad.viewport", "三维视图", true, || Box::new(ViewportView)));
        // 文档类型（编解码由领域模块定义，平台不侵入数据模型）
        reg.register_document_type(wb::DocumentTypeDef { ... });
    }
}
```

要点：
- 视图通过 `PaintBackend`（fill/stroke/line/circle/text 六原语）绘制，不接触 GUI 类型；
- 命令三档执行：`Sync`（UI 线程）/ `Async`（共享执行器）/ `Background`（独占线程任务，可进度/取消）；
- 文档修改经 `DocumentService::edit/commit`，后台结果经 `DocCommit`（版本校验）提交，自动支持撤销/重做；
- `init` 允许 panic——装配时被捕获隔离，不拖垮产品。

### 3) 插件开发者：Python 扩展

见 [docs/plugins.md](docs/plugins.md) 与可运行示例 `plugins/demo-plugin/`。
宿主能力（Host API v1）：同步/后台命令、进度与协作取消、Ribbon 工具栏贡献；
清单校验、API 版本检查、错误隔离、运行时禁用均已内建。

## 运行与诊断

| 命令 | 说明 |
|---|---|
| `--selfcheck` | 无头自检（探测式：与示例/插件无关的项自动 SKIP），退出码 0/1 |
| `--selfcheck-crash` | 崩溃恢复探针（由自检调用，一般不手动跑） |
| `--smoke [秒]` | 运行指定秒数后自动退出（默认 2.5），CI 冒烟 |
| `RUST_LOG=info` | 输出 wgpu/eframe 诊断 |
| stderr `wb-log [... ]` | 平台日志镜像（终端可见，自动化可 grep） |

> 注意：GDI 截屏（PrintWindow/CopyFromScreen）抓不到 GPU 交换链内容。
> 验证 egui 渲染请用 `EFRAME_SCREENSHOT_TO=<path>` 环境变量 + `--smoke`
> （GPU 回读，退出时存 PNG）。

## Python 插件

`plugins/demo-plugin/` 是可直接运行的示例。运行产品前需保证 Python 在 PATH
（本机为 `C:\ProgramData\miniforge3`）；构建用解释器经 `.cargo/config.toml`
的 `PYO3_PYTHON` 指定。插件开发详见 [docs/plugins.md](docs/plugins.md)。

## 里程碑状态（对 design.md §17 验收映射）

- **阶段 1（垂直切片）**：验收 #1–#10、#12–#18、#21–#23 原型验证通过。
- **阶段 2（异步命令与事务）**：三档命令执行模型、统一命令调用结果与可查询调用历史、`DocCommit` 版本校验事务、文档修订号 API。
- **阶段 3（Python 插件闭环）**：清单与发现、API 版本检查、Host API v1、错误隔离、运行时禁用、演示插件。
- **RibbonBar**：`RibbonTab → Group → Command` 三级模型（稳定 ID、受控合并、活动 Tab 持久化），双端渲染经 GPU 回读截图验证。
- **阶段 5（生产级扩展第一期）**：设置服务、自动保存 + 崩溃恢复、插件管理面板、诊断面板与帧统计、性能回归测试 + 一键验证脚本。
- 待办：wgpu 渲染服务（可选能力）、不可信插件进程隔离（设计草图见 plan.md §10.6）、多平台打包。

## 发布（crates.io）

可发布 crate：`workbench-api`、`workbench-core`、`workbench-python`、`workbench-ui-egui`、`workbench-ui-gpui`、`workbench-example`（全部 MIT，元数据齐全；`cargo package -p workbench-api` 已含构建验证通过）。两个示例产品不发布（`publish = false`）。

内部依赖已声明版本号，发布须按依赖顺序执行：

```bash
cargo publish -p workbench-api
cargo publish -p workbench-core
cargo publish -p workbench-python   # 以及 workbench-example / ui-egui / ui-gpui（此后顺序任意）
```

发布前运行 `powershell -File scripts/verify.ps1 -SkipSmoke` 做最终确认。
