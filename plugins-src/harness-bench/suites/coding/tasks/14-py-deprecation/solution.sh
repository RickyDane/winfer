python3 - <<'PY'
import pathlib
import re

CALL = re.compile(r"\b((?:self\.)?(?:client|_http)|api)\.fetch\(")
for path in pathlib.Path("app").rglob("*.py"):
    s = path.read_text()
    out, pos = [], 0
    for m in CALL.finditer(s):
        if m.start() < pos:
            continue
        depth, i, args, start = 1, m.end(), [], m.end()
        while depth:
            ch = s[i]
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
            elif ch == "," and depth == 1:
                args.append(s[start:i]); start = i + 1
            i += 1
        last = s[start:i - 1]
        if last.strip():
            args.append(last)
        url, timeout = (a.strip() for a in args)
        if not timeout.startswith("timeout="):
            timeout = "timeout=" + timeout
        out.append(s[pos:m.start()] + f'{m.group(1)}.request("GET", {url}, {timeout})')
        pos = i
    if out:
        path.write_text("".join(out) + s[pos:])
PY
