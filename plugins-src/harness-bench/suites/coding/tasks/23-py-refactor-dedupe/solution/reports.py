"""Plain-text reports for the back office.

Each report groups rows, totals them and renders a fixed-width table. The output format is
consumed by other scripts, so it must stay byte-for-byte stable.
"""

WIDTH = 32


def _by_total(totals):
    return lambda k: (-totals[k], k)


def _money(value):
    return f"{value / 100:>12.2f}"


def _negative_money(value):
    return f"{'(' + format(value / 100, '.2f') + ')':>12}"


def _render_report(rows, title, keep, group, value, fmt, total_label, by_total=True):
    totals = {}
    counts = {}
    for row in rows:
        if not keep(row):
            continue
        key = str(row.get(group, "")).strip() or "(none)"
        totals[key] = totals.get(key, 0) + row[value]
        counts[key] = counts.get(key, 0) + 1
    lines = [title, "=" * len(title)]
    if not totals:
        lines.append("(no data)")
        return "\n".join(lines) + "\n"
    order = sorted(totals, key=_by_total(totals)) if by_total else sorted(totals)
    for key in order:
        lines.append(f"{key + f' ({counts[key]})':<20}{fmt(totals[key])}")
    lines.append("-" * WIDTH)
    lines.append(f"{total_label:<20}{fmt(sum(totals.values()))}")
    return "\n".join(lines) + "\n"


def sales_report(rows, title="SALES BY REGION"):
    """rows: dicts with region, amount_cents and status. Only paid rows count."""
    return _render_report(rows, title, lambda r: r.get("status") == "paid", "region", "amount_cents", _money, "TOTAL")


def refund_report(rows, title="REFUNDS BY REGION"):
    """rows: dicts with region, amount_cents and status. Only refunded rows count."""
    return _render_report(
        rows, title, lambda r: r.get("status") == "refunded", "region", "amount_cents", _negative_money, "TOTAL REFUNDED", by_total=False
    )


def inventory_report(rows, title="STOCK BY WAREHOUSE"):
    """rows: dicts with warehouse, sku and qty. Rows with no stock are skipped."""
    return _render_report(rows, title, lambda r: r.get("qty", 0) > 0, "warehouse", "qty", lambda v: f"{v:>12d}", "TOTAL UNITS")
