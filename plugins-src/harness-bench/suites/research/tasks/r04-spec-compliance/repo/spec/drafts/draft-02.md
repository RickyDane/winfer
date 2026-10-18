# PACEL draft 02 (superseded, not normative)

- Version byte: 0x02. Decoders MAY accept 0x01 for compatibility with draft-01 peers.
- Maximum payload: 1048576 bytes.
- Checksum: CRC-32C replaces CRC-32.
- Unknown stream: RESET with code 0x20.
- Payload padded to 4 bytes.
