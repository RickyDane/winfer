def export_text(doc):
    with open(doc.path_with(".txt"), "w") as f:
        f.write(doc.body)
