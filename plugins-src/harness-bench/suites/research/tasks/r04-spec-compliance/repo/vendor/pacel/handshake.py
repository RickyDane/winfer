"""HELLO / HELLO_ACK negotiation (section 5)."""

from .constants import ERR_NO_COMMON_VERSION, ERROR, HELLO, HELLO_ACK, SUPPORTED_VERSIONS
from .frame import Frame


def client_hello() -> Frame:
    return Frame(HELLO, 0, bytes(SUPPORTED_VERSIONS))


def server_answer(hello: Frame):
    """Returns (frame to send, chosen version or None). With None the caller closes the connection."""
    common = [v for v in hello.payload if v in SUPPORTED_VERSIONS]
    if not common:
        return Frame(ERROR, 0, bytes([ERR_NO_COMMON_VERSION])), None
    chosen = max(common)
    return Frame(HELLO_ACK, 0, bytes([chosen])), chosen
