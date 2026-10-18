from .auth import AuthMiddleware
from .dedupe import DedupeMiddleware
from .metrics import MetricsMiddleware
from .ratelimit import RateLimitMiddleware

REGISTRY = {
    "auth": AuthMiddleware,
    "ratelimit": RateLimitMiddleware,
    "metrics": MetricsMiddleware,
    "dedupe": DedupeMiddleware,
}
