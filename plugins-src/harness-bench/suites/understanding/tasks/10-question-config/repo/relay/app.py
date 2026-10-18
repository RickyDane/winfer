"""Entry point: load settings, build the forwarder and run it."""

import logging

from relay.config.loader import load_settings
from relay.forwarder import Forwarder
from relay.net.client import HttpClient


def main(argv):
    settings = load_settings(argv)
    logging.basicConfig(level=settings.log_level.upper())
    client = HttpClient.from_settings(settings)
    fwd = Forwarder(client, settings.upstream, retries=settings.retries)
    return fwd.run_stdin()
