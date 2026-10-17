from app.actions.registry import dispatch

BUTTONS = [
    ("New", "file.new"),
    ("Open", "file.open"),
    ("Export", "doc.export"),
    ("Print", "doc.print"),
]


def on_click(label: str, doc) -> None:
    for name, action in BUTTONS:
        if name == label:
            dispatch(action, doc)
            return
    raise KeyError(label)
