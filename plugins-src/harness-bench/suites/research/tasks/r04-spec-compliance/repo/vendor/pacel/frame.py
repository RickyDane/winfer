"""Encoding and decoding of PACEL frames (section 3 and 4)."""

import struct
from dataclasses import dataclass

from . import checksum
from .constants import (ACCEPTED_VERSIONS, CHECKSUM_SIZE, HEADER_FORMAT, HEADER_SIZE, MAGIC,
                        MAX_PAYLOAD, RESERVED_FLAGS, VERSION)
from .errors import ChecksumMismatch, FrameTooLarge, ProtocolError


@dataclass
class Frame:
    type: int
    stream_id: int
    payload: bytes = b""
    flags: int = 0


def encode(frame: Frame) -> bytes:
    flags = frame.flags & ~RESERVED_FLAGS & 0xFF
    header = struct.pack(HEADER_FORMAT, MAGIC, VERSION, frame.type, flags, frame.stream_id, len(frame.payload))
    body = header + frame.payload
    return body + struct.pack(">I", checksum.compute(body))


def decode(buf: bytes):
    """Decodes one frame from the start of `buf`. Returns (frame, bytes consumed) or (None, 0) if
    more data is needed."""
    if len(buf) < HEADER_SIZE:
        return None, 0
    magic, version, ftype, flags, stream_id, length = struct.unpack(HEADER_FORMAT, buf[:HEADER_SIZE])
    if magic != MAGIC:
        raise ProtocolError("bad magic")
    if version not in ACCEPTED_VERSIONS:
        raise ProtocolError(f"unsupported version {version}")
    if length > MAX_PAYLOAD:
        raise FrameTooLarge(length)
    end = HEADER_SIZE + length
    if len(buf) < end + CHECKSUM_SIZE:
        return None, 0
    (expected,) = struct.unpack(">I", buf[end:end + CHECKSUM_SIZE])
    if checksum.compute(buf[:end]) != expected:
        raise ChecksumMismatch()
    return Frame(ftype, stream_id, bytes(buf[HEADER_SIZE:end]), flags), end + CHECKSUM_SIZE
