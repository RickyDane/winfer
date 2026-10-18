"""Protocol constants for PACEL v2."""

MAGIC = b"PC"
VERSION = 0x02
# Keep reading frames from peers that still speak the draft version.
ACCEPTED_VERSIONS = (0x01, 0x02)

# Section 3.4.
MAX_PAYLOAD = 1048576

HEADER_FORMAT = ">2sBBBII"  # magic, version, type, flags, stream id, length
HEADER_SIZE = 13
CHECKSUM_SIZE = 4

HELLO, HELLO_ACK, DATA, RESET, PING, CLOSE, ERROR = range(1, 8)

RESERVED_FLAGS = 0xF0

ERR_NO_COMMON_VERSION = 0x10
ERR_STREAM = 0x20
RESET_UNKNOWN_STREAM = 0x21

SUPPORTED_VERSIONS = (0x02,)
