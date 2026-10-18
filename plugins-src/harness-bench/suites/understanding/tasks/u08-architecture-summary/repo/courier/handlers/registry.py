HANDLERS = {}


def on(event):
    def deco(fn):
        HANDLERS[event] = fn
        return fn

    return deco


def dispatch(msg):
    fn = HANDLERS.get(msg.get("type"))
    return fn(msg) if fn else None
