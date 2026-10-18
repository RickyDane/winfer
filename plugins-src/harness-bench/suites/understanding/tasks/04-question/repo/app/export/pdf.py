from app.export.io import atomic_write


def _render(doc):
    return b"%PDF-1.7\n" + doc.body.encode()


def export_pdf(doc):
    data = _render(doc)
    return _save(doc.path_with(".pdf"), data)


def export_postscript(doc):
    return _save(doc.path_with(".ps"), b"%!PS\n" + doc.body.encode())


def _save(path, data):
    # Retries are handled by atomic_write.
    return atomic_write(path, data)
