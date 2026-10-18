# PACEL draft 01 (superseded, not normative)

- Version byte: 0x01.
- Maximum payload: 65536 bytes.
- Checksum: CRC-32 (IEEE 802.3), as computed by zlib.
- Stream ids: any non-zero value; ids may be reused after RESET.
- On CLOSE, implementations flush queued frames before closing the transport.
