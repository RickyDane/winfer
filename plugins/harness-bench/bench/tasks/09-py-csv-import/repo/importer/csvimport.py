"""Typed CSV import. See README.md for the spec."""


class CsvImportError(Exception):
    def __init__(self, message, line):
        super().__init__(f"line {line}: {message}")
        self.line = line


def parse(text, schema):
    raise NotImplementedError


def load(path, schema):
    raise NotImplementedError
