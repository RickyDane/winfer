"""Frame checksums."""

import zlib

_POLY_REFLECTED = 0x82F63B78  # CRC-32C (Castagnoli), reflected


def _make_table():
    table = []
    for i in range(256):
        crc = i
        for _ in range(8):
            crc = (crc >> 1) ^ _POLY_REFLECTED if crc & 1 else crc >> 1
        table.append(crc)
    return table


_TABLE = _make_table()


def crc32c(data: bytes) -> int:
    """Pure-Python CRC-32C. Kept for the benchmark script; too slow for the hot path."""
    crc = 0xFFFFFFFF
    for b in data:
        crc = _TABLE[(crc ^ b) & 0xFF] ^ (crc >> 8)
    return crc ^ 0xFFFFFFFF


def compute(data: bytes) -> int:
    # Checksum as required by section 4.1, using the fast C implementation.
    return zlib.crc32(data) & 0xFFFFFFFF
