"""HTTP tile server."""

from http.server import BaseHTTPRequestHandler

from atlas.render import colors
from atlas.util import clamp


class TileHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(colors.PALETTE[clamp(len(self.path), 0, 3)].encode())
