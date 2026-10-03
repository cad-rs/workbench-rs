# Plugin Authoring Guide (Host API v1)

For Python plugin developers. Runtime prerequisites: see "Python plugins" in
the README.

## Plugin layout

```text
my-plugin/
  plugin.toml     # manifest (required)
  plugin.py       # entry script (pointed to by the manifest entry_point)
  ...             # other resources ship with the plugin directory
```

## Manifest: plugin.toml

```toml
[plugin]
id = "com.example.my-plugin"   # stable and unique (letters/digits/./-/_)
name = "My Plugin"
version = "0.1.0"
api_version = "1"              # must equal the host HOST_API_VERSION ("1"), otherwise refused
entry_point = "plugin.py:register"

[compatibility]
products = ["*"]               # optional: restrict to product IDs

[permissions]                  # optional: audit/prompt purposes only
                               # (in-process Python is not a security sandbox)
document_read = false
document_write = false
filesystem_read = false
network = false
```

## Host API

After discovery the host calls `register(api)` (UI thread, inside the GIL).
The `api` object:

| Method | Description |
|---|---|
| `api.register_command(id, title, callback, background=False)` | register a command; duplicate ids raise |
| `api.add_toolbar_item(group, command, label=None)` | contribute a Ribbon toolbar item; the group is auto-created (same-id commands merge into the group) |
| `api.log_info(msg)` | assembly-time log (shows up in the platform log) |
| `api.api_version` | host API version |

### Sync commands (UI thread)

`callback(args)` where `args` is a `dict` (command arguments). May return a
`str` used as the execution note (lands in the invocation history and status
log), or `None` to simply finish. Exceptions become command failures
(`CommandResult::Failed`) — the platform captures and logs them; the host never
crashes.

```python
def hello(args):
    name = (args or {}).get("name") or "world"
    return "hello, %s!" % name

api.register_command(id="my.hello", title="Greet", callback=hello)
```

### Background commands (task thread, cancellable, reportable)

`callback(task, args)` runs on a background task thread (GIL acquired
automatically). The `task` object:

| Method | Description |
|---|---|
| `task.report(fraction=None, stage=None)` | progress (0.0~1.0) and stage text |
| `task.set_stage(stage)` | update the stage text only |
| `task.log(msg)` | task log (into the platform log) |
| `task.check_cancelled()` | raises `PluginCancelled` once cancelled — call it in loops for cooperative exit |
| `task.is_cancelled()` | non-raising query |

```python
import time

def progress_job(task, args):
    for i in range(20):
        task.check_cancelled()          # user cancel -> task ends Cancelled
        task.report(i / 20, "step %d" % (i + 1))
        time.sleep(0.1)

api.register_command(id="my.progress", title="Long Task", callback=progress_job, background=True)
```

### Toolbar contributions

```python
api.add_toolbar_item(group="my.tools", command="my.hello", label="Greet")  # creates group my.tools
api.add_toolbar_item(group="my.tools", command="my.progress")             # existing group: appends
```

Prefix group names with your plugin id (e.g. `my.tools`). Contributing into
existing groups (controlled cross-module merge) is also supported.

## Lifecycle and boundaries

- invalid manifest / incompatible `api_version` / `register()` exception → the
  plugin is **refused** (reason visible in the platform log and the Plugins
  panel); other plugins and the host are unaffected;
- the Plugins panel offers runtime **Disable**: removes every command and
  toolbar contribution the plugin registered (session-scoped);
- a duplicate plugin id is skipped for later discoveries;
- document read/write is not exposed in Host API v1 (planned for v1.1:
  `api.document` read-only snapshot + controlled writes via commands);
- long computation must use `background=True`; sync commands run on the UI
  thread — anything beyond ~16ms will lag the interface.
