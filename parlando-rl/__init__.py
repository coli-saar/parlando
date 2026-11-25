"""Exports for OpenEnv discovery when this checkout is used as a server source."""

__all__ = ["ParlandoAction", "ParlandoOpenenvEnv", "ParlandoObservation"]


def __getattr__(name: str):
    """Load OpenEnv only when a discovery client requests an adapter export."""
    if name not in __all__:
        raise AttributeError(name)
    from parlando_rl.openenv import client

    return getattr(client, name)
