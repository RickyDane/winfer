"""VAT rates per region (percent)."""

VAT_RATES = {
    "DE": 19,
    "FR": 20,
    "NL": 21,
    "US": 0,
    "CA": 0,
}

EU = {"DE", "FR", "NL"}


def vat_rate(region):
    return VAT_RATES.get(region, 0)
