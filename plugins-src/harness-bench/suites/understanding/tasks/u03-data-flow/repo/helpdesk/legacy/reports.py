"""Old reporting endpoint (removed from routing in 2022)."""

from ..db.conn import execute


def report(request):
    email = request.json["email"]
    return execute(f"SELECT * FROM tickets WHERE reporter_email = '{email}'")
