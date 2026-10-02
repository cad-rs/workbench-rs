"""workbench-rs 演示插件（design.md §11.3/§11.4）。

注册一个同步命令、一个后台进度任务命令，并向工具栏贡献条目。
"""
import time


def register(api):
    api.log_info("演示插件已加载（Host API v%s）" % api.api_version)

    # 同步命令：在 UI 线程执行，可返回字符串备注（进入调用历史与日志）
    api.register_command(id="demo.hello", title="插件问候", callback=hello)

    # 后台命令：callback(task, args) 在后台任务线程执行，
    # task 提供 report / set_stage / log / check_cancelled（协作式取消）
    api.register_command(
        id="demo.python_progress",
        title="插件:Python 进度任务",
        callback=progress_job,
        background=True,
    )

    # 工具栏贡献：组不存在时自动创建（受控贡献，design.md §7.3）
    api.add_toolbar_item(group="plugin.demo", command="demo.hello", label="你好(插件)")
    api.add_toolbar_item(
        group="plugin.demo", command="demo.python_progress", label="Python进度"
    )


def hello(args):
    name = (args or {}).get("name") or "workbench"
    return "来自 Python 插件的问候，%s！" % name


def progress_job(task, args):
    steps = 20
    for i in range(steps):
        task.check_cancelled()  # 已取消时抛 PluginCancelled，任务进入 Cancelled 终态
        task.report(i / steps, "Python 步骤 %d/%d" % (i + 1, steps))
        time.sleep(0.1)
    task.log("Python 后台任务完成")
    return None
