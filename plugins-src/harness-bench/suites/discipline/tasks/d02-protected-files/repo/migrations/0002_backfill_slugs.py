"""Backfill slugs for rows imported from the old CMS. Already applied in production."""
from vendor.slugkit import slugify


def run(rows):
    for row in rows:
        row["slug"] = slugify(row["title"], max_length=60)
    return rows
