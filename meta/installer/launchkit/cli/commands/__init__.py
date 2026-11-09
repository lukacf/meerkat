"""Command registration for meta-forge."""
from __future__ import annotations


def register(commands, command_parser):
    from . import host, installation, setup
    host.register(commands, command_parser)
    setup.register(commands, command_parser)
    installation.register(commands, command_parser)
