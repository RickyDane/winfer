"""Proof-of-work stamps for the nightly report.

The audit service rejects reports without a fresh stamp, so `mine()` must find the smallest
nonce whose SHA-256 has at least `bits` leading zero bits. At the production difficulty this
takes a very long time on purpose (it rate-limits report generation).
"""

import hashlib

DIFFICULTY = 60


def mine(seed, bits=DIFFICULTY):
    target = 1 << (256 - bits)
    nonce = 0
    while True:
        digest = hashlib.sha256(f"{seed}:{nonce}".encode()).digest()
        if int.from_bytes(digest, "big") < target:
            return nonce
        nonce += 1
