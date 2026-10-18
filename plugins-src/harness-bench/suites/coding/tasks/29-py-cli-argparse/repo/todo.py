#!/usr/bin/env python3
"""A tiny todo list stored in a JSON file.

usage: todo.py [--file PATH] add TEXT... [--priority N]
       todo.py [--file PATH] list [--all] [--priority N]
       todo.py [--file PATH] done ID
       todo.py [--file PATH] rm ID
"""

import json
import os
import sys

VERSION = "1.4.0"
USAGE = __doc__.split("\n\n", 1)[1]


def load(path):
    if not os.path.exists(path):
        return {"next": 1, "items": []}
    with open(path) as f:
        return json.load(f)


def save(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(data, f, indent=2)
    os.replace(tmp, path)


def find(data, item_id):
    for item in data["items"]:
        if item["id"] == item_id:
            return item
    return None


def cmd_add(path, words, priority):
    data = load(path)
    item = {"id": data["next"], "text": " ".join(words), "priority": priority, "done": False}
    data["next"] += 1
    data["items"].append(item)
    save(path, data)
    print(f"added #{item['id']}: {item['text']} (p{priority})")
    return 0


def cmd_list(path, show_all, priority):
    data = load(path)
    items = [i for i in data["items"] if show_all or not i["done"]]
    if priority is not None:
        items = [i for i in items if i["priority"] == priority]
    items.sort(key=lambda i: (i["priority"], i["id"]))
    if not items:
        print("nothing to do")
    for i in items:
        mark = "x" if i["done"] else " "
        print(f"[{mark}] #{i['id']:<3} p{i['priority']}  {i['text']}")
    return 0


def cmd_done(path, item_id):
    data = load(path)
    item = find(data, item_id)
    if item is None:
        print(f"error: no task #{item_id}", file=sys.stderr)
        return 1
    item["done"] = True
    save(path, data)
    print(f"done #{item_id}: {item['text']}")
    return 0


def cmd_rm(path, item_id):
    data = load(path)
    item = find(data, item_id)
    if item is None:
        print(f"error: no task #{item_id}", file=sys.stderr)
        return 1
    data["items"].remove(item)
    save(path, data)
    print(f"removed #{item_id}")
    return 0


def usage_error():
    print(USAGE, file=sys.stderr)
    return 1


def main():
    args = sys.argv[1:]
    path = os.environ.get("TODO_FILE", "todo.json")
    if len(args) >= 2 and args[0] == "--file":
        path = args[1]
        args = args[2:]
    if not args:
        return usage_error()
    cmd, rest = args[0], args[1:]
    if cmd == "add":
        priority = 3
        if "--priority" in rest:
            i = rest.index("--priority")
            try:
                priority = int(rest[i + 1])
            except (IndexError, ValueError):
                return usage_error()
            rest = rest[:i] + rest[i + 2 :]
        if not rest or not 1 <= priority <= 5:
            return usage_error()
        return cmd_add(path, rest, priority)
    if cmd == "list":
        show_all = "--all" in rest
        rest = [a for a in rest if a != "--all"]
        priority = None
        if rest[:1] == ["--priority"] and len(rest) == 2 and rest[1].isdigit():
            priority = int(rest[1])
            rest = []
        if rest:
            return usage_error()
        return cmd_list(path, show_all, priority)
    if cmd in ("done", "rm"):
        if len(rest) != 1 or not rest[0].isdigit():
            return usage_error()
        return (cmd_done if cmd == "done" else cmd_rm)(path, int(rest[0]))
    return usage_error()


if __name__ == "__main__":
    sys.exit(main())
