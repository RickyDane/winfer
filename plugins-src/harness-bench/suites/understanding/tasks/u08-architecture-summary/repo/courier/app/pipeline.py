"""Builds the middleware pipeline. Middlewares run in the order they're configured."""

from courier.middleware import REGISTRY
from courier.middleware.tracing import TracingMiddleware


def build_pipeline(settings, final_handler):
    configured = settings.get("pipeline.middleware", settings.get("middleware"))
    names = [n.strip() for n in configured.split(",") if n.strip()]
    classes = [REGISTRY[n] for n in names] + [TracingMiddleware]
    handler = final_handler
    for cls in classes:
        handler = cls(handler)
    return handler
