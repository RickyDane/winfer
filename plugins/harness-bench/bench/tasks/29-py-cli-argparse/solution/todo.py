#!/usr/bin/env python3
"""A tiny todo list stored in a JSON file.

usage: todo.py [--file PATH] add TEXT... [--priority N]
       todo.py [--file PATH] list [--all] [--priority N]
       todo.py [--file PATH] done ID
       todo.py [--file PATH] rm ID
"""

import argparse
import json
import os
import sys

VERSION = "1.4.0"


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


def priority_type(text):
    try:
        value = int(text)
    except ValueError:
        raise argparse.ArgumentTypeError(f"invalid priority: {text!r}")
    if not 1 <= value <= 5:
        raise argparse.ArgumentTypeError("priority must be between 1 and 5")
    return value


def build_parser():
    parser = argparse.ArgumentParser(prog="todo.py", description="A tiny todo list stored in a JSON file.")
    parser.add_argument("--version", action="version", version=f"todo {VERSION}")
    parser.add_argument("--file", default=os.environ.get("TODO_FILE", "todo.json"), help="todo file (default: $TODO_FILE or todo.json)")
    sub = parser.add_subparsers(dest="cmd", required=True, metavar="COMMAND")

    p = sub.add_parser("add", help="add a task")
    p.add_argument("words", nargs="+", metavar="TEXT")
    p.add_argument("--priority", type=priority_type, default=3)

    p = sub.add_parser("list", help="list tasks")
    p.add_argument("--all", action="store_true", help="include done tasks")
    p.add_argument("--priority", type=int)

    for name, text in (("done", "mark a task done"), ("rm", "remove a task")):
        p = sub.add_parser(name, help=text)
        p.add_argument("id", type=int, metavar="ID")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    if args.cmd == "add":
        return cmd_add(args.file, args.words, args.priority)
    if args.cmd == "list":
        return cmd_list(args.file, args.all, args.priority)
    return (cmd_done if args.cmd == "done" else cmd_rm)(args.file, args.id)


if __name__ == "__main__":
    sys.exit(main())
