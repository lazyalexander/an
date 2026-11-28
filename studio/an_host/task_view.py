"""Expose public task fields and current permission requests to clients."""


def task_view(graph, agent_status):
    result = graph("view", {})
    if result["authorized"] and result["revision"]:
        graph("state", {"after": 2**53})
        result = graph("view", {})
    # Keep execution evidence behind the explicit sources/evidence actions.
    for collection, fields in (
        ("nodes", ("id", "kind", "title")),
        ("activity", ("id", "status")),
    ):
        result[collection] = {
            node: {field: row[field] for field in fields if field in row}
            for node, row in result[collection].items()
        }
    result["permissions"] = graph("requests", {}) if result["authorized"] else []
    result["deliveries"] = graph("deliveries", {})
    result["agent"] = agent_status
    return result
