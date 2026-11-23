"""Port contract shared by the framework and every instance.

An instance module may export:

    actions(app)   -> {name: handler(body) -> JSON}
    inputs(app)    -> {name: handler(body) -> JSON}
    reads(app)     -> {name: handler(query) -> JSON}
    thumbnail(app, version_id) -> bytes | None

Handlers receive JSON objects and return JSON-serializable values. Raising
BadRequest yields HTTP 400; InvalidTransition/ValueError yields 409.
"""


class BadRequest(ValueError):
    """The caller sent a malformed event or query."""


def force(body: dict) -> bool:
    value = body.get("force", False)
    if not isinstance(value, bool):
        raise BadRequest("force must be a boolean")
    return value
