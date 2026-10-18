# Configuration details

The request timeout can be set with `RELAY_TIMEOUT` or `--timeout`.

Nested settings can be given in the config file as sections:

    [http]
    request_timeout = 20
    connect_timeout = 3

Retries are controlled by `RELAY_RETRIES`.
