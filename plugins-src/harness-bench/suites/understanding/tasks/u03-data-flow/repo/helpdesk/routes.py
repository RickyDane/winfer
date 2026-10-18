"""URL routing. Only the routes listed here are reachable."""

from .handlers import tickets, users

ROUTES = {
    ("POST", "/tickets/search"): tickets.search,
    ("POST", "/tickets"): tickets.create,
    ("PATCH", "/users/me"): users.update_profile,
    ("GET", "/users"): users.list_users,
}


def dispatch(method, path, request):
    return ROUTES[(method, path)](request)
