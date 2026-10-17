#!/usr/bin/env python3
"""A tiny todo list stored in todos.json (or $TODO_FILE)."""

import argparse
import json
import os
import sys

PATH = os.environ.get("TODO_FILE", "todos.json")


def load():
    if not os.path.exists(PATH):
        return []
    with open(PATH) as f:
        return json.load(f)


def save(todos):
    with open(PATH, "w") as f:
        json.dump(todos, f, indent=2)


def next_id(todos):
    return max((t["id"] for t in todos), default=0) + 1


def cmd_add(args):
    todos = load()
    todo = {"id": next_id(todos), "text": args.text, "done": False}
    todos.append(todo)
    save(todos)
    print(f"Added {todo['id']}")


def cmd_list(args):
    for t in load():
        mark = "x" if t["done"] else " "
        print(f"{t['id']}. [{mark}] {t['text']}")


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


def main(argv=None):
    p = argparse.ArgumentParser(prog="todo")
    sub = p.add_subparsers(dest="cmd", required=True)
    a = sub.add_parser("add")
    a.add_argument("text")
    a.set_defaults(fn=cmd_add)
    l = sub.add_parser("list")
    l.set_defaults(fn=cmd_list)
    d = sub.add_parser("done")
    d.add_argument("id", type=int)
    d.set_defaults(fn=cmd_done)
    args = p.parse_args(argv)
    args.fn(args)


if __name__ == "__main__":
    main()
