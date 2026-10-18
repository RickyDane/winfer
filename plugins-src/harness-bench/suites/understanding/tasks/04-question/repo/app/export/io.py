import os
import tempfile


def atomic_write(path, data: bytes):
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or ".")
    with os.fdopen(fd, "wb") as f:
        f.write(data)
    os.replace(tmp, path)
    return path


def write_plain(path, data: bytes):
    with open(path, "wb") as f:
        f.write(data)
