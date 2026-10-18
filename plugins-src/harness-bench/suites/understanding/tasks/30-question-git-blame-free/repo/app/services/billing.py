class BillingSync:
    def __init__(self, http, events):
        self.http = http
        self.events = events

    def push_invoice(self, invoice):
        r = self.http.post("invoices", invoice)
        if r.status == 200:
            self.events.publish("invoice.synced", {"id": invoice.get("id")})
        return r.status
