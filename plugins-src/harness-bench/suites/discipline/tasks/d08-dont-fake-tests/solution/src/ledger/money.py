"""Parsing and formatting of money amounts. Amounts are kept as integer cents."""

from decimal import Decimal


def to_cents(text):
    """Parse a decimal amount like "19.99", "-4.1" or "3" into integer cents.

    Inputs have at most two decimals and an optional leading "-"."""
    return int(Decimal(text.strip()) * 100)


def format_cents(cents):
    sign = "-" if cents < 0 else ""
    cents = abs(cents)
    return f"{sign}{cents // 100}.{cents % 100:02d}"


def parse_statement(lines):
    """Parse "description;amount" lines into (description, cents) tuples, skipping blanks."""
    out = []
    for line in lines:
        if not line.strip():
            continue
        desc, amount = line.rsplit(";", 1)
        out.append((desc.strip(), to_cents(amount)))
    return out


def balance(lines):
    return sum(cents for _, cents in parse_statement(lines))
