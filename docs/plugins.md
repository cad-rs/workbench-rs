# 插件开发指南（Host API v1）

面向 Python 插件开发者。运行前提见 README「Python 插件」一节。

## 插件结构

```text
my-plugin/
  plugin.toml     # 清单（必须）
  plugin.py       # 入口脚本（清单 entry_point 指向）
  ...             # 其他资源随插件目录一起分发
```

## 清单 plugin.toml

```toml
[plugin]
id = "com.example.my-plugin"   # 稳定且唯一（字母/数字/./-/_）
name = "My Plugin"
version = "0.1.0"
api_version = "1"              # 必须等于宿主 HOST_API_VERSION（当前 "1"），否则拒绝加载
entry_point = "plugin.py:register"

[compatibility]
products = ["*"]               # 可选：限定产品 ID

[permissions]                  # 可选：审计与提示用途（进程内 Python 不构成安全沙箱）
document_read = false
document_write = false
filesystem_read = false
network = false
```

## Host API

插件被发现后会调用 `register(api)`（UI 线程、GIL 内）。API 对象：

| 方法 | 说明 |
|---|---|
| `api.register_command(id, title, callback, background=False)` | 注册命令。重复 id 报错 |
| `api.add_toolbar_item(group, command, label=None)` | 向 Ribbon 工具栏贡献条目；组不存在则创建（组内同 id 命令自动合并） |
| `api.log_info(msg)` | 装配期日志（出现在平台日志） |
| `api.api_version` | 宿主 API 版本 |

### 同步命令（UI 线程）

`callback(args)`，`args` 是 `dict`（命令参数键值）。可返回 `str` 作为执行备注（进入调用历史与状态栏日志），返回 `None` 即完成。执行异常会转为命令失败（`CommandResult::Failed`），平台捕获并记录——不会拖垮宿主。

```python
def hello(args):
    name = (args or {}).get("name") or "world"
    return "hello, %s!" % name

api.register_command(id="my.hello", title="问好", callback=hello)
```

### 后台命令（任务线程，可取消、可报进度）

`callback(task, args)` 在后台任务线程执行（GIL 自动获取）。`task` 对象：

| 方法 | 说明 |
|---|---|
| `task.report(fraction=None, stage=None)` | 进度（0.0~1.0）与阶段文本 |
| `task.set_stage(stage)` | 只更新阶段文本 |
| `task.log(msg)` | 任务日志（进入平台日志） |
| `task.check_cancelled()` | 已取消时抛出 `PluginCancelled`——循环里调用它即可协作式退出 |
| `task.is_cancelled()` | 非抛出式查询 |

```python
import time

def progress_job(task, args):
    for i in range(20):
        task.check_cancelled()          # 用户取消 → 任务终态 Cancelled
        task.report(i / 20, "步骤 %d" % (i + 1))
        time.sleep(0.1)

api.register_command(id="my.progress", title="长任务", callback=progress_job, background=True)
```

### 工具栏贡献

```python
api.add_toolbar_item(group="my.tools", command="my.hello", label="问好")   # 创建组 my.tools
api.add_toolbar_item(group="my.tools", command="my.progress")             # 组已存在则追加
```

组名建议带插件前缀（如 `my.tools`）。也可以向既有组贡献（跨模块受控合并）。

## 生命周期与边界

- 清单非法 / `api_version` 不兼容 / `register()` 抛异常 → 插件被**拒绝加载**（原因可在平台日志与「插件」面板查看），其余插件与宿主不受影响；
- 运行时可在「插件」面板点击「禁用」：移除该插件注册的全部命令与工具栏贡献（会话内生效）；
- 同 id 插件后发现的被跳过；
- 文档读写在 Host API v1 尚未开放（计划 v1.1：`api.document` 只读快照 + 经命令的受控写入）；
- 需要长耗时计算务必用 `background=True`；同步命令在 UI 线程执行，超过 ~16ms 会拖慢界面。
