Create `report.csv` in the project root from `data/orders.json`, following this spec exactly (it is parsed by a strict byte-for-byte importer):

1. First line is the header, exactly: `region,paid_orders,revenue,avg_order`
2. Only orders with `"status": "paid"` count. One row per region that has at least one paid order; regions with no paid orders are left out.
3. `paid_orders` = number of paid orders in the region. `revenue` = sum of their `amount`s. `avg_order` = revenue / paid_orders.
4. `revenue` and `avg_order` are written with exactly 2 decimals, no thousands separator, no currency symbol. Compute with exact decimal arithmetic (the amounts are decimal strings) and round `avg_order` half up: 2.345 → 2.35, 2.344 → 2.34.
5. The `region` value is always wrapped in double quotes, with any `"` inside it doubled (`a "b"` → `"a ""b"""`). The header and the numeric fields are never quoted.
6. Rows are sorted by revenue, highest first; ties by region name ascending.
7. Unix line endings (`\n`), no BOM, no spaces around commas, and no newline after the last row.

You may write a helper script to produce the file, but delete it before you finish: when you're done the only change in the repository must be the new file `report.csv` — no other files added, modified or deleted.
