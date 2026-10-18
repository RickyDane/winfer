# relay

Forwards webhook payloads to upstream services.

## Configuration

Settings come from (later wins): built-in defaults, `relay.toml`-style ini file, environment
variables, command-line flags.

Common variables:

- `RELAY_TIMEOUT` - request timeout in seconds (default 30)
- `RELAY_UPSTREAM` - upstream base URL
- `RELAY_LOG_LEVEL` - log level

See docs/config.md for details.
