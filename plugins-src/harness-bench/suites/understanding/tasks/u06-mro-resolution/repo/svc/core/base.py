class BaseHandler:
    name = "base"
    attempts = 1

    def handle(self, req):
        return [f"{self.name}:{req}"]

    def describe(self):
        return f"<{self.name}>"
