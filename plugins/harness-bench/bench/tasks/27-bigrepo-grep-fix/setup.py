"""Generates a ~150-file invoicing project into argv[1] (deterministic).

ledger/currencies/<code>.py each format money for one currency; exactly one of them (NOK)
rounds down instead of half-up, which makes one invoice in tests/test_invoices.py wrong.
"""

import os
import random
import sys
from decimal import ROUND_DOWN, ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal

RM = {"HALF_UP": ROUND_HALF_UP, "HALF_EVEN": ROUND_HALF_EVEN, "DOWN": ROUND_DOWN}

root = sys.argv[1]
rng = random.Random(2701)


def write(rel, text):
    path = os.path.join(root, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text.lstrip("\n"))


# code, symbol, decimals, symbol after amount, thousands sep, decimal sep, rounding mode, cash step
CURRENCIES = [
    ("USD", "$", 2, False, ",", ".", "HALF_UP", None),
    ("EUR", "€", 2, True, ".", ",", "HALF_UP", None),
    ("GBP", "£", 2, False, ",", ".", "HALF_UP", None),
    ("JPY", "¥", 0, False, ",", ".", "HALF_UP", None),
    ("CHF", "CHF ", 2, False, "'", ".", "HALF_UP", "0.05"),
    ("SEK", " kr", 2, True, " ", ",", "HALF_UP", None),
    ("NOK", " kr", 2, True, " ", ",", "HALF_UP", None),
    ("DKK", " kr.", 2, True, ".", ",", "HALF_UP", None),
    ("PLN", " zł", 2, True, " ", ",", "HALF_UP", None),
    ("CZK", " Kč", 2, True, " ", ",", "HALF_UP", None),
    ("HUF", " Ft", 0, True, " ", ",", "HALF_EVEN", None),
    ("KRW", "₩", 0, False, ",", ".", "HALF_UP", None),
    ("INR", "₹", 2, False, ",", ".", "HALF_UP", None),
    ("BRL", "R$ ", 2, False, ".", ",", "HALF_UP", None),
    ("MXN", "MX$", 2, False, ",", ".", "HALF_UP", None),
    ("CAD", "CA$", 2, False, ",", ".", "HALF_UP", None),
    ("AUD", "A$", 2, False, ",", ".", "HALF_UP", None),
    ("NZD", "NZ$", 2, False, ",", ".", "HALF_UP", None),
    ("SGD", "S$", 2, False, ",", ".", "HALF_UP", None),
    ("HKD", "HK$", 2, False, ",", ".", "HALF_UP", None),
    ("CNY", "CN¥", 2, False, ",", ".", "HALF_UP", None),
    ("ZAR", "R ", 2, False, " ", ".", "HALF_UP", None),
    ("TRY", "₺", 2, False, ".", ",", "HALF_UP", None),
    ("ILS", "₪", 2, False, ",", ".", "HALF_UP", None),
    ("AED", "AED ", 2, False, ",", ".", "HALF_UP", None),
    ("KWD", "KD ", 3, False, ",", ".", "HALF_UP", None),
    ("BHD", "BD ", 3, False, ",", ".", "HALF_UP", None),
    ("CLP", "CLP$", 0, False, ".", ",", "HALF_EVEN", None),
    ("ISK", " kr", 0, True, ".", ",", "DOWN", None),
    ("THB", "฿", 2, False, ",", ".", "HALF_UP", None),
    ("RON", " lei", 2, True, ".", ",", "HALF_UP", None),
    ("BGN", " лв.", 2, True, " ", ",", "HALF_UP", None),
]
BUGGY = "NOK"

write("ledger/__init__.py", '"""Invoicing toolkit."""\n')
write("ledger/core/__init__.py", "")
write("ledger/core/rounding.py", '''
"""Rounding helpers. All money math uses Decimal."""

from decimal import ROUND_DOWN, ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal

MODES = {"HALF_UP": ROUND_HALF_UP, "HALF_EVEN": ROUND_HALF_EVEN, "DOWN": ROUND_DOWN}


def quantize(value, decimals, mode="HALF_UP"):
    """Rounds value (Decimal, int or str) to `decimals` places."""
    exp = Decimal(1).scaleb(-decimals)
    return Decimal(value).quantize(exp, rounding=MODES[mode])


def to_step(value, step, mode="HALF_UP"):
    """Rounds value to a multiple of step (cash rounding, e.g. 0.05)."""
    step = Decimal(step)
    return (Decimal(value) / step).quantize(Decimal(1), rounding=MODES[mode]) * step
''')
write("ledger/core/grouping.py", '''
def group_digits(digits, sep):
    """'1234567' -> '1,234,567' for sep=','."""
    out = []
    while len(digits) > 3:
        out.insert(0, digits[-3:])
        digits = digits[:-3]
    out.insert(0, digits)
    return sep.join(out)
''')
write("ledger/currencies/__init__.py", '''
"""One module per currency. `get(code)` returns the module."""

import importlib

CODES = %r


def get(code):
    if code not in CODES:
        raise KeyError(f"unknown currency {code}")
    return importlib.import_module(f"ledger.currencies.{code.lower()}")
''' % [c[0] for c in CURRENCIES])

for code, sym, dec, after, ts, ds, mode, step in CURRENCIES:
    real_mode = "DOWN" if code == BUGGY else mode
    rounding = (
        f'    q = to_step(quantize(value, DECIMALS + 1, ROUNDING), CASH_STEP, ROUNDING)\n'
        f'    q = quantize(q, DECIMALS, ROUNDING)\n'
        if step
        else f"    q = quantize(value, DECIMALS, ROUNDING)\n"
    )
    write(f"ledger/currencies/{code.lower()}.py", f'''
"""{code} money formatting."""

from decimal import Decimal

from ledger.core.grouping import group_digits
from ledger.core.rounding import quantize{", to_step" if step else ""}

CODE = "{code}"
SYMBOL = {sym!r}
DECIMALS = {dec}
SYMBOL_AFTER = {after}
THOUSANDS = {ts!r}
DECIMAL_POINT = {ds!r}
ROUNDING = "{real_mode}"
{f"CASH_STEP = {step!r}" if step else ""}

def round_amount(value):
{rounding}    return q


def format_amount(value):
    """value: Decimal (major units). Returns e.g. {"'1 234,50 kr'" if after else "'$1,234.50'"}."""
    q = round_amount(Decimal(value))
    sign = "-" if q < 0 else ""
    whole, _, frac = f"{{abs(q):.{{DECIMALS}}f}}".partition(".")
    text = group_digits(whole, THOUSANDS) + (DECIMAL_POINT + frac if DECIMALS else "")
    return sign + (text + SYMBOL if SYMBOL_AFTER else SYMBOL + text)
''')

write("ledger/invoice.py", '''
"""Invoices: lines of (description, quantity, unit price) in one currency."""

from decimal import Decimal

from ledger import currencies
from ledger.tax import rate_for


class Invoice:
    def __init__(self, number, currency, country):
        self.number = number
        self.currency = currencies.get(currency)
        self.country = country
        self.lines = []

    def add(self, description, qty, unit_price):
        self.lines.append((description, qty, Decimal(str(unit_price))))
        return self

    def net(self):
        return sum((qty * price for _, qty, price in self.lines), Decimal(0))

    def tax(self):
        return self.net() * rate_for(self.country)

    def gross(self):
        return self.net() + self.tax()

    def total_text(self):
        return self.currency.format_amount(self.gross())
''')

COUNTRIES = {"US": "0", "DE": "0.19", "FR": "0.20", "GB": "0.20", "JP": "0.10", "CH": "0.081", "SE": "0.25",
             "NO": "0.25", "DK": "0.25", "PL": "0.23", "CZ": "0.21", "HU": "0.27", "KR": "0.10", "IN": "0.18",
             "BR": "0.17", "MX": "0.16", "CA": "0.05", "AU": "0.10", "NZ": "0.15", "SG": "0.09", "HK": "0",
             "CN": "0.13", "ZA": "0.15", "TR": "0.20", "IL": "0.17", "AE": "0.05", "KW": "0", "BH": "0.10",
             "CL": "0.19", "IS": "0.24", "TH": "0.07", "RO": "0.19", "BG": "0.20"}
write("ledger/tax.py", '''
from decimal import Decimal

RATES = %r


def rate_for(country):
    return Decimal(RATES.get(country, "0"))
''' % COUNTRIES)

# Filler: plausible modules so the bug has to be found, not stumbled upon.
AREAS = ["customers", "payments", "reminders", "exports", "imports", "audit", "reports", "webhooks", "sync", "notifications"]
NOUNS = ["record", "entry", "batch", "event", "item", "job", "request", "message", "snapshot", "token", "note", "rule"]
VERBS = ["load", "save", "validate", "merge", "split", "normalize", "summarize", "render", "parse", "schedule"]
for area in AREAS:
    write(f"ledger/{area}/__init__.py", f'"""{area.title()} subsystem."""\n')
    for k in range(10):
        noun = NOUNS[(k + len(area)) % len(NOUNS)]
        fns = []
        for j in range(3):
            verb = VERBS[rng.randrange(len(VERBS))]
            limit = rng.choice([10, 50, 100, 250, 1000])
            fns.append(f'''
def {verb}_{noun}_{j}(items, limit={limit}):
    """{verb.title()}s {noun}s for {area}; keeps at most `limit`."""
    out = []
    for item in items[:limit]:
        if isinstance(item, dict):
            item = {{k: v for k, v in item.items() if v is not None}}
        out.append(item)
    return out
''')
        write(f"ledger/{area}/{noun}_{k}.py", f'"""{noun.title()} helpers for {area}."""\n' + "\n".join(fns))

# Tests: a handful of invoices across currencies; expected strings computed independently here.
def expected_total(code, country, lines):
    c = next(x for x in CURRENCIES if x[0] == code)
    _, sym, dec, after, ts, ds, mode, step = c
    net = sum((qty * Decimal(str(p)) for _, qty, p in lines), Decimal(0))
    gross = net + net * Decimal(COUNTRIES[country])
    if step:
        q = Decimal(gross).quantize(Decimal(1).scaleb(-(dec + 1)), rounding=ROUND_HALF_UP)
        q = (q / Decimal(step)).quantize(Decimal(1), rounding=ROUND_HALF_UP) * Decimal(step)
    else:
        q = gross
    q = Decimal(q).quantize(Decimal(1).scaleb(-dec), rounding=RM[mode])
    sign = "-" if q < 0 else ""
    whole, _, frac = f"{abs(q):.{dec}f}".partition(".")
    parts = []
    while len(whole) > 3:
        parts.insert(0, whole[-3:])
        whole = whole[:-3]
    parts.insert(0, whole)
    text = ts.join(parts) + (ds + frac if dec else "")
    return sign + (text + sym if after else sym + text)


CURRENCY_COUNTRY = {"USD": "US", "EUR": "DE", "GBP": "GB", "JPY": "JP", "CHF": "CH", "SEK": "SE", "NOK": "NO",
                    "DKK": "DK", "PLN": "PL", "CZK": "CZ", "HUF": "HU", "KRW": "KR", "INR": "IN", "BRL": "BR",
                    "MXN": "MX", "CAD": "CA", "AUD": "AU", "NZD": "NZ", "SGD": "SG", "HKD": "HK", "CNY": "CN",
                    "ZAR": "ZA", "TRY": "TR", "ILS": "IL", "AED": "AE", "KWD": "KW", "BHD": "BH", "CLP": "CL",
                    "ISK": "IS", "THB": "TH", "RON": "RO", "BGN": "BG"}
cases = []
num = 1000
for code in [c[0] for c in CURRENCIES]:
    for _ in range(2):
        num += rng.randrange(1, 40)
        lines = [(f"item {i}", rng.randrange(1, 9), rng.choice(["0.99", "1.25", "12.10", "19.95", "249.00", "3.333", "1234.56", "7.45"]))
                 for i in range(rng.randrange(1, 4))]
        cases.append((f"INV-{num}", code, CURRENCY_COUNTRY[code], lines))
rows = "\n".join(f"    ({n!r}, {c!r}, {cc!r}, {l!r}, {expected_total(c, cc, l)!r})," for n, c, cc, l in cases)
write("tests/__init__.py", "")
write("tests/test_invoices.py", f'''
import unittest

from ledger.invoice import Invoice

CASES = [
{rows}
]


class Invoices(unittest.TestCase):
    def test_totals(self):
        for number, currency, country, lines, want in CASES:
            inv = Invoice(number, currency, country)
            for desc, qty, price in lines:
                inv.add(desc, qty, price)
            self.assertEqual(inv.total_text(), want, f"total of {{number}}")


if __name__ == "__main__":
    unittest.main()
''')
write("tests/test_grouping.py", '''
import unittest

from ledger.core.grouping import group_digits


class Grouping(unittest.TestCase):
    def test_group(self):
        self.assertEqual(group_digits("1234567", ","), "1,234,567")
        self.assertEqual(group_digits("12", " "), "12")
        self.assertEqual(group_digits("123", " "), "123")


if __name__ == "__main__":
    unittest.main()
''')
write("README.md", "# ledger\n\nInvoicing toolkit. Run the tests with `python3 -m unittest discover -s tests`.\n")
