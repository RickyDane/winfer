"""Compatibility shims kept for old config files.

Since 2.3 HTTP retries share the transport-level backoff instead of having their own; the
http.* names below are kept so existing overrides keep working.
"""

from app.settings.registry import alias

alias("http.retry.backoff", "net.retry.base_delay_ms")
alias("http.timeout", "net.timeout_s")
