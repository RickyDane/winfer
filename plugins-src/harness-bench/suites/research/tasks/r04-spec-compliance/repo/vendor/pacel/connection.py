"""A PACEL connection over any object with write(bytes) and close()."""

import struct
from collections import deque

from . import handshake
from .constants import CLOSE, DATA, ERROR, HELLO, HELLO_ACK, PING, RESET, RESET_UNKNOWN_STREAM
from .errors import ConnectionClosed, ProtocolError
from .frame import Frame, decode, encode
from .streams import StreamIds


class Connection:
    def __init__(self, transport, is_client: bool, on_data=None):
        self.transport = transport
        self.is_client = is_client
        self.on_data = on_data or (lambda sid, payload: None)
        self.streams = StreamIds(is_client)
        self.ready = False
        self.closed = False
        self._pending = deque()
        self._buf = b""
        self._remote_seen = set()
        if is_client:
            self._write(handshake.client_hello())

    # ---- sending ----

    def open_stream(self) -> int:
        return self.streams.allocate()

    def send_data(self, sid: int, payload: bytes) -> None:
        if self.closed:
            raise ConnectionClosed()
        frame = Frame(DATA, sid, payload)
        if not self.ready:
            # Section 5.3: hold DATA until the handshake is done.
            self._pending.append(frame)
            return
        self._write(frame)

    def reset_stream(self, sid: int, code: int) -> None:
        self._write(Frame(RESET, sid, bytes([code])))
        self.streams.release(sid)

    def finish_stream(self, sid: int) -> None:
        self.streams.release(sid)

    def ping(self) -> None:
        self._write(Frame(PING, 0))

    def close(self, reason: int = 0) -> None:
        if self.closed:
            return
        self._write(Frame(CLOSE, 0, struct.pack(">H", reason)))
        self.closed = True
        # Don't lose DATA the application queued before the handshake finished.
        self._flush_pending()
        self.transport.close()

    def _flush_pending(self) -> None:
        while self._pending:
            self._write(self._pending.popleft())

    def _write(self, frame: Frame) -> None:
        self.transport.write(encode(frame))

    # ---- receiving ----

    def feed(self, data: bytes) -> None:
        self._buf += data
        while True:
            frame, used = decode(self._buf)
            if frame is None:
                return
            self._buf = self._buf[used:]
            self._handle(frame)

    def _handle(self, frame: Frame) -> None:
        if frame.type == HELLO and not self.is_client:
            answer, chosen = handshake.server_answer(frame)
            self._write(answer)
            if chosen is None:
                self.closed = True
                self.transport.close()
                return
            self.ready = True
            self._flush_pending()
        elif frame.type == HELLO_ACK and self.is_client:
            self.ready = True
            self._flush_pending()
        elif frame.type == DATA:
            if not self.streams.is_open(frame.stream_id) and not self._accept_remote(frame.stream_id):
                self._write(Frame(RESET, frame.stream_id, bytes([RESET_UNKNOWN_STREAM])))
                return
            self.on_data(frame.stream_id, frame.payload)
        elif frame.type == RESET:
            self.streams.release(frame.stream_id)
        elif frame.type == CLOSE:
            self.closed = True
            self.transport.close()
        elif frame.type == ERROR:
            raise ProtocolError(f"peer error {frame.payload.hex()}")

    def _accept_remote(self, sid: int) -> bool:
        """A DATA frame with a new id of the peer's parity opens a stream."""
        peer_parity = 0 if self.is_client else 1
        if sid != 0 and sid % 2 == peer_parity and sid not in self._remote_seen:
            self._remote_seen.add(sid)
            self.streams.open.add(sid)
            return True
        return False
