#!/usr/bin/env python3
"""A tiny todo list stored in todos.json (or $TODO_FILE)."""

import argparse
import json
import os
import sys

PATH = os.environ.get("TODO_FILE", "todos.json")
PRIORITIES = ["high", "med", "low"]


def load():
    if not os.path.exists(PATH):
        return []
    with open(PATH) as f:
        todos = json.load(f)
    for t in todos:
        t.setdefault("priority", "med")
    return todos


def save(todos):
    with open(PATH, "w") as f:
        json.dump(todos, f, indent=2)


def next_id(todos):
    return max((t["id"] for t in todos), default=0) + 1


def cmd_add(args):
    todos = load()
    todo = {"id": next_id(todos), "text": args.text, "done": False, "priority": args.priority}
    todos.append(todo)
    save(todos)
    print(f"Added {todo['id']}")


def cmd_list(args):
    todos = load()
    if args.sort == "priority":
        todos = sorted(todos, key=lambda t: PRIORITIES.index(t["priority"]))
    for t in todos:
        mark = "x" if t["done"] else " "
        print(f"{t['id']}. [{t['priority']}] [{mark}] {t['text']}")


def cmd_done(args):
    todos = load()
    for t in todos:
        if t["id"] == args.id:
            t["done"] = True
            save(todos)
            print(f"Done {t['id']}")
            return
    print(f"No todo {args.id}", file=sys.stderr)
    sys.exit(1)


def cmd_clear_done(args):
    todos = load()
    kept = [t for t in todos if not t["done"]]
    save(kept)
    print(f"Removed {len(todos) - len(kept)}")


def main(argv=None):
    p = argparse.ArgumentParser(prog="todo")
    sub = p.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("add")
    a.add_argument("text")
    a.add_argument("--priority", choices=["low", "med", "high"], default="med")
    a.set_defaults(fn=cmd_add)
    l = sub.add_parser("list")
    l.add_argument("--sort", choices=["priority"])
    l.set_defaults(fn=cmd_list)
    d = sub.add_parser("done")
    d.add_argument("id", type=int)
    d.set_defaults(fn=cmd_done)
    c = sub.add_parser("clear-done")
    c.set_defaults(fn=cmd_clear_done)
    args = p.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
