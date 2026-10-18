"""Contact list cleanup."""


def normalize_email(email):
    return (email or "").strip().lower()


def dedupe_contacts(contacts):
    """Merge contacts that share an email address.

    Emails are compared after stripping whitespace and lowercasing. The result has one
    contact per email, in the order each email first appears. The first contact for an
    email is kept; a later duplicate only fills in fields that are missing or empty
    (None or "") in the kept contact, never overwrites them. Contacts without an email
    are never merged and are kept as they are, in place. The input dicts are not modified.
    """
    result = []
    for c in contacts:
        key = normalize_email(c.get("email"))
        if not key:
            result.append(dict(c))
            continue
        found = None
        for kept in result:
            if normalize_email(kept.get("email")) == key:
                found = kept
                break
        if found is None:
            result.append(dict(c))
            continue
        for field, value in c.items():
            if found.get(field) in (None, "") and value not in (None, ""):
                found[field] = value
    return result
