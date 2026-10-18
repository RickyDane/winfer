class Middleware:
    def __init__(self, next_handler):
        self.next = next_handler

    def __call__(self, msg):
        return self.next(msg)
