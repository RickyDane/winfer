"""Generates a Node project with an installed node_modules tree for a license audit into argv[1] (deterministic)."""

import json
import os
import random
import sys

root = sys.argv[1]
rng = random.Random(505)


def write(path, data):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(data if isinstance(data, str) else json.dumps(data, indent=2) + "\n")


TEXT = {
    "MIT": "MIT License\n\nCopyright (c) {year} {author}\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, subject to the following conditions: ...\n",
    "ISC": "ISC License\n\nCopyright (c) {year} {author}\n\nPermission to use, copy, modify, and/or distribute this software for any\npurpose with or without fee is hereby granted ...\n",
    "BSD-3-Clause": "BSD 3-Clause License\n\nCopyright (c) {year}, {author}\n\nRedistribution and use in source and binary forms, with or without\nmodification, are permitted provided that ...\n",
    "Apache-2.0": "                                 Apache License\n                           Version 2.0, January 2004\n                        http://www.apache.org/licenses/\n\n   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION ...\n",
    "LGPL-3.0": "                   GNU LESSER GENERAL PUBLIC LICENSE\n                       Version 3, 29 June 2007\n\n Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>\n\n  This version of the GNU Lesser General Public License incorporates\nthe terms and conditions of version 3 of the GNU General Public\nLicense, supplemented by the additional permissions listed below. ...\n",
    "AGPL-3.0": "                    GNU AFFERO GENERAL PUBLIC LICENSE\n                       Version 3, 19 November 2007\n\n Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>\n\n  The GNU Affero General Public License is a free, copyleft license for\nsoftware and other kinds of works, specifically designed to ensure\ncooperation with the community in the case of network server software. ...\n",
}
AUTHORS = ["Nadia Ross", "Kenji Arai", "Pavel Novak", "Lucia Ferreira", "Owen Blake", "Sara Lindqvist"]

# name -> (version, package.json license value, license file (name, kind) or None, deps, extra pkg fields)
P = {}


def pkg(path, name, version, license=None, file=None, deps=None, dev=None, extra=None):
    P[path] = dict(name=name, version=version, license=license, file=file, deps=deps or {}, dev=dev or {}, extra=extra or {})


nm = "node_modules/"
# production tree
pkg(nm + "express-lite", "express-lite", "4.1.0", "MIT", ("LICENSE", "MIT"), {"body-parse": "^1.3.0", "path-match": "^2.0.0", "mime-lite": "^1.0.0"}, dev={"gpl-mock": "^1.0.0", "test-runner": "^8.0.0"})
pkg(nm + "body-parse", "body-parse", "1.3.0", "MIT", None, {"qs-tiny": "^6.0.0", "raw-bytes": "^2.1.0"})
pkg(nm + "qs-tiny", "qs-tiny", "6.0.2", None, None, {}, extra={"licenses": [{"type": "MPL-2.0", "url": "https://mozilla.org/MPL/2.0/"}]})
pkg(nm + "raw-bytes", "raw-bytes", "2.1.4", "ISC", ("LICENSE", "ISC"))
pkg(nm + "path-match", "path-match", "2.0.1", "MIT")
pkg(nm + "mime-lite", "mime-lite", "1.0.3", "MIT", None, {"mime-db-lite": "^1.40.0"})
pkg(nm + "mime-db-lite", "mime-db-lite", "1.40.0", "MIT", ("LICENSE", "MIT"))
pkg(nm + "chartkit", "chartkit", "3.0.0", "Apache-2.0", ("LICENSE", "Apache-2.0"), {"svg-path": "^2.2.0", "pdf-mini": "^0.4.0", "fast-diff-ng": "^1.1.0"})
pkg(nm + "svg-path", "svg-path", "2.2.0", "MIT", None, {"color-space": "^2.0.0"})
pkg(nm + "color-space", "color-space", "2.0.0", "MIT", ("LICENSE.md", "LGPL-3.0"))
pkg(nm + "pdf-mini", "pdf-mini", "0.4.0", "(MIT AND GPL-2.0-or-later)", None)
pkg(nm + "fast-diff-ng", "fast-diff-ng", "1.1.0", "(MIT OR GPL-3.0-only)", None)
pkg(nm + "md-render", "md-render", "5.0.1", "MIT", ("LICENSE", "MIT"), {"html-sanitize": "^3.0.0", "tiny-glob": "^1.2.0", "entities-lite": "^4.0.0"})
pkg(nm + "html-sanitize", "html-sanitize", "3.0.2", "BSD-3-Clause", ("LICENSE", "BSD-3-Clause"), {"xml-sax-lite": "^0.7.0", "entities-lite": "^4.0.0"})
pkg(nm + "xml-sax-lite", "xml-sax-lite", "0.7.3", "SEE LICENSE IN COPYING", ("COPYING", "AGPL-3.0"))
pkg(nm + "entities-lite", "entities-lite", "4.2.0", "BSD-3-Clause")
pkg(nm + "tiny-glob", "tiny-glob", "1.2.0", "MIT", ("LICENSE", "MIT"))
pkg(nm + "date-tiny", "date-tiny", "2.0.0", "MIT", None, {"uuid-ng": "^9.0.0"})
pkg(nm + "uuid-ng", "uuid-ng", "9.0.0", "GPL-3.0", ("LICENSE", "MIT"))
pkg(nm + "auth-guard", "auth-guard", "1.5.0", "ISC", ("LICENSE", "ISC"), {"saml-parse": "^2.0.0", "jwt-lite": "^3.1.0"})
pkg(nm + "saml-parse", "saml-parse", "2.0.4", "MIT", None, {"xml-sax-lite": "^0.7.1"})
pkg(nm + "jwt-lite", "jwt-lite", "3.1.0", "MIT", None, {"base64-tiny": "^1.0.0"})
pkg(nm + "base64-tiny", "base64-tiny", "1.0.2", "MIT")
pkg(nm + "csv-stream", "csv-stream", "0.8.0", "MIT", ("LICENSE", "MIT"), {"tiny-glob": "^0.9.0", "stream-utils": "^1.0.0"})
pkg(nm + "csv-stream/node_modules/tiny-glob", "tiny-glob", "0.9.1", "GPL-2.0-only", None)
pkg(nm + "stream-utils", "stream-utils", "1.0.7", "MIT", None)
# dev-only tree
pkg(nm + "test-runner", "test-runner", "8.2.0", "MIT", ("LICENSE", "MIT"), {"assert-plus": "^1.0.0", "fast-diff-ng": "^1.0.0"})
pkg(nm + "assert-plus", "assert-plus", "1.0.0", "MIT")
pkg(nm + "lint-pack", "lint-pack", "2.3.1", "MIT", None, {"leftpad-gpl": "^1.0.0"})
pkg(nm + "leftpad-gpl", "leftpad-gpl", "1.0.4", "GPL-3.0-only", None)
pkg(nm + "bundle-x", "bundle-x", "0.19.0", "MIT", ("LICENSE", "MIT"), {"xml-sax-lite": "^0.7.0", "qs-tiny": "^6.0.0", "gpl-mock": "^1.0.0"})
# extraneous (left over in node_modules, nothing in production needs it)
pkg(nm + "gpl-mock", "gpl-mock", "1.0.0", "GPL-3.0-or-later", None)

# noise leaves hung under some packages
parents = [nm + x for x in ["path-match", "raw-bytes", "stream-utils", "base64-tiny", "entities-lite", "mime-db-lite", "assert-plus", "jwt-lite"]]
words = ["is", "has", "to", "fmt", "safe", "fast", "deep", "tiny", "micro", "quick"]
nouns = ["number", "object", "string", "array", "equal", "merge", "clone", "escape", "trim", "flat", "once", "debounce"]
names = set()
while len(names) < 22:
    names.add(f"{rng.choice(words)}-{rng.choice(nouns)}")
for n in sorted(names):
    lic = rng.choice(["MIT", "MIT", "ISC", "BSD-3-Clause", "Apache-2.0", "0BSD", "BSD-2-Clause"])
    has_file = lic in TEXT and rng.random() < 0.5
    pkg(nm + n, n, f"{rng.randint(0, 5)}.{rng.randint(0, 20)}.{rng.randint(0, 9)}", lic, ("LICENSE", lic) if has_file else None)
    par = rng.choice(parents)
    P[par]["deps"][n] = "^" + P[nm + n]["version"].split(".")[0] + ".0.0"

for path, p in P.items():
    pj = {"name": p["name"], "version": p["version"], "description": f"{p['name']} package", "main": "index.js"}
    if p["license"] is not None:
        pj["license"] = p["license"]
    pj.update(p["extra"])
    if p["deps"]:
        pj["dependencies"] = dict(sorted(p["deps"].items()))
    if p["dev"]:
        pj["devDependencies"] = p["dev"]
    write(path + "/package.json", pj)
    write(path + "/index.js", f"module.exports = require('./lib/{p['name']}');\n")
    if p["file"]:
        fname, kind = p["file"]
        write(f"{path}/{fname}", TEXT[kind].format(year=rng.randint(2014, 2024), author=rng.choice(AUTHORS)))

write("package.json", {
    "name": "harbor-dashboard", "version": "1.0.0", "private": True, "license": "UNLICENSED",
    "dependencies": {"express-lite": "^4.1.0", "chartkit": "^3.0.0", "md-render": "^5.0.0", "date-tiny": "^2.0.0", "auth-guard": "^1.5.0", "csv-stream": "^0.8.0"},
    "devDependencies": {"test-runner": "^8.2.0", "lint-pack": "^2.3.0", "bundle-x": "^0.19.0"},
})
write("src/index.js", "const express = require('express-lite');\nconst app = express();\napp.listen(3000);\n")
write("docs/licensing.md", """# Licensing notes (last reviewed 2024-02)

We ship harbor-dashboard to customers, so copyleft dependencies need legal review.
Last audit found: `uuid-ng` (GPL-3.0, via date-tiny) and `fast-diff-ng` (GPL). Everything else is permissive.
""")
