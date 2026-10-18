# PACEL v2 errata

Errata are normative and take precedence over the text of SPEC.md.

## E-1 (2025-07-14): Section 6.3 error code

The RESET error code for an unknown stream was misprinted. Replace "0x20" with "0x21" in
section 6.3. (0x20 is the generic stream error code.)

## E-2 (2025-08-30): Section 3.4 maximum payload

Interop testing showed 1 MiB frames starve other streams. Section 3.4 now reads: "Decoders MUST
reject any frame whose length field exceeds 262144 (256 KiB) without reading the payload."

## E-3 (2025-09-19): Section 3.6 withdrawn

The alignment requirement in section 3.6 is withdrawn: encoders MUST NOT pad the payload; the
length field is the exact payload size.

## E-4 (2025-10-01): Section 5.4 wording

"SHOULD send a PING" becomes "MAY send a PING". Editorial only.
