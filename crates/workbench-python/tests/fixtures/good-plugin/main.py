def register(api):
    api.log_info("good plugin loaded")
    api.register_command(id="good.ping", title="Ping", callback=ping)
    api.register_command(id="good.long", title="Long", callback=long_job, background=True)
    api.add_toolbar_item(group="good.tools", command="good.ping", label="Ping!")
    api.add_toolbar_item(group="good.tools", command="good.long")


def ping(args):
    name = (args or {}).get("name") or "default"
    return "pong:" + name


def long_job(task, args):
    for i in range(30):
        task.check_cancelled()
        task.report(i / 30.0, "step %d" % i)
        import time
        time.sleep(0.02)
