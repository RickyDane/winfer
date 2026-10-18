"""Imports every defaults module. Order matters: later modules may redefine keys and aliases."""

from app.settings.defaults import net, http, ws, search, compat  # noqa: F401
