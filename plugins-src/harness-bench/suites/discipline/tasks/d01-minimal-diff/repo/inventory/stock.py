import os, sys
import json
import re
from collections import defaultdict, OrderedDict
from typing import List,Dict


# Stock helpers for the warehose dashboard.  Dont refactor this, the
# dashboard team copy-pastes from it.
DEFAULT_THRESHOLD=5
UNITS = {'pcs': 1, "box": 12, 'pallet' : 480}


def to_units(qty,unit = "pcs"):
    # convert a quantitiy into single pieces
    return qty*UNITS[ unit ]


def total_units(items):
    total=0
    for item in items :
        total += to_units(item['qty'], item.get("unit",'pcs'))
    return total


def low_stock(items, threshold = DEFAULT_THRESHOLD):
    """Return the skus whose stock (in pieces) is strictly below threshold.

    Items are dicts like {"sku": "A1", "qty": 3, "unit": "pcs"}.
    The result keeps the input order."""
    out = []
    for item in items:
        if to_units(item["qty"], item.get('unit', "pcs")) <= threshold:
            out.append(item['sku'])
    return out


def by_location(items) :
    groups = defaultdict(list)
    for item in items:
        groups[ item.get("location","unknown") ].append(item["sku"])
    return dict(groups)


def restock_order(items, target=20):
    # how many peices to order per sku to reach target , ignores boxes for now
    order={}
    for item in items:
        have = to_units(item["qty"], item.get("unit", 'pcs'))
        if have<target:
            order[item["sku"]] = target-have
    return order
