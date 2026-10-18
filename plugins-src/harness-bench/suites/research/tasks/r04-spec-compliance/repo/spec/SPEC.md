# PACEL: Packet Channel Exchange Layer, version 2

Status: Final (2025-06-02). Amended by ERRATA.md.

## 1. Conventions

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are to be interpreted as in RFC 2119.
Only MUST and MUST NOT requirements are conformance requirements. All multi-byte integers are
unsigned and big-endian (network order) unless stated otherwise.

## 2. Overview

PACEL multiplexes independent streams of messages over one reliable byte transport. Every unit of
data is a frame. A connection begins with a handshake (section 5) and ends with a CLOSE frame
(section 7).

## 3. Frame format

    +--------+---------+------+-------+-----------+--------+---------+----------+
    | magic  | version | type | flags | stream id | length | payload | checksum |
    | 2      | 1       | 1    | 1     | 4         | 4      | length  | 4        |
    +--------+---------+------+-------+-----------+--------+---------+----------+

Frame types: HELLO = 1, HELLO_ACK = 2, DATA = 3, RESET = 4, PING = 5, CLOSE = 6, ERROR = 7.

### 3.1 Magic

Encoders MUST write the two bytes 0x50 0x43 ("PC") at the start of every frame. Decoders MUST
reject a frame whose first two bytes differ.

### 3.2 Version

The version byte MUST be 0x02 in every frame sent. Decoders MUST reject frames carrying any other
version value, including 0x01 (pre-standard drafts).

### 3.3 Length

The length field MUST be a 4-byte big-endian unsigned integer counting the payload bytes only
(not the header or the checksum).

### 3.4 Maximum payload

Decoders MUST reject any frame whose length field exceeds 1048576 (1 MiB) without reading the
payload.

### 3.5 Flags

Bits 0-3 of the flags byte are defined per frame type. Bits 4-7 are reserved: senders MUST set them
to zero, and receivers SHOULD ignore them.

### 3.6 Alignment

Encoders MUST pad the payload with zero bytes to a multiple of 4 bytes; the length field counts
the padding.

## 4. Integrity

### 4.1 Checksum algorithm

The checksum MUST be computed with CRC-32C (Castagnoli, polynomial 0x1EDC6F41) over the header and
the payload. Receivers MUST verify it and MUST discard frames whose checksum does not match.

### 4.2 Checksum encoding

The checksum MUST be appended after the payload as a 4-byte big-endian integer.

## 5. Handshake

### 5.1 HELLO

The client MUST send a HELLO frame on stream 0 as its first frame. Its payload is the list of
protocol versions the client supports, one byte each.

### 5.2 HELLO_ACK

The server MUST answer a HELLO with a HELLO_ACK whose payload is the single chosen version, or, if
it shares no version with the client, MUST send an ERROR frame with code 0x10 and close the
connection.

### 5.3 Ordering

A peer MUST NOT send DATA frames before the handshake has completed (the client has received
HELLO_ACK, or the server has sent it).

### 5.4 Keepalive

Peers SHOULD send a PING frame on stream 0 when the connection has been idle for 30 seconds.

## 6. Streams

### 6.1 Stream identifiers

Streams opened by the client MUST use odd identifiers and streams opened by the server MUST use
even identifiers. Stream 0 is reserved for connection-level frames. A stream is opened by the
first DATA frame carrying an identifier of the sender's parity that has not been used before on
the connection.

### 6.2 No reuse

An endpoint MUST NOT reuse a stream identifier within the lifetime of a connection, even after the
stream has been closed or reset.

### 6.3 Unknown streams

On receiving a DATA frame for a stream identifier that is neither open nor a new identifier of the
sender's parity (section 6.1), an endpoint MUST respond with a RESET frame for that stream carrying
the error code 0x20.

## 7. Closing

### 7.1 CLOSE

A CLOSE frame MUST carry a payload of exactly two bytes: the big-endian reason code.

### 7.2 After CLOSE

After sending a CLOSE frame an endpoint MUST NOT send any further frames on the connection.

## 8. Security considerations

Implementations SHOULD bound the number of concurrently open streams. PACEL provides no
confidentiality; run it over TLS when needed.
