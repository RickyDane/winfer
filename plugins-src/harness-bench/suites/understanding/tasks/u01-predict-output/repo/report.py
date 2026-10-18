"""Nightly inventory report. Run: python3 report.py"""

from stamp import mine


def label(item, seen=[]):
    seen.append(item)
    return f"{item}#{len(seen)}"


def scalers():
    return [lambda v: v * k for k in (1, 2, 3)]


def safe_div(a, b):
    try:
        return a / b
    except ZeroDivisionError:
        return "inf"
    finally:
        if b == 0:
            return "nan"


def restock(stock):
    del stock["apple"]
    stock["pear"] = 4  # pear was out of stock
    stock["apple"] = 7
    stock.setdefault("fig", 0)
    stock.setdefault("plum", 1)
    return stock


def main():
    stamp = mine("inventory-2024-06-01")
    flags = stamp << 4 & 0xF

    stock = restock({"apple": 3, "pear": 0, "fig": 5, "kiwi": 2})
    print(", ".join(f"{name}={qty}" for name, qty in stock.items()))

    factor = 1
    data = list(stock.values())
    scaled = (v * factor for v in data)
    factor = 10
    data.append(flags + 1)
    data = [100]
    print(sum(scaled), sum(scaled))

    print([f(2) for f in scalers()])
    print(label("a"), label("b"), label("c", []), label("d"))
    print(safe_div(6, 3), safe_div(1, 0))
    print(-2 ** 2 + flags, 7 & 3 == 3, not 1 == 2 in [2], 1 < 2 == 2)

    totals = {}
    for name, qty in stock.items():
        totals[qty % 3] = totals.get(qty % 3, []) + [name]
    print(totals)


if __name__ == "__main__":
    main()
