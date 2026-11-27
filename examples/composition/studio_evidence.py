"""Grade the real host graph, held publication, delivery bytes and Codex traces."""

import base64
import hashlib
import json

from live import expected_outputs
from scoring import assess_safely


def verify_plan(runtime, case, graph, previous, cursor):
    nodes, activity = graph["nodes"], graph["activity"]
    grade = assess_safely(
        {"decision": "plan", "nodes": list(nodes.values())}, case, runtime.images
    )
    if not all(grade.values()):
        raise ValueError(f"Submitted composition failed fixture grading: {grade}")
    added = set(nodes) - set(previous.get("nodes", {}))
    submissions = [
        e
        for e in graph["events"]
        if e["kind"] == "submitted"
        and e["actor"] == "agent"
        and e.get("instruction_cursor") == cursor
    ]
    if not added or {node for event in submissions for node in event["nodes"]} != added:
        raise ValueError("New nodes lack an agent submission tied to this instruction")
    for key, node in previous.get("nodes", {}).items():
        old = previous["activity"][key]
        if nodes.get(key) != node or any(
            activity[key].get(k) != old.get(k) for k in ("id", "attempt")
        ):
            raise ValueError("Previously submitted node changed or executed again")
        if node["kind"] == "run" and activity[key] != old:
            raise ValueError("Original completed run changed or executed again")
    return sorted(added)


def held_publication(runtime, graph, pending, deliveries):
    if deliveries or len(pending) != 1:
        raise ValueError("Expected a single pending permission before any delivery")
    key, node = next(
        (k, n) for k, n in graph["nodes"].items() if n["kind"] == "delivery"
    )
    slot, ref = next(iter(node["inputs"].items()))
    activity = graph["activity"]
    consumer = activity[key]
    producer = activity[ref["node"]]
    version = producer["outputs"][ref["port"]]["revision"]
    request = pending[0]
    if (
        producer["status"] != "done"
        or consumer["status"] != "permission_required"
        or consumer["requests"][slot] != request["id"]
        or request["operation"] != "publish"
        or request["scope"] != [runtime.images["line-art"], slot, runtime.workspace]
        or request["details"] != [consumer["id"], version]
        or request["revision"] != version
    ):
        raise ValueError(
            "The pending decision does not guard publication of the finished line art"
        )
    return {
        "request": request["id"],
        "revision": version,
        "producer_attempt": producer["id"],
        "delivery_attempt": consumer["id"],
    }


def verify_delivery(request, case, graph):
    nodes, activity = graph["nodes"], graph["activity"]
    expected = expected_outputs(case)
    opened, revisions, deliveries = {}, {}, {}
    for key, node in nodes.items():
        for name, ref in node["inputs"].items():
            if "node" in ref:
                version = activity[ref["node"]]["outputs"][ref["port"]]["revision"]
                if activity[key]["bindings"][name]["revision"] != version:
                    raise ValueError(
                        f"{key}.{name} did not consume the upstream revision"
                    )
                revisions[f"{key}.{name}"] = version
        if node["kind"] == "delivery":
            delivery = activity[key]["delivery"]
            deliveries[key] = delivery
            for files in request("open", delivery=delivery)["files"].values():
                for name, data in files.items():
                    if name in opened:
                        raise ValueError("Duplicate delivered filename")
                    opened[name] = base64.b64decode(
                        data if isinstance(data, str) else data["data"], validate=True
                    )
    if opened != expected:
        raise ValueError("Studio delivery bytes differ from the fixture oracle")
    return {
        "revision": graph["revision"],
        "bound_revisions": revisions,
        "deliveries": deliveries,
        "sha256": {k: hashlib.sha256(v).hexdigest() for k, v in opened.items()},
    }


def verify_wakes(directory, task, records):
    state = directory / "registry" / "agents" / task
    wakes = sorted(state.glob("wake-*.jsonl"), key=lambda p: int(p.stem.split("-")[1]))
    thread = (state / "thread").read_text()
    for index, record in enumerate(records):
        # A decision can wake Codex again if it waited for the human before submission.
        low = record["cursor"]
        high = (
            records[index + 1]["cursor"] if index + 1 < len(records) else float("inf")
        )
        paths, tools = [], set()
        for trace in wakes:
            if not low <= int(trace.stem.split("-")[1]) < high:
                continue
            events = [json.loads(line) for line in trace.read_text().splitlines()]
            if not any(
                e.get("type") == "thread.started" and e.get("thread_id") == thread
                for e in events
            ) or not any(e.get("type") == "turn.completed" for e in events):
                raise ValueError("Codex did not complete a wake on the original thread")
            paths.append(str(trace.relative_to(directory)))
            for event in events:
                item = event.get("item", {})
                if (
                    event["type"] == "item.completed"
                    and item.get("type") == "mcp_tool_call"
                    and item.get("server") == "an"
                    and item.get("status") == "completed"
                    and not item.get("error")
                ):
                    tools.add(item["tool"])
        if not {"an_scope_propose", "an_graph_submit"} <= tools:
            raise ValueError(
                f"Missing completed Codex scope/graph tool calls for instruction {low}"
            )
        record["codex_traces"] = paths
        record["codex_tools"] = sorted(tools)
    return thread
