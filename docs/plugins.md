# Plugin Authoring Guide (Host API v1, Rhai)

For Rhai script plugin developers. Runtime behavior (discovery, isolation,
runtime disable) is identical to what the platform documents elsewhere; this
guide covers the Rhai specifics. A runnable example lives in
`plugins/rhai-demo/`.

## Plugin layout

```text
my-plugin/
  plugin.toml        # manifest (required)
  plugin.rhai        # entry script (pointed to by the manifest entry_point)
  ui/                # optional: script panel (gpui-rhai view)
    main.rhai        #   entry: view(ctx) returns a UiNode tree
    components/      #   vendored registry component sources
    theme.rhai       #   project theme
    styles.rhai      #   component skin (optional)
    locales/         #   locale tables (optional)
  ...                # other resources ship with the plugin directory
```

## Manifest: plugin.toml

```toml
[plugin]
id = "com.example.my-plugin"   # stable and unique (letters/digits/./-/_)
name = "My Plugin"
version = "0.1.0"
api_version = "1"              # must equal the host HOST_API_VERSION ("1"), otherwise refused
entry_point = "plugin.rhai"    # Rhai script; "plugin.rhai:setup" names the entry fn (default: register)

[compatibility]
products = ["*"]               # optional: restrict to product IDs

[permissions]                  # optional: audit/prompt purposes only
                               # (in-process scripting is not a security sandbox)
document_read = false
document_write = false
filesystem_read = false
network = false
```

## Host API

After discovery the host compiles the script and calls `register()` (assembly
time). The following global functions are injected by the host:

| Global | Description |
|---|---|
| `register_command(id, title, callback_fn)` | register a sync command (UI thread) |
| `register_background_command(id, title, callback_fn)` | register a background command (task thread, progress + cancellation) |
| `add_toolbar_item(group, command, label)` | contribute a Ribbon toolbar item; the group is auto-created (same-id commands merge into the group) |
| `add_toolbar_item_no_label(group, command)` | same without a label |
| `register_panel(id, title, script_path, area)` | register a script UI panel (`area`: `"left"`/`"right"`/`"bottom"`/`"top"`; `script_path` relative to the plugin dir) |
| `log_info(msg)` | assembly-time log (shows up in the platform log) |

### Sync commands (UI thread)

`fn callback(args)` where `args` is a Rhai map of the command arguments.
Return a `string` used as the execution note (lands in the invocation history
and status log), or nothing to simply finish. Runtime errors become command
failures (`CommandResult::Failed`) — the platform captures and logs them; the
host never crashes.

```rhai
fn hello(args) {
    let name = if args.name == "" { "world" } else { args.name };
    "hello, {name}!"
}

fn register() {
    register_command("my.hello", "Greet", "hello");
}
```

### Background commands (task thread, cancellable, reportable)

`fn callback(task, args)` runs on a background task thread. The `task` object:

| Method | Description |
|---|---|
| `task.report(fraction, stage)` | progress (0.0~1.0) and stage text |
| `task.report_progress(fraction)` | progress only |
| `task.set_stage(stage)` | stage text only |
| `task.log(msg)` | task log (into the platform log) |
| `task.check_cancelled()` | throws once cancelled — call it in loops for cooperative exit |
| `task.is_cancelled()` | non-throwing query |

```rhai
fn progress_job(task, args) {
    for i in 0..20 {
        task.check_cancelled()          // user cancel -> task ends Cancelled
        task.report(i as float / 20.0, `step {i + 1}`);
    }
}

fn register() {
    register_background_command("my.progress", "Long Task", "progress_job");
}
```

### Toolbar contributions

```rhai
fn register() {
    add_toolbar_item("my.tools", "my.hello", "Greet");        // creates group my.tools
    add_toolbar_item_no_label("my.tools", "my.progress");     // existing group: appends
}
```

Prefix group names with your plugin id (e.g. `my.tools`). Contributing into
existing groups (controlled cross-module merge) is also supported.

## Script UI panels (gpui-rhai)

`register_panel` mounts a panel whose content is a gpui-rhai view: the script
(`ui/main.rhai`) returns a declarative UiNode tree built from registry
components; state lives in the script (`ctx.get_state`/`ctx.set_state`) and
events are named functions.

```rhai
// ui/main.rhai
import "components/button" as button;
import "components/input" as input;
import "components/label" as label;

fn state_schema() {
    #{ fields: #{ count: #{ schema: #{ type: "integer" },
        "default": #{ type: "integer", value: 0 } } } }
}

fn clicked(ctx, payload) { ctx.set_state("count", ctx.get_state("count") + 1); }

fn view(ctx) {
    column([
        label::Label(#{ text: `点击次数：${ctx.get_state("count")}` }),
        button::Button(#{ text: "加一", variant: "primary", on_click: Fn("clicked") }),
    ])
}
```

Register it from the entry script: `register_panel("my.panel", "My Panel", "ui/main.rhai", "right")`.
Component sources (button/input/label/tag/progress/card/…) come from the
gpui-rhai registry and are vendored into the plugin's `ui/components/`
directory — see `plugins/rhai-demo/` for a complete custom-UI panel.

## Lifecycle and boundaries

- invalid manifest / incompatible `api_version` / `register()` runtime error →
  the plugin is **refused** (reason visible in the platform log and the
  Plugins panel); other plugins and the host are unaffected;
- the Plugins panel offers runtime **Disable**: removes every command and
  toolbar contribution the plugin registered (session-scoped);
- a duplicate plugin id is skipped for later discoveries;
- document read/write is not exposed in Host API v1 (planned for v1.1:
  `api.document` read-only snapshot + controlled writes via commands);
- long computation must use background commands; sync commands run on the UI
  thread — anything beyond ~16ms will lag the interface;
- script UI panels are rendered by the gpui frontend; on wasm32 builds the
  plugin stage is not mounted (plugin directories are a desktop convenience).
