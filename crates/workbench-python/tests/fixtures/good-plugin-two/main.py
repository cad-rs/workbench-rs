def register(api):
    api.register_command(id="two.hello", title="Two", callback=lambda args: "hi")
