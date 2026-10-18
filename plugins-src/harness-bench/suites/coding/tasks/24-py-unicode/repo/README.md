# textutils

Helpers in `textutils.py`. All functions take and return `str` and must work for any Unicode
text, not just ASCII. Use only the Python standard library (`unicodedata`, `re`, ...).

## `slugify(text) -> str`

1. Apply this transliteration table first (both cases): `ß`/`ẞ` → `ss`, `æ` → `ae`, `œ` → `oe`,
   `ø` → `o`, `đ` → `d`, `ł` → `l`, `þ` → `th` (uppercase forms map to the same lowercase output).
2. Decompose (NFKD) and drop all combining marks, so `é` → `e`, `ﬁ` → `fi`, `①` → `1`.
3. Lowercase. Every run of characters that are not ASCII `a-z`/`0-9` becomes a single `-`;
   leading and trailing hyphens are removed.
4. If nothing is left, return `n-a`.

Examples: `"Crème Brûlée"` → `crème` → `creme-brulee`; `"Straße 5"` → `strasse-5`;
`"Ærøskøbing"` → `aeroskobing`; `"日本語"` → `n-a`; `"Hello, 🌍 World!"` → `hello-world`.

## `truncate(text, limit) -> str`

`limit` counts user-perceived characters, not code points. A character is one base code point
plus everything attached to it:

- combining marks (Unicode categories Mn, Mc, Me), e.g. `e` + U+0301;
- variation selectors U+FE0E / U+FE0F and emoji skin-tone modifiers U+1F3FB–U+1F3FF;
- a ZERO WIDTH JOINER (U+200D) together with the code point that follows it
  (so `👩‍💻` and `👨‍👩‍👧` are one character each);
- two consecutive regional indicator symbols (U+1F1E6–U+1F1FF) form one flag, e.g. `🇩🇪`.

If the text has at most `limit` characters it is returned unchanged. Otherwise return the first
`limit - 1` characters (never splitting one) followed by `…`, so the result has exactly `limit`
characters. The text is not normalized. `limit < 1` raises `ValueError`.

## `equals_ignore_case(a, b) -> bool`

Caseless match that ignores canonical-equivalence differences: compare
`NFD(casefold(NFD(s)))` of both strings. So `"Straße" == "STRASSE"`, composed `"é"` equals
decomposed `"é"`, and `"ΣΊΣΥΦΟΣ"` equals `"σίσυφος"`. (Turkish dotted/dotless i is out of scope.)

## `word_count(text) -> int`

A word is a maximal run of letters and digits in any script (`str.isalnum()`), where combining
marks directly after a letter or digit belong to that word, and an apostrophe (`'` or `’`)
between two letters/digits joins them (`don’t` is one word, `'quoted'` is one word `quoted`).
Everything else (spaces, punctuation, hyphens, emoji, symbols) separates words.
`"Ça va? Très bien — naïve café-crème"` has 7 words.
