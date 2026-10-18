"""Plain-text reports for the back office.

Each report groups rows, totals them and renders a fixed-width table. The output format is
consumed by other scripts, so it must stay byte-for-byte stable.
"""

WIDTH = 32


def sales_report(rows, title="SALES BY REGION"):
    """rows: dicts with region, amount_cents and status. Only paid rows count."""
    totals = {}
    counts = {}
    for row in rows:
        if row.get("status") != "paid":
            continue
        key = str(row.get("region", "")).strip() or "(none)"
        totals[key] = totals.get(key, 0) + row["amount_cents"]
        counts[key] = counts.get(key, 0) + 1
    lines = [title, "=" * len(title)]
    if not totals:
        lines.append("(no data)")
        return "\n".join(lines) + "\n"
    grand = 0
    for key in sorted(totals, key=lambda k: (-totals[k], k)):
        value = totals[key]
        grand += value
        label = f"{key} ({counts[key]})"
        lines.append(f"{label:<20}{value / 100:>12.2f}")
    lines.append("-" * WIDTH)
    lines.append(f"{'TOTAL':<20}{grand / 100:>12.2f}")
    return "\n".join(lines) + "\n"


def refund_report(rows, title="REFUNDS BY REGION"):
    """rows: dicts with region, amount_cents and status. Only refunded rows count."""
    totals = {}
    counts = {}
    for row in rows:
        if row.get("status") != "refunded":
            continue
        key = str(row.get("region", "")).strip() or "(none)"
        totals[key] = totals.get(key, 0) + row["amount_cents"]
        counts[key] = counts.get(key, 0) + 1
    lines = [title, "=" * len(title)]
    if not totals:
        lines.append("(no data)")
        return "\n".join(lines) + "\n"
    grand = 0
    for key in sorted(totals):
        value = totals[key]
        grand += value
        label = f"{key} ({counts[key]})"
        lines.append(f"{label:<20}{'(' + format(value / 100, '.2f') + ')':>12}")
    lines.append("-" * WIDTH)
    lines.append(f"{'TOTAL REFUNDED':<20}{'(' + format(grand / 100, '.2f') + ')':>12}")
    return "\n".join(lines) + "\n"


def inventory_report(rows, title="STOCK BY WAREHOUSE"):
    """rows: dicts with warehouse, sku and qty. Rows with no stock are skipped."""
    totals = {}
    counts = {}
    for row in rows:
        if row.get("qty", 0) <= 0:
            continue
        key = str(row.get("warehouse", "")).strip() or "(none)"
        totals[key] = totals.get(key, 0) + row["qty"]
        counts[key] = counts.get(key, 0) + 1
    lines = [title, "=" * len(title)]
    if not totals:
        lines.append("(no data)")
        return "\n".join(lines) + "\n"
    grand = 0
    for key in sorted(totals, key=lambda k: (-totals[k], k)):
        value = totals[key]
        grand += value
        label = f"{key} ({counts[key]})"
        lines.append(f"{label:<20}{value:>12d}")
    lines.append("-" * WIDTH)
    lines.append(f"{'TOTAL UNITS':<20}{grand:>12d}")
    return "\n".join(lines) + "\n"
