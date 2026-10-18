"""Command-line layer."""

import argparse


def parse(argv):
    p = argparse.ArgumentParser(prog="relay")
    p.add_argument("--config")
    p.add_argument("--upstream")
    p.add_argument("--log-level", dest="log_level")
    p.add_argument("--retries")
    p.add_argument("--timeout", dest="connect_timeout", help="connection timeout in seconds")
    args = vars(p.parse_args(argv))
    out = {k: v for k, v in args.items() if v is not None and k != "connect_timeout"}
    if args.get("connect_timeout") is not None:
        out["http"] = {"connect_timeout": args["connect_timeout"]}
    return out
