"""python3 -m fileserver.serve ROOT [PORT]"""

import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .static import handle


def main():
    root = sys.argv[1]
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 8000

    class H(BaseHTTPRequestHandler):
        def _go(self):
            status, headers, body = handle(root, self.command, self.path)
            self.send_response(status)
            for k, v in headers.items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        do_GET = do_HEAD = do_POST = _go

    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()


if __name__ == "__main__":
    main()
