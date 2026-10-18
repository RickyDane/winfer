from app.actions.registry import action
from app.export import formats
from app.legacy import exporter as legacy_exporter


@action("doc.export")
def export_document(doc):
    """Old entry point, shadowed by the alias in the registry."""
    return legacy_exporter.export(doc)


@action("export.default")
def export_default(doc):
    fmt = doc.settings.get("format", "pdf")
    return formats.exporter_for(fmt)(doc)


@action("doc.print")
def print_document(doc):
    return formats.exporter_for("ps")(doc)
