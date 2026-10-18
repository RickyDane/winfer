from courier.handlers.registry import on


@on("*")
def audit_all(msg):
    return None
