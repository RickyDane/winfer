class UserSync:
    def __init__(self, http):
        self.http = http

    def pull(self, since=None):
        path = "users" + (f"?since={since}" if since else "")
        return self.http.get(path).body
