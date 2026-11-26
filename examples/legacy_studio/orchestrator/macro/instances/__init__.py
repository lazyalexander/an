"""Instance registry: the whole surface a new instance must implement.

A module registered here exports:

    MANIFEST          dict   — see instances/bridge.py for the contract
    stages(ctx)       list   — one {"id", "status", "detail"} per manifest stage
    pending(ctx)      list   — decisions awaiting the user, keyed by decision kind

Adding an instance means adding a module and one registry row; the state
builder, HTTP server and page stay untouched.
"""

from . import bridge, cad_animation

INSTANCES = {
    "bridge-studio": bridge,
    "cad-animation": cad_animation,
}


def names() -> list[str]:
    return sorted(INSTANCES)


def load(name: str):
    try:
        return INSTANCES[name]
    except KeyError:
        raise ValueError(
            f"unknown instance {name!r}; known: {', '.join(names())}") from None
