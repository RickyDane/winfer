import argparse

from courier.config.settings import load


def run(argv=None):
    parser = argparse.ArgumentParser("courier")
    parser.add_argument("command", nargs="?", default="serve", choices=["serve", "check"])
    parser.add_argument("--port", type=int, default=7600)
    args = parser.parse_args(argv)
    settings = load()
    if args.command == "check":
        print("config ok")
        return
    from courier.app.server import serve

    serve(settings, port=args.port)
