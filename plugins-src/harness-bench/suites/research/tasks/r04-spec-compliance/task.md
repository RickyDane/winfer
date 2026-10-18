This folder contains the PACEL v2 protocol specification (`spec/`) and a vendored Python implementation of it (`vendor/pacel/`).

Find every conformance requirement of the specification that the implementation violates.

Rules:
- The normative specification is `spec/SPEC.md` as amended by `spec/ERRATA.md`; where they differ, the errata win. Drafts (`spec/drafts/`) and mailing-list threads (`spec/mail/`) are background only, not normative.
- Only MUST / MUST NOT requirements count (not SHOULD or MAY).
- Judge what the code in `vendor/pacel/` actually does on any reachable code path, not what its comments or README claim. Tests don't count as part of the implementation.

Write `ANSWER.json` with exactly this shape, listing each violated requirement once by its section number:

```json
{
  "violated_sections": ["<section number, e.g. 2.1>", "..."]
}
```

Don't change any other file.
