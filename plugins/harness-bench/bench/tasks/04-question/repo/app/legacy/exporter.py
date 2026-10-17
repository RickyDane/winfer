from app.export.io import write_plain


def export(doc):
    """Pre-2.0 exporter. Still registered but no longer reachable from the toolbar."""
    return write_plain(doc.path_with(".pdf"), doc.body.encode())
