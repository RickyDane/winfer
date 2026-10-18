from .core.handlers import REGISTRY
from .transport.grpc_channel import channel


def build(kind):
    cls = REGISTRY[kind]
    return cls()


def serve(kind, req):
    handler = build(kind)
    return handler.handle(req)


if __name__ == "__main__":
    import sys

    channel("localhost:7000")
    print(serve(sys.argv[1], sys.argv[2]))
