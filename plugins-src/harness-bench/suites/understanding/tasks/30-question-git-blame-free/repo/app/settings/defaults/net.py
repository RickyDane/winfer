from app.settings.registry import alias, register

register("net.timeout_s", 10.0, doc="Socket timeout for every connection.")
register("net.retry.attempts", 4, doc="How many times a failed request is tried in total.")
register("net.retry.base_delay_ms", 200, doc="First retry delay; doubles on every further retry.")
register("net.retry.jitter", False, doc="Reserved; jitter is not implemented yet.")
register("net.proxy", "", doc="HTTP(S) proxy URL, empty for none.")
alias("retry.attempts", "net.retry.attempts")
