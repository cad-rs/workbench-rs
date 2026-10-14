// Rhai demo plugin entry: registers commands, toolbar items, and one script
// UI panel (ui/main.rhai) through the host-injected global functions.

fn register() {
    // 同步命令：UI 线程执行，返回字符串作为备注（返回 () 表示无备注）
    register_command("rhai.hello", "Rhai 问候", "hello");
    add_toolbar_item("plugin.rhai", "rhai.hello", "问候");

    // 后台命令：任务线程执行，`task` 对象提供进度/日志/协作式取消
    register_background_command("rhai.progress", "Rhai 后台任务", "progress_job");
    add_toolbar_item("plugin.rhai", "rhai.progress", "后台任务");

    // 脚本 UI 面板：内容完全由 ui/main.rhai 的 UiNode 树定义
    register_panel("rhai.panel", "Rhai 演示", "ui/main.rhai", "right");

    log_info("Rhai demo 插件已注册（命令 ×2、工具栏组、脚本面板 ×1）");
}

// 同步回调：args 为命令参数表（字符串键值），返回备注字符串
fn hello(args) {
    let name = args.name;
    if name == "" {
        "来自 Rhai 脚本的问候"
    } else {
        `来自 Rhai 脚本的问候（{name}）`
    }
}

// 后台回调：task.report/set_stage/log 驱动状态栏；task.check_cancelled()
// 在已取消时抛出宿主识别的取消哨兵（任务随即以 Cancelled 终态收尾）
fn progress_job(task, args) {
    let steps = 20;
    for i in 0..steps {
        task.check_cancelled();
        task.report(i.to_float() / steps.to_float(), `处理批次 {i + 1}/{steps}`);
        if i == 10 {
            task.log("已完成一半（Rhai 后台任务）");
        }
    }
    task.set_stage("收尾");
    task.report_progress(1.0);
}
