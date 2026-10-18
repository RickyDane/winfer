import importlib
import json
import socketserver

from courier import handlers
from courier.app.pipeline import build_pipeline
from courier.delivery.policy import RetryPolicy
from courier.delivery.worker import Worker
from courier.handlers.registry import dispatch
from courier.storage import make_store


def load_plugins(settings):
    for name in settings.get("plugins.enabled", "").split(","):
        if name.strip():
            importlib.import_module(f"courier.plugins.{name.strip()}")


def create_app(settings):
    load_plugins(settings)
    store = make_store(settings)
    worker = Worker(RetryPolicy(settings), store)

    def final(msg):
        result = dispatch(msg)
        if result is not None:
            worker.deliver(result)
        return result

    return build_pipeline(settings, final)


def serve(settings, port):
    app = create_app(settings)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            for line in self.rfile:
                app(json.loads(line))

    with socketserver.ThreadingTCPServer(("0.0.0.0", port), Handler) as srv:
        srv.serve_forever()
