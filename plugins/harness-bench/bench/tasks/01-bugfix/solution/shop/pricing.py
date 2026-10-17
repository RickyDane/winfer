"""Price helpers. Prices are integers in cents."""

TAX_RATE = 0.19


def apply_discount(cents: int, percent: int) -> int:
    """Price after a percentage discount, rounded to the nearest cent (halves round up)."""
    if not 0 <= percent <= 100:
        raise ValueError("percent must be between 0 and 100")
    return (cents * (100 - percent) + 50) // 100


def with_tax(cents: int) -> int:
    """Price including tax, rounded to the nearest cent (halves round up)."""
    return int(cents * (1 + TAX_RATE) + 0.5)
