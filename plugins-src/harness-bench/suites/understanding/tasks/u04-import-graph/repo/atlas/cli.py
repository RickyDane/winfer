import argparse

from atlas.plugins import registry


def main(argv=None):
    p = argparse.ArgumentParser("atlas")
    p.add_argument("plugin", choices=sorted(registry.PLUGINS))
    args = p.parse_args(argv)
    registry.PLUGINS[args.plugin]()
