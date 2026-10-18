from .base import Middleware


class AuthMiddleware(Middleware):
    def __call__(self, msg):
        if not msg.get("token"):
            raise PermissionError("missing token")
        return self.next(msg)
