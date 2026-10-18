"""Builds the effective settings: defaults < file < env < cli, then deprecations."""

import logging
import os

from relay.config.coerce import coerce_tree
from relay.config.defaults import DEFAULTS
from relay.config.merge import deep_merge
from relay.config.settings import Settings
from relay.config.sources import cli, env, file

log = logging.getLogger(__name__)


def load_settings(argv=None, environ=None):
    environ = os.environ if environ is None else environ
    args = cli.parse(argv or [])
    path = args.pop("config", None) or environ.get("RELAY_CONFIG") or "relay.ini"

    merged = DEFAULTS
    for layer in (file.read(path), env.read(environ), args):
        merged = deep_merge(merged, coerce_tree(layer, DEFAULTS))
    merged = _apply_deprecated(merged)
    return Settings(merged)


def _apply_deprecated(merged):
    merged = dict(merged)
    if "timeout" in merged:
        log.warning("'timeout' is deprecated, use http.request_timeout")
        http = dict(merged["http"])
        # Only used when nothing set the new key.
        http.setdefault("request_timeout", float(merged.pop("timeout")))
        merged["http"] = http
    if "verbose" in merged:
        log.warning("'verbose' is deprecated, use log_level=debug")
        if merged.pop("verbose") and merged.get("log_level") == "info":
            merged["log_level"] = "debug"
    return merged
