# courier

Message relay: receives events over TCP, runs them through a middleware pipeline, dispatches them
to handlers and delivers the results downstream with retries.

    python -m courier serve

See `docs/architecture.md`. Configuration is read from `courier.ini` in the working directory
(override the path with `COURIER_CONFIG`); see `settings.example.ini` for all options.
