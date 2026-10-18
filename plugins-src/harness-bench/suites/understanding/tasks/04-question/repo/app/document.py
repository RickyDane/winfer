from dataclasses import dataclass, field


@dataclass
class Document:
    path: str
    body: str
    settings: dict = field(default_factory=dict)  # "format" defaults to pdf

    def path_with(self, ext):
        base, _ = __import__("os").path.splitext(self.path)
        return base + ext
