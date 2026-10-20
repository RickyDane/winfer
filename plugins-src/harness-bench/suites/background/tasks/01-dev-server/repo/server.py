"""Dev server: 'compiles' for a few seconds, then serves the API on a free port."""
import http.server, os, random, socketserver, string, sys, time

open(".git/server.pid", "w").write(str(os.getpid()))
print("compiling assets…", flush=True)
for i in range(1, 6):
    time.sleep(0.8)
    print(f"  built chunk {i}/5", flush=True)
token = "".join(random.choice(string.ascii_lowercase) for _ in range(12))
with open(".git/access.log", "w") as f:
    f.write(f"token {token}\n")

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = (token if self.path == "/api/secret" else "ok").encode()
        self.send_response(200 if self.path in ("/", "/api/secret") else 404)
        self.end_headers()
        self.wfile.write(body)
        with open(".git/access.log", "a") as f:
            f.write(f"GET {self.path}\n")
    def log_message(self, fmt, *args):
        print("request:", fmt % args, flush=True)

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", 0), H) as srv:
    print(f"ready: listening on http://127.0.0.1:{srv.server_address[1]}", flush=True)
    last = time.time()
    srv.timeout = 0.5
    while True:
        srv.handle_request()
        if time.time() - last > 1:
            print("hmr: watching for changes", flush=True)
            last = time.time()
