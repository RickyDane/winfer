from app.export import pdf, text


def exporter_for(fmt):
    return {"pdf": pdf.export_pdf, "txt": text.export_text, "ps": pdf.export_postscript}[fmt]
