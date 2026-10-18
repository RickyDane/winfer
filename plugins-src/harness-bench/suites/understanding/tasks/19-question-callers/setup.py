"""Generates the project for this task into the directory given as argv[1] (deterministic)."""

import os
import random
import sys

root = sys.argv[1]
rng = random.Random(19)


def write(path, text):
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)


write(
    "audit.py",
    '''"""Audit trail. Everything security-relevant goes through log_event()."""

import json
import time

_SINK = []


def log_event(kind, **fields):
    """Append an audit event. Call as audit.log_event("login", user=...)."""
    _SINK.append(json.dumps({"kind": kind, "at": time.time(), **fields}, sort_keys=True))


def log_login(user):
    log_event("login", user=user)


def log_logout(user):
    # Not audited any more: log_event("logout", user=user)
    pass


def events():
    return list(_SINK)
''',
)

write(
    "metrics.py",
    '''"""Metrics (not audit). Has its own log_event, unrelated to audit.log_event."""

_COUNTS = {}


def log_event(name, value=1):
    _COUNTS[name] = _COUNTS.get(name, 0) + value


def snapshot():
    return dict(_COUNTS)
''',
)

areas = ["accounts", "billing", "orders", "inventory", "reports", "admin", "shipping", "support"]
write("app/__init__.py", "")
for a in areas:
    write(f"app/{a}/__init__.py", "")

nouns = ["user", "invoice", "order", "item", "report", "ticket", "shipment", "refund", "token", "role", "note", "batch"]
verbs = ["create", "update", "delete", "load", "sync", "close", "open", "archive", "export", "validate", "merge", "approve"]


def filler(indent="    "):
    k = rng.randrange(4)
    if k == 0:
        return f"{indent}total = sum(x * {rng.randint(2, 9)} for x in range({rng.randint(3, 30)}))\n{indent}return total\n"
    if k == 1:
        return f"{indent}data = {{'id': {rng.randint(1, 999)}, 'ok': True}}\n{indent}data['seen'] = len(data)\n{indent}return data\n"
    if k == 2:
        return f"{indent}if not args:\n{indent}    return None\n{indent}return args[0]\n"
    return f"{indent}result = []\n{indent}for a in args:\n{indent}    result.append(str(a).strip())\n{indent}return result\n"


used = set()


def fname():
    while True:
        n = f"{rng.choice(verbs)}_{rng.choice(nouns)}"
        if n in used:
            n = f"{n}_{rng.choice(['v2', 'fast', 'all', 'safe', 'bulk', 'once'])}"
        if n not in used:
            used.add(n)
            return n


# Hand-placed tricky modules.
write(
    "app/accounts/session.py",
    '''import audit
from metrics import log_event as count


def sign_in(user, password):
    if not password:
        count("signin.empty")
        return False
    audit.log_event("signin", user=user)
    audit.log_event("signin.detail", user=user, ok=True)
    return True


def sign_out(user):
    count("signout")
    return True


def describe():
    """Calls audit.log_event("describe") when ... no, it doesn't."""
    return "audit.log_event is called by sign_in"


class SessionStore:
    def __init__(self):
        self.items = {}

    def expire(self, sid):
        self.items.pop(sid, None)
        audit.log_event("expire", sid=sid)

    def touch(self, sid):
        # audit.log_event("touch", sid=sid)  -- too noisy
        self.items[sid] = True


audit.log_event("module.loaded", module=__name__)
''',
)

write(
    "app/accounts/roles.py",
    '''from audit import log_event as record


def grant(user, role):
    record("grant", user=user, role=role)


def revoke(user, role):
    def _do():
        record("revoke", user=user, role=role)

    _do()
    return True


def replay(entries, record):
    # `record` here is the caller's callback, not the audit function.
    for e in entries:
        record(e)


def handlers():
    return [record]
''',
)

write(
    "app/billing/charges.py",
    '''import metrics
from metrics import log_event


def charge(card, cents):
    log_event("charge", cents)
    metrics.log_event("charge.count")
    return {"card": card, "cents": cents}


def refund(card, cents):
    log_event("refund", cents)
    return {"card": card, "cents": -cents}
''',
)

write(
    "app/billing/disputes.py",
    '''import audit as au


def open_dispute(charge_id, reason):
    au.log_event(
        "dispute.open",
        charge=charge_id,
        reason=reason,
    )
    return charge_id


def close_dispute(charge_id):
    msg = "au.log_event('dispute.close')"
    return msg
''',
)

write(
    "app/orders/checkout.py",
    '''from audit import log_event
import metrics


def place(order):
    metrics.log_event("place")
    log_event("order.placed", order=order)


def cancel(order):
    metrics.log_event("cancel")


class Checkout:
    def confirm(self, order):
        place(order)

    def abort(self, order):
        log_event("order.aborted", order=order)
        cancel(order)

    @staticmethod
    def preview(order):
        return {"order": order}
''',
)

write(
    "app/orders/history.py",
    '''def record(order, store):
    store.append(order)


def archive(orders, store):
    for o in orders:
        record(o, store)
''',
)

write(
    "app/admin/tools.py",
    '''import audit
from audit import log_event as record


def purge(user):
    record = lambda *a, **k: None  # silenced during purge
    record("purge", user=user)


def ban(user):
    audit.log_event("ban", user=user)
    record("ban.detail", user=user)


def unban(user):
    return user


def _impersonate(user):
    record("impersonate", user=user)
''',
)

write(
    "app/support/tickets.py",
    '''from audit import log_event as record
from audit import events


class Ticket:
    def __init__(self, title):
        self.title = title
        record("ticket.new", title=title)

    def escalate(self):
        return self.title.upper()

    def resolve(self):
        record("ticket.resolve", title=self.title)
        record("ticket.resolve.detail", title=self.title)


def recent():
    return events()[-10:]
''',
)

write(
    "app/shipping/labels.py",
    '''import audit


def print_label(shipment):
    return f"LABEL {shipment}"


def void_label(shipment):
    return {"void": shipment, "by": "audit.log_event"}


def reprint(shipment):
    """Reprints. TODO: call audit.log_event here."""
    return print_label(shipment)
''',
)

write(
    "_unused/old_audit_users.py",
    '''import audit
from audit import log_event as record


def legacy_login(user):
    audit.log_event("login", user=user)


def legacy_export(rows):
    record("export", n=len(rows))
''',
)

# Generated bulk: many modules with a mix of real calls and look-alikes.
gen_positive = 0
for a in areas:
    for m in range(rng.randint(3, 5)):
        style = rng.choice(["mod", "alias", "plain", "metrics", "none", "none"])
        head = {
            "mod": "import audit\n",
            "alias": "from audit import log_event as record\n",
            "plain": "from audit import log_event\n",
            "metrics": "from metrics import log_event\n",
            "none": "",
        }[style]
        call = {
            "mod": "audit.log_event",
            "alias": "record",
            "plain": "log_event",
            "metrics": "log_event",
            "none": None,
        }[style]
        body = head + "import os\n\n\n"
        for _ in range(rng.randint(3, 7)):
            name = fname()
            body += f"def {name}(*args):\n"
            r = rng.random()
            if call and r < 0.3:
                body += f'    {call}("{name}", args=len(args))\n'
                if style != "metrics":
                    gen_positive += 1
            elif call and r < 0.4:
                body += f'    # {call}("{name}")  disabled\n'
            elif r < 0.5:
                body += f'    """See audit.log_event for the event format."""\n'
            body += filler() + "\n\n"
        write(f"app/{a}/mod_{m}.py", body)

write("README.md", "# backoffice\n\nAudit events: see audit.py. Metrics: see metrics.py.\n")
