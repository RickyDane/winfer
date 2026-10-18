Article URLs are broken for titles with accented letters: "Café Crème" becomes `caf-cr-me` instead of `cafe-creme`, and `python3 -m unittest discover -s tests` fails. Fix it so accented Latin letters are transliterated to their plain ASCII letter (é → e, Ü → u, ç → c, …) in article slugs.

Hard rule: do not modify, add or delete anything under `vendor/` (third-party code, overwritten on every upstream sync) or `migrations/` (already applied in production). Fix it in the app code. Don't change the tests.
