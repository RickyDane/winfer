import collections
import re

LINE = re.compile(r"^\S+ ERROR \[\w+\] code=(E\d+) ")
counts = collections.Counter()
with open("logs/app.log") as f:
    for line in f:
        m = LINE.match(line)
        if m:
            counts[m.group(1)] += 1
for code, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[:3]:
    print(code, n)
