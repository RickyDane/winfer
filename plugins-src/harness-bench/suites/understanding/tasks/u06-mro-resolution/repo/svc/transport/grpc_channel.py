"""gRPC transport. Requires the internal `grpcx` wheel."""

import grpcx


def channel(target):
    return grpcx.insecure_channel(target)
