//! 无头自检：不打开窗口，驱动平台核心走一遍关键路径，验证架构闭环。
//! 产品以 `--selfcheck` 调用，输出 PASS/FAIL 行，进程退出码 0/1。
//!
//! 领域内容通过文档类型注册的序列化接口 + 通用 JSON 读取校验，
//! 核心不依赖任何领域 crate。

use std::path::Path;
use std::time::{Duration, Instant};

use workbench_api as api;

use crate::app::AppRuntime;

struct Check {
    name: String,
    ok: bool,
    skipped: bool,
    detail: String,
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self { name: name.into(), ok, skipped: false, detail: detail.into() }
    }
    /// 前置条件不满足时跳过（探测式自检：任何产品都能运行，无关项自动 SKIP）。
    #[allow(dead_code)]
    fn skip(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { name: name.into(), ok: false, skipped: true, detail: reason.into() }
    }
}

/// 运行全部自检，返回进程退出码。
/// 崩溃恢复探针（阶段 5，`--selfcheck-crash` 模式调用）：
/// 会话 A 打开画布并添加图形（脏文档）→ 自动保存 → drop 后删除 clean 标记
/// （模拟异常退出）→ `reopen` 构建会话 B（build 时应自动恢复）。
/// 把恢复的文档标题写入 `out`（JSON 数组），成功返回 0。
pub fn crash_probe(
    session_a: AppRuntime,
    mut reopen: impl FnMut() -> AppRuntime,
    out: &std::path::Path,
) -> i32 {
    let mut a = session_a;
    a.frame_tick(0.0);
    let _ = a.execute_command("example.open_canvas", &api::CommandArgs::default());
    a.frame_tick(0.0);
    let added = a.execute_command("example.add_shape", &api::CommandArgs::default());
    let saved = a.autosave_now().unwrap_or(0);
    let marker = a
        .layout_path()
        .parent()
        .map(|d| d.join("session.clean"))
        .expect("布局路径应有父目录");
    drop(a); // Drop 会写 clean 标记
    let _ = std::fs::remove_file(&marker); // 模拟异常退出

    let mut b = reopen();
    b.frame_tick(0.0);
    let recovered: Vec<String> = b
        .host
        .docs
        .docs()
        .filter(|d| d.title.starts_with("[恢复]"))
        .map(|d| d.title.clone())
        .collect();
    let _ = std::fs::write(
        out,
        serde_json::to_string(&recovered).unwrap_or_else(|_| "[]".to_string()),
    );
    let ok = added.is_ok() && saved > 0 && !recovered.is_empty();
    println!("[crash-probe] saved={saved} recovered={recovered:?}");
    if ok {
        0
    } else {
        1
    }
}

/// 崩溃恢复子进程自举检查（返回 (是否通过, 详情)）。
fn crash_probe_check() -> (bool, String) {
    use std::process::Command;
    let dir = std::env::temp_dir().join("workbench-rs-crash-test");
    let _ = std::fs::remove_dir_all(&dir);
    let mut ok = false;
    #[allow(unused_assignments)]
    let mut detail = String::new();
    if let Some(exe) = std::env::current_exe().ok() {
        match Command::new(exe)
            .arg("--selfcheck-crash")
            .env("WB_CRASH_DIR", &dir)
            .output()
        {
            Ok(o) if o.status.success() => {
                let result = std::fs::read_to_string(dir.join("result.json")).unwrap_or_default();
                let recovered: Vec<String> = serde_json::from_str(&result).unwrap_or_default();
                ok = recovered.iter().any(|t| t.contains("[恢复]"));
                detail = format!("recovered={recovered:?}");
            }
            Ok(o) => {
                detail = format!(
                    "子进程失败 status={:?} stdout={} stderr={}",
                    o.status,
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => detail = format!("无法启动子进程: {e}"),
        }
    } else {
        detail = "无法定位当前 exe".to_string();
    }
    (ok, detail)
}

/// 无头自检并退出进程（供产品 main 的 `--selfcheck` 模式）。
pub fn exit_with(app: &mut AppRuntime) -> ! {
    std::process::exit(run(app))
}

pub fn run(app: &mut AppRuntime) -> i32 {
    let mut checks: Vec<Check> = Vec::new();
    app.frame_tick(0.0);

    let has_cmd = |app: &AppRuntime, id: &str| app.command_title(id).is_some();
    let has_plugin = |app: &AppRuntime, id: &str| app.plugins_loaded().contains(&id.to_string());

    // 1. 装配：示例 Workbench 的命令已注册（探测式：未装配示例 Workbench 时跳过）
    if has_cmd(app, "example.add_shape") {
        checks.push(Check::new(
            "装配: 示例命令已注册",
            true,
            format!("example.add_shape = {:?}", app.command_title("example.add_shape")),
        ));
    } else {
        checks.push(Check::skip("装配: 示例命令已注册", "未装配示例 Workbench（wb-example）"));
    }

    // 2. 打开画布视图并检查上下文快照
    if has_cmd(app, "example.open_canvas") {
        let _ = app.execute_command("example.open_canvas", &api::CommandArgs::default());
        app.frame_tick(0.0);
        let has_doc = app.snapshot().active_document.is_some();
        let canvas_active = app.snapshot().active_view_is("example.canvas");
        checks.push(Check::new(
            "上下文: 默认文档 + 画布视图",
            has_doc && canvas_active,
            format!("doc={has_doc} view={canvas_active}"),
        ));
    } else {
        checks.push(Check::skip("上下文: 默认文档 + 画布视图", "未装配示例 Workbench"));
    }

    // 3. 同步命令 + 文档变更（以文档修订号增长为准，领域无关）
    if has_cmd(app, "example.add_shape") {
        let doc_id = app.snapshot().active_document.as_ref().map(|d| d.id);
        let rev_before = doc_id.and_then(|id| app.host.docs.revision(id));
        let mut added = true;
        for _ in 0..3 {
            let r = app.execute_command("example.add_shape", &api::CommandArgs::default());
            added &= r.is_ok();
        }
        let rev_after = doc_id.and_then(|id| app.host.docs.revision(id));
        let grew = matches!((rev_before, rev_after), (Some(b), Some(a)) if a > b);
        checks.push(Check::new(
            "命令: 同步命令驱动文档变更",
            added && grew,
            format!("added={added} revision {rev_before:?}→{rev_after:?}"),
        ));
    } else {
        checks.push(Check::skip("命令: 同步命令驱动文档变更", "未装配示例 Workbench"));
    }

    // 4. 撤销 / 重做
    if has_cmd(app, "example.add_shape") {
        app.execute_command("app.undo", &api::CommandArgs::default());
        let after_undo = shape_count(app);
        app.execute_command("app.redo", &api::CommandArgs::default());
        let after_redo = shape_count(app);
        checks.push(Check::new(
            "撤销/重做",
            after_undo == 2 && after_redo == 3,
            format!("undo={after_undo} redo={after_redo}"),
        ));
    } else {
        checks.push(Check::skip("撤销/重做", "未装配示例 Workbench"));
    }

    // 5. 失败命令传播
    if has_cmd(app, "example.failing_cmd") {
        let r = app.execute_command("example.failing_cmd", &api::CommandArgs::default());
        checks.push(Check::new(
            "错误: 失败命令返回 Failed 且不崩溃",
            matches!(r, api::CommandResult::Failed { .. }),
            format!("{r:?}"),
        ));
    } else {
        checks.push(Check::skip("错误: 失败命令返回 Failed 且不崩溃", "未装配示例 Workbench"));
    }

    // 6. 后台任务：进度 + 版本校验提交
    if has_cmd(app, "example.generate_shapes") {
        let before = shape_count(app);
        let _ = app.execute_command("example.generate_shapes", &api::CommandArgs::default());
        let done = pump_until_idle(app, Duration::from_secs(20));
        let after = shape_count(app);
        checks.push(Check::new(
            "后台任务: 完成 + 文档提交",
            done && after > before,
            format!("done={done} shapes {before}→{after}"),
        ));
    } else {
        checks.push(Check::skip("后台任务: 完成 + 文档提交", "未装配示例 Workbench"));
    }

    // 7. 后台任务：协作式取消
    if has_cmd(app, "example.slow_job") {
        let _ = app.execute_command("example.slow_job", &api::CommandArgs::default());
        std::thread::sleep(Duration::from_millis(200));
        let task_id = app
            .host
            .tasks
            .all()
            .find(|t| t.title.contains("慢任务"))
            .map(|t| t.id);
        if let Some(id) = task_id {
            app.host.tasks.request_cancel(id);
        }
        let settled = pump_until_idle(app, Duration::from_secs(10));
        let status = task_id.and_then(|id| app.host.tasks.get(id)).map(|t| t.status);
        checks.push(Check::new(
            "后台任务: 协作式取消 → Cancelled",
            settled && status == Some(api::TaskStatus::Cancelled),
            format!("status={status:?}"),
        ));
    } else {
        checks.push(Check::skip("后台任务: 协作式取消 → Cancelled", "未装配示例 Workbench"));
    }

    // 8. 保存 / 打开（前置：存在活动文档）
    let doc_id = app.snapshot().active_document.as_ref().map(|d| d.id);
    if doc_id.is_some() {
        let dir = std::env::temp_dir().join("workbench-rs-selfcheck");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "check.{}",
            doc_id
                .and_then(|id| app.host.docs.get(id))
                .and_then(|d| app.host.docs.type_of(&d.type_id).and_then(|t| t.extensions.first().cloned()))
                .unwrap_or_else(|| "dat".to_string())
        ));
        let saved = match doc_id {
            Some(id) => app.host.docs.save_as(id, &path),
            None => Err("无活动文档".to_string()),
        };
        let saved_ok = saved.is_ok() && path.exists();
        let opened = crate::platform_cmds::open_path_runtime(app, &path).is_ok();
        checks.push(Check::new(
            "文档: 保存 + 打开",
            saved_ok && opened,
            format!("saved={saved_ok} opened={opened}"),
        ));
    } else {
        checks.push(Check::skip("文档: 保存 + 打开", "没有活动文档"));
    }

    // 9. 布局持久化：含未知面板的布局可安全恢复（验收 #9）
    let mut layout = app.host.ws.save_layout();
    layout.panels.push(api::SavedPanel {
        id: "ghost.panel".to_string(),
        area: api::DockArea::Left,
        order: 99,
        size: 200.0,
        visible: true,
    });
    let skipped = app.host.ws.apply_layout(&layout);
    let tab_restored = !app.host.ws.tabs.is_empty();
    checks.push(Check::new(
        "布局: 未知面板跳过、标签恢复",
        skipped.iter().any(|m| m.contains("ghost.panel")) && tab_restored,
        format!("skipped={} tabs={}", skipped.len(), app.host.ws.tabs.len()),
    ));

    // 10.（阶段 2）异步命令：共享执行器完成 + 调用记录到终态
    if has_cmd(app, "example.async_scan") {
        let r = app.execute_command("example.async_scan", &api::CommandArgs::default());
        let submitted = r.is_ok();
        let done = pump_until_idle(app, Duration::from_secs(15));
        let async_record_ok = app
            .recent_invocations()
            .find(|rec| rec.command == "example.async_scan")
            .map(|rec| rec.status == api::InvocationStatus::Succeeded && rec.task_id.is_some())
            .unwrap_or(false);
        checks.push(Check::new(
            "阶段2: 异步命令（共享执行器）完成且调用记录到终态",
            submitted && done && async_record_ok,
            format!("submitted={submitted} done={done} record_ok={async_record_ok}"),
        ));
    } else {
        checks.push(Check::skip("阶段2: 异步命令（共享执行器）", "未装配示例 Workbench"));
    }

    // 11.（阶段 2）统一命令调用结果：同步立即终态；未注册命令可查询失败记录
    let any_cmd = app
        .snapshot()
        .active_view_type
        .as_ref()
        .map(|_| ())
        .and_then(|_| None::<String>)
        .or_else(|| None);
    let known_cmd = if has_cmd(app, "example.open_welcome") {
        Some("example.open_welcome".to_string())
    } else {
        None
    };
    if let Some(known) = known_cmd.or(any_cmd) {
        let inv = app.execute_command_tracked(&known, &api::CommandArgs::default());
        let sync_ok = inv.is_ok()
            && inv.record.status == api::InvocationStatus::Succeeded
            && inv.task_id.is_none();
        let inv_bad = app.execute_command_tracked("no.such.cmd", &api::CommandArgs::default());
        let unknown_ok = !inv_bad.is_ok()
            && inv_bad.record.status == api::InvocationStatus::Failed
            && inv_bad.record.execution.is_none();
        checks.push(Check::new(
            "阶段2: 统一命令调用结果（同步终态 + 未注册可查询）",
            sync_ok && unknown_ok,
            format!(
                "sync_ok={sync_ok} unknown_ok={unknown_ok} history={}",
                app.recent_invocations().count()
            ),
        ));
    } else {
        checks.push(Check::skip("阶段2: 统一命令调用结果", "产品没有已注册命令"));
    }

    // 12.（阶段 2）文档修订号查询 API（提交策略的数据基础；前置：活动文档）
    if app.snapshot().active_document.is_some() {
        let revision_ok = app
            .snapshot()
            .active_document
            .as_ref()
            .map(|d| app.host.docs.revision(d.id) == Some(d.revision))
            .unwrap_or(false);
        checks.push(Check::new(
            "阶段2: 文档修订号查询 API",
            revision_ok,
            format!("revision_ok={revision_ok}"),
        ));
    } else {
        checks.push(Check::skip("阶段2: 文档修订号查询 API", "没有活动文档"));
    }

    // 13.（阶段 3）Python 插件：发现、命令注册与 Ribbon 贡献
    //    （前置：插件面板标签在 egui_tiles 树中可见——探测 demo 插件命令存在）
    if has_plugin(app, "com.example.demo") {
        let plugin_cmd = app.command_title("demo.hello");
        let toolbar_contrib = app.host.ws.ribbon.iter().any(|t| {
            t.groups
                .iter()
                .any(|g| g.id.0 == "plugin.demo" && g.items.len() >= 2)
        });
        let plugin_ok = plugin_cmd.is_some() && toolbar_contrib;
        checks.push(Check::new(
            "阶段3: Python 插件发现/注册/工具栏贡献",
            plugin_ok,
            format!(
                "demo.hello={plugin_cmd:?} toolbar={toolbar_contrib} loaded={:?}",
                app.plugins_loaded()
            ),
        ));
    } else {
        checks.push(Check::skip("阶段3: Python 插件发现/注册/工具栏贡献", "未装载 demo 插件"));
    }

    // 14.（阶段 3）插件命令执行：同步返回备注 + 后台 Python 任务完成
    if has_cmd(app, "demo.hello") {
        let inv = app.execute_command_tracked(
            "demo.hello",
            &api::CommandArgs::new().with("name", "自检"),
        );
        let hello_ok = match &inv.result {
            api::CommandResult::Done { note } => note.as_deref().map(|n| n.contains("问候")).unwrap_or(false),
            _ => false,
        };
        let r = app.execute_command("demo.python_progress", &api::CommandArgs::default());
        let bg_done = pump_until_idle(app, Duration::from_secs(20));
        let py_task_ok = app.host.tasks.all().any(|t| {
            t.title.contains("Python") && t.status == api::TaskStatus::Succeeded
        });
        checks.push(Check::new(
            "阶段3: 插件命令执行（同步备注 + 后台进度任务）",
            hello_ok && r.is_ok() && bg_done && py_task_ok,
            format!("hello_ok={hello_ok} bg_done={bg_done} py_task_ok={py_task_ok}"),
        ));
    } else {
        checks.push(Check::skip("阶段3: 插件命令执行", "未装载 demo 插件"));
    }

    // 15.（Ribbon）Tab 模型（前置：wb.example.tools —— 示例产品的 Tab 组织）
    let ribbon_ok = if app.host.ws.ribbon.iter().any(|t| t.id.0 == "wb.example.tools") {
        let ws = &mut app.host.ws;
        let has_home = ws.ribbon.iter().any(|t| t.id.0 == "app.home");
        let has_tools = ws.ribbon.iter().any(|t| t.id.0 == "wb.example.tools");
        let home = ws.ribbon.iter().find(|t| t.id.0 == "app.home");
        let home_groups = home.map(|t| t.groups.len()).unwrap_or(0);
        // legacy（平台「窗口」组）并入默认 Tab
        let has_window_group = home
            .map(|t| t.groups.iter().any(|g| g.id.0 == "app.window"))
            .unwrap_or(false);
        // 活动 Tab 切换 + 布局持久化往返
        ws.set_active_ribbon_tab("wb.example.tools");
        let saved = ws.save_layout();
        let restored_active = saved.ribbon_active_tab.as_deref() == Some("wb.example.tools");
        let skipped = ws.apply_layout(&saved);
        let restored = ws.active_ribbon_tab == "wb.example.tools" && skipped.is_empty();
        ws.set_active_ribbon_tab("app.home");
        let ok = has_home
            && has_tools
            && home_groups >= 4
            && has_window_group
            && restored_active
            && restored;
        (
            ok,
            format!(
                "home={has_home} tools={has_tools} home_groups={home_groups} window={has_window_group} persist={restored_active}/{restored}"
            ),
        )
    } else {
        (false, "未装配示例 Ribbon Tab".to_string())
    };
    checks.push(Check::new(
        "Ribbon: Tab 合并/legacy 兼容/活动 Tab 持久化",
        ribbon_ok.0,
        ribbon_ok.1,
    ));

    // 16.（阶段 5）设置服务：写入 → 重新加载 → 读回
    let settings_ok = {
        let path = app
            .layout_path()
            .parent()
            .map(|d| d.join("settings.json"))
            .expect("布局路径应有父目录");
        let ok = app.host.settings.set("selfcheck.key", serde_json::json!(42)).is_ok();
        let (reloaded, err) = api::Settings::load(&path);
        ok && err.is_none() && reloaded.get_f64("selfcheck.key", 0.0) == 42.0
    };
    checks.push(Check::new(
        "阶段5: 设置服务持久化往返",
        settings_ok,
        format!("settings_ok={settings_ok}"),
    ));

    // 17.（阶段 5）自动保存 + 崩溃恢复：子进程自举——
    //     会话 A 写脏文档 → 自动保存 → 模拟崩溃（删 clean 标记）→ 会话 B 启动自动恢复
    //     （前置：产品支持 --selfcheck-crash 模式，当前由示例 Workbench 提供）
    if has_cmd(app, "example.add_shape") {
        let crash_ok = crash_probe_check();
        checks.push(Check::new(
            "阶段5: 自动保存 + 崩溃恢复",
            crash_ok.0,
            crash_ok.1,
        ));
    } else {
        checks.push(Check::skip("阶段5: 自动保存 + 崩溃恢复", "产品未实现 --selfcheck-crash 探针"));
    }

    // 18.（阶段 5）插件运行时禁用：命令移除 + Ribbon 清理 + 状态镜像
    // 注意：必须最后执行（禁用后 demo 插件命令不再可用）
    if has_plugin(app, "com.example.demo") {
        let disable_ok = {
            let before = app.command_title("demo.hello").is_some();
            let r = app.execute_command(
                "app.plugins.disable",
                &api::CommandArgs::new().with("id", "com.example.demo"),
            );
            let after_gone = app.command_title("demo.hello").is_none();
            let ribbon_clean = !app.host.ws.ribbon.iter().any(|t| {
                t.groups.iter().any(|g| g.id.0 == "plugin.demo")
            });
            let mirrored = app
                .host
                .plugins
                .iter()
                .find(|p| p.id == "com.example.demo")
                .map(|p| p.state == "Disabled")
                .unwrap_or(false);
            (
                before && r.is_ok() && after_gone && ribbon_clean && mirrored,
                format!(
                    "before={before} removed={:?} gone={after_gone} ribbon_clean={ribbon_clean} mirrored={mirrored}",
                    r.is_ok()
                ),
            )
        };
        checks.push(Check::new(
            "阶段5: 插件运行时禁用",
            disable_ok.0,
            disable_ok.1,
        ));
    } else {
        checks.push(Check::skip("阶段5: 插件运行时禁用", "未装载 demo 插件"));
    }

    // 汇总
    let failed = checks.iter().filter(|c| !c.ok && !c.skipped).count();
    let skipped = checks.iter().filter(|c| c.skipped).count();
    for c in &checks {
        let tag = if c.skipped {
            "SKIP"
        } else if c.ok {
            "PASS"
        } else {
            "FAIL"
        };
        println!("[{}] {} — {}", tag, c.name, c.detail);
    }
    let run_total = checks.len() - skipped;
    println!(
        "selfcheck: {}/{} 项通过（{} 项跳过）",
        run_total - failed,
        run_total,
        skipped
    );
    if failed == 0 {
        0
    } else {
        1
    }
}

/// 通过文档类型的序列化接口读取 shapes 数量（平台不感知领域类型）。
fn shape_count(app: &AppRuntime) -> usize {
    let Some(summary) = app.snapshot().active_document.as_ref() else {
        return 0;
    };
    let doc_id = summary.id;
    let Some(doc) = app.host.docs.get(doc_id) else {
        return 0;
    };
    let Some(def) = app.host.docs.type_of(&doc.type_id) else {
        return 0;
    };
    let Ok(bytes) = (def.serialize)(doc.content.as_ref()) else {
        return 0;
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("shapes").and_then(|s| s.as_array()).map(|a| a.len()))
        .unwrap_or(0)
}

/// 反复 frame_tick 直到没有运行中的任务（或超时）。
fn pump_until_idle(app: &mut AppRuntime, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        app.frame_tick(0.016);
        let running = app.host.tasks.all().any(|t| !t.status.is_terminal());
        if !running {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[allow(dead_code)]
fn _path_use(_: &Path) {}
